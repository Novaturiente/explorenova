//! Thin zoxide integration: query the database for a `z`-style jump and
//! record visited dirs so the database stays useful. Both degrade to a
//! no-op / None when the `zoxide` binary is absent.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Best matching directory for `query`, or None (no match / zoxide absent).
pub fn query(query: &str) -> Option<PathBuf> {
    let out = Command::new("zoxide")
        .arg("query")
        .arg("--")
        .arg(query)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if line.is_empty() {
        None
    } else {
        Some(PathBuf::from(line))
    }
}

/// Record a visited directory (fire-and-forget; ignores all errors).
pub fn add(path: &Path) {
    use std::process::Stdio;
    if let Ok(mut child) = Command::new("zoxide")
        .arg("add")
        .arg("--")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        // Reap off-thread: a dropped Child is never waited on, so each
        // `cd` would otherwise leave a zombie for the session lifetime.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn query_missing_binary_is_none_safe() {
        // Can't assume zoxide is installed in CI; just assert the call
        // never panics and returns an Option. (No DB match expected for a
        // random token.)
        let _ = super::query("zzz-nonexistent-token-zzz");
    }
}
