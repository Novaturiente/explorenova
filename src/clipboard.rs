//! Write file paths to the system clipboard so the user can paste them
//! into terminals, editors, or other apps. Detects Wayland (`wl-copy`)
//! vs X11 (`xclip`) at runtime and falls through cleanly when neither
//! tool is available.
//!
//! Format: plain newline-separated absolute paths (no `file://` prefix,
//! no percent-encoding). Pasting into a terminal or editor yields the
//! raw path directly.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn build_payload(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

fn write_to(cmd: &mut Command, payload: &str) -> std::io::Result<()> {
    // wl-copy (and xclip) fork a daemon that keeps the clipboard alive
    // after the parent exits. If we leave stderr piped, the daemon
    // inherits our pipe and `wait_with_output` blocks forever waiting
    // for EOF on stderr — which freezes the TUI. Discard stdout/stderr
    // outright, and drop the stdin handle as soon as we've written the
    // payload so the parent process sees EOF and can exit.
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("clipboard tool: no stdin"))?;
        stdin.write_all(payload.as_bytes())?;
        // explicit drop: closes the pipe so the child can exit
    }
    let status = child.wait()?;
    if !status.success() {
        return Err(std::io::Error::other(format!(
            "clipboard tool exited {status}"
        )));
    }
    Ok(())
}

/// Copy `paths` to the system clipboard. Returns the number of paths
/// written on success.
pub fn copy_paths(paths: &[PathBuf]) -> anyhow::Result<usize> {
    if paths.is_empty() {
        anyhow::bail!("no paths to copy");
    }
    let payload = build_payload(paths);

    // Wayland takes precedence over X11 when both are present (xclip
    // would talk to XWayland, but wl-copy writes the native compositor
    // clipboard which is what GTK/Qt apps actually read).
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let x11 = std::env::var_os("DISPLAY").is_some();

    if wayland {
        let mut c = Command::new("wl-copy");
        match write_to(&mut c, &payload) {
            Ok(()) => return Ok(paths.len()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // wl-copy missing — fall through to xclip
            }
            Err(e) => anyhow::bail!("wl-copy: {e}"),
        }
    }

    if x11 {
        let mut c = Command::new("xclip");
        c.args(["-selection", "clipboard", "-i"]);
        match write_to(&mut c, &payload) {
            Ok(()) => return Ok(paths.len()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                anyhow::bail!("xclip not installed");
            }
            Err(e) => anyhow::bail!("xclip: {e}"),
        }
    }

    anyhow::bail!("no display server detected (\\$WAYLAND_DISPLAY / \\$DISPLAY unset)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_is_plain_newline_joined_paths() {
        let p = build_payload(&[
            PathBuf::from("/a/b"),
            PathBuf::from("/c with space"),
        ]);
        assert_eq!(p, "/a/b\n/c with space");
    }

    #[test]
    fn payload_single_path_has_no_trailing_newline() {
        let p = build_payload(&[PathBuf::from("/home/u/x.txt")]);
        assert_eq!(p, "/home/u/x.txt");
    }

    #[test]
    fn empty_paths_errors() {
        let r = copy_paths(&[]);
        assert!(r.is_err());
    }
}
