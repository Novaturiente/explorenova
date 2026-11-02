use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceKind {
    /// Physical/removable block device discovered via udiskie.
    Block,
    /// Saved SMB network share. `url` is the gio mount URL.
    Smb { url: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub device: String,
    pub label: Option<String>,
    pub size: Option<String>,
    pub fstype: Option<String>,
    pub mountpoint: Option<PathBuf>,
    pub kind: DeviceKind,
}

impl Device {
    pub fn is_mounted(&self) -> bool {
        self.mountpoint.is_some()
    }
    pub fn is_smb(&self) -> bool {
        matches!(self.kind, DeviceKind::Smb { .. })
    }
}

/// Parse a single tab-separated line from `udiskie-info -a -o ...`.
/// Expected columns: device_presentation, is_mounted, mount_paths,
/// device_size, id_label, id_type. Empty columns become `None`.
/// Returns `None` if the line has fewer than 6 columns.
fn parse_line(line: &str) -> Option<Device> {
    let cols: Vec<&str> = line.split('\t').collect();
    if cols.len() < 6 { return None; }
    let device = cols[0].trim().to_string();
    if device.is_empty() { return None; }
    // udiskie-info formats list-typed columns ({mount_paths}) as the
    // Python repr of a list of strings — e.g. "['/run/media/u/X']" — so
    // strip the surrounding `[ ]` and per-entry `'` or `"` before parsing.
    let mount_paths = cols[2]
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim();
    let mountpoint = if mount_paths.is_empty() {
        None
    } else {
        // multi-mount: take the first path
        let first = mount_paths
            .split(',')
            .next()?
            .trim()
            .trim_matches(|c| c == '\'' || c == '"')
            .trim();
        if first.is_empty() { None } else { Some(PathBuf::from(first)) }
    };
    let opt = |s: &str| {
        let s = s.trim();
        if s.is_empty() { None } else { Some(s.to_string()) }
    };
    Some(Device {
        device,
        label: opt(cols[4]),
        size: opt(cols[3]),
        fstype: opt(cols[5]),
        mountpoint,
        kind: DeviceKind::Block,
    })
}

/// Build the full devices row list: physical block devices first, then
/// saved SMB shares with live mount status. Shared by the `M` devices
/// modal and the sidebar's Disks section.
pub fn build_rows(shares: &[crate::smb::SmbShare]) -> Vec<Device> {
    let mut items = list().unwrap_or_default();
    items.extend(crate::smb::to_device_rows(shares));
    items
}

/// Ceiling on any udiskie call. These run on the UI thread, and udisks2
/// talks to D-Bus: a dead gvfs mount or a stuck udisksd left `output()`
/// blocked forever with no way out, freezing the whole event loop.
const UDISKIE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// `Command::output()` with a hard deadline. Both pipes are drained on
/// their own threads so a chatty child cannot deadlock against the wait
/// loop, and the child is killed on expiry.
fn output_with_timeout(mut cmd: Command) -> anyhow::Result<std::process::Output> {
    use std::io::Read;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let drain = |mut pipe: Option<std::process::ChildStdout>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(p) = pipe.as_mut() {
                let _ = p.read_to_end(&mut buf);
            }
            buf
        })
    };
    let out_r = drain(child.stdout.take());
    // Same shape for stderr; the types differ so it can't share `drain`.
    let mut err_pipe = child.stderr.take();
    let err_r = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = err_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });
    let t0 = std::time::Instant::now();
    let status = loop {
        match child.try_wait()? {
            Some(st) => break st,
            None if t0.elapsed() >= UDISKIE_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("timed out after {}s", UDISKIE_TIMEOUT.as_secs());
            }
            None => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    };
    Ok(std::process::Output {
        status,
        stdout: out_r.join().unwrap_or_default(),
        stderr: err_r.join().unwrap_or_default(),
    })
}

