//! Write file paths to the system clipboard so the user can paste them
//! into browsers, other file managers, chat apps, etc. Detects Wayland
//! (`wl-copy`) vs X11 (`xclip`) at runtime and falls through cleanly
//! when neither tool is available.
//!
//! Format: newline-separated `file://` URIs. Most apps consume this as
//! either a uri-list or as plain text. Multi-MIME advertisement (e.g.
//! `x-special/gnome-copied-files`) is intentionally out of scope —
//! single representation keeps the helper portable.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// URL-encode path for use as the path portion of a file:// URI.
/// Encodes characters outside [A-Za-z0-9._~-/] as percent-escapes.
/// (Slashes survive — they delimit segments and don't need encoding.)
fn encode_path(p: &Path) -> String {
    let s = p.to_string_lossy();
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        let safe = b.is_ascii_alphanumeric()
            || matches!(b, b'.' | b'_' | b'~' | b'-' | b'/');
        if safe {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn build_payload(paths: &[PathBuf]) -> String {
    let mut s = String::new();
    for (i, p) in paths.iter().enumerate() {
        if i > 0 {
            s.push('\n');
        }
        s.push_str("file://");
        s.push_str(&encode_path(p));
    }
    s
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
        c.args(["--type", "text/uri-list"]);
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
        c.args(["-selection", "clipboard", "-t", "text/uri-list", "-i"]);
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
    fn encode_preserves_safe_chars() {
        let s = encode_path(Path::new("/home/u/Pictures/cat.png"));
        assert_eq!(s, "/home/u/Pictures/cat.png");
    }

    #[test]
    fn encode_percent_escapes_spaces_and_unicode() {
        let s = encode_path(Path::new("/tmp/My File.txt"));
        assert_eq!(s, "/tmp/My%20File.txt");

        // Unicode (em-dash) → multi-byte percent-encoded
        let s2 = encode_path(Path::new("/tmp/a—b.txt"));
        assert!(s2.starts_with("/tmp/a%E2%80%94b.txt"));
    }

    #[test]
    fn payload_joins_with_newlines_and_file_scheme() {
        let p = build_payload(&[
            PathBuf::from("/a/b"),
            PathBuf::from("/c with space"),
        ]);
        assert_eq!(p, "file:///a/b\nfile:///c%20with%20space");
    }

    #[test]
    fn empty_paths_errors() {
        let r = copy_paths(&[]);
        assert!(r.is_err());
    }
}
