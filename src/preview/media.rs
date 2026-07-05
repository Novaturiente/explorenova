use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Hard ceiling on any preview subprocess. There is ONE preview worker
/// thread — a single hung rasterizer (malformed video, dead network
/// mount) would otherwise stall every later preview for the session.
const SUBPROCESS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Spawn `cmd` (stdin null, stderr null, stdout piped + captured) and
/// wait at most `timeout`, killing the child on expiry. Returns the
/// exit status and captured stdout, or None on timeout/spawn failure.
/// stderr stays silenced — rasterizer progress lines would otherwise
/// leak into the ratatui alt-screen as phantom text.
pub(crate) fn capture_with_timeout(
    cmd: &mut Command,
    timeout: std::time::Duration,
) -> Option<(std::process::ExitStatus, Vec<u8>)> {
    use std::io::Read;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut pipe = child.stdout.take()?;
    // Drain stdout concurrently so a chatty child can't fill the pipe
    // and deadlock against our wait loop.
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        buf
    });
    let t0 = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let buf = reader.join().unwrap_or_default();
                return Some((status, buf));
            }
            Ok(None) if t0.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait(); // closes the pipe; reader exits
                return None;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(25)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Run `cmd` silenced with the standard timeout; status only.
fn run_silent(cmd: &mut Command) -> Option<std::process::ExitStatus> {
    capture_with_timeout(cmd, SUBPROCESS_TIMEOUT).map(|(st, _)| st)
}

pub const VIDEO_EXTS: &[&str] = &[
    "mp4", "mkv", "webm", "mov", "avi", "wmv", "flv", "m4v", "mpg",
    "mpeg", "3gp", "ts",
];

pub fn is_video(ext_lower: &str) -> bool {
    VIDEO_EXTS.contains(&ext_lower)
}

/// Temp-cache path for a rasterized variant of `src`. Keyed by the FULL
/// path (hashed) + mtime: stem+mtime alone collides for two same-named
/// files touched in the same second (e.g. one `cp -r`), showing the
/// wrong thumbnail. mtime still forces a fresh render after an edit.
fn temp_png_for(prefix: &str, src: &Path) -> Option<PathBuf> {
    let mtime = std::fs::metadata(src)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(std::env::temp_dir().join(format!(
        "explorenova-{prefix}-{:016x}-{mtime}.png",
        path_hash(src)
    )))
}

fn path_hash(p: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    p.hash(&mut h);
    h.finish()
}

/// Delete rasterized temp PNGs older than a day. Run once at startup on
/// a detached thread — long sessions otherwise leak thumbnails in /tmp
/// for the machine's uptime.
pub fn cleanup_stale_temps() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    let cutoff =
        std::time::SystemTime::now() - std::time::Duration::from_secs(24 * 3600);
    for e in entries.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("explorenova-") || !name.ends_with(".png") {
            continue;
        }
        let stale = e
            .metadata()
            .and_then(|m| m.modified())
            .map(|m| m < cutoff)
            .unwrap_or(false);
        if stale {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Rasterize PDF page 1 to a PNG in the temp dir via `pdftoppm`.
/// Returns None if poppler is not installed or the call fails.
pub fn rasterize_pdf(pdf: &Path) -> Option<PathBuf> {
    // Key the temp name by full-path hash + mtime so two different PDFs
    // sharing a filename don't read back each other's stale
    // rasterization. (pdftoppm appends ".png" to this stem.)
    let mtime = std::fs::metadata(pdf)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let stem = std::env::temp_dir().join(format!(
        "explorenova-pdf-{:016x}-{mtime}",
        path_hash(pdf)
    ));
    let status = run_silent(
        Command::new("pdftoppm")
            .args(["-png", "-singlefile", "-f", "1", "-l", "1"])
            .arg(pdf)
            .arg(&stem),
    )?;
    if !status.success() {
        return None;
    }
    let png = stem.with_extension("png");
    png.exists().then_some(png)
}

/// Decode AVIF to PNG via `avifdec` (libavif). Returns None if the tool
/// is not installed or the file is malformed. Output cached in temp.
pub fn rasterize_avif(src: &Path) -> Option<PathBuf> {
    let out = temp_png_for("avif", src)?;
    if out.exists() {
        return Some(out);
    }
    let status =
        run_silent(Command::new("avifdec").arg(src).arg(&out))?;
    if !status.success() {
        return None;
    }
    out.exists().then_some(out)
}

/// First-frame thumbnail of a video via `ffmpegthumbnailer`. Default
/// size 512 px wide; ffmpegthumbnailer outputs PNG when the extension
/// is .png. Returns None if the tool is missing or extraction fails.
pub fn video_thumbnail(src: &Path) -> Option<PathBuf> {
    let out = temp_png_for("video", src)?;
    if out.exists() {
        return Some(out);
    }
    let status = run_silent(
        Command::new("ffmpegthumbnailer")
            .args(["-s", "512", "-q", "8", "-i"])
            .arg(src)
            .arg("-o")
            .arg(&out),
    )?;
    if !status.success() {
        return None;
    }
    out.exists().then_some(out)
}