pub fn list() -> anyhow::Result<Vec<Device>> {
    let mut cmd = Command::new("udiskie-info");
    cmd.args([
        "-a",
        "-o",
        "{device_presentation}\t{is_mounted}\t{mount_paths}\t{device_size}\t{id_label}\t{id_type}",
    ]);
    let out = output_with_timeout(cmd)
        .map_err(|e| anyhow::anyhow!("udiskie-info not available: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        anyhow::bail!("udiskie-info failed: {}", err.trim());
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    Ok(stdout.lines().filter_map(parse_line).collect())
}

pub fn mount(device: &str) -> anyhow::Result<()> {
    let mut cmd = Command::new("udiskie-mount");
    cmd.arg(device);
    let out = output_with_timeout(cmd)
        .map_err(|e| anyhow::anyhow!("udiskie-mount not available: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        anyhow::bail!("{}", err.trim());
    }
    Ok(())
}

pub fn unmount(device: &str) -> anyhow::Result<()> {
    let mut cmd = Command::new("udiskie-umount");
    cmd.arg(device);
    let out = output_with_timeout(cmd)
        .map_err(|e| anyhow::anyhow!("udiskie-umount not available: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        anyhow::bail!("{}", err.trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_line_with_all_fields() {
        let line = "/dev/sdb1\tTrue\t/run/media/u/Backup\t32 GB\tBackup\text4";
        let d = parse_line(line).unwrap();
        assert_eq!(d.device, "/dev/sdb1");
        assert_eq!(d.size.as_deref(), Some("32 GB"));
        assert_eq!(d.label.as_deref(), Some("Backup"));
        assert_eq!(d.fstype.as_deref(), Some("ext4"));
        assert_eq!(
            d.mountpoint,
            Some(PathBuf::from("/run/media/u/Backup"))
        );
        assert!(d.is_mounted());
    }

    #[test]
    fn parse_line_with_empty_label_and_mountpoint() {
        let line = "/dev/sdc1\tFalse\t\t8 GB\t\tvfat";
        let d = parse_line(line).unwrap();
        assert_eq!(d.device, "/dev/sdc1");
        assert!(d.label.is_none());
        assert!(d.mountpoint.is_none());
        assert!(!d.is_mounted());
        assert_eq!(d.fstype.as_deref(), Some("vfat"));
    }

    #[test]
    fn parse_skips_malformed_lines() {
        assert!(parse_line("only\ttwo\tcols").is_none());
        assert!(parse_line("").is_none());
    }

    #[test]
    fn mount_paths_takes_first_when_comma_separated() {
        let line = "/dev/sdd1\tTrue\t/mnt/a,/mnt/b\t1 GB\tdouble\text4";
        let d = parse_line(line).unwrap();
        assert_eq!(d.mountpoint, Some(PathBuf::from("/mnt/a")));
    }

    #[test]
    fn mount_paths_strips_python_list_repr() {
        // udiskie-info formats {mount_paths} as the Python repr of a list:
        // ['/run/media/nova/JYESH']. Parser must strip [ ] and quotes.
        let line =
            "/dev/sdb1\tTrue\t['/run/media/nova/JYESH']\t32 GB\tJYESH\text4";
        let d = parse_line(line).unwrap();
        assert_eq!(
            d.mountpoint,
            Some(PathBuf::from("/run/media/nova/JYESH"))
        );

        // Double-quoted variant
        let line2 =
            "/dev/sdc1\tTrue\t[\"/run/media/u/Stick\"]\t8 GB\tStick\tvfat";
        let d2 = parse_line(line2).unwrap();
        assert_eq!(
            d2.mountpoint,
            Some(PathBuf::from("/run/media/u/Stick"))
        );

        // Multi-entry list — take first, strip quotes
        let line3 =
            "/dev/sdd1\tTrue\t['/mnt/a', '/mnt/b']\t1 GB\tdouble\text4";
        let d3 = parse_line(line3).unwrap();
        assert_eq!(d3.mountpoint, Some(PathBuf::from("/mnt/a")));
    }
}
