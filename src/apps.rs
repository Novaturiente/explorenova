//! freedesktop `.desktop` discovery + MIME ranking for the open-with picker.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct AppEntry {
    pub name: String,
    /// Raw `Exec=` value (field codes not yet expanded).
    pub exec: String,
    pub terminal: bool,
}

/// Application directories, user dir first (so it overrides system).
pub fn app_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".local/share")));
    if let Some(d) = data_home {
        dirs.push(d.join("applications"));
    }
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    for d in data_dirs.split(':').filter(|s| !s.is_empty()) {
        dirs.push(PathBuf::from(d).join("applications"));
    }
    dirs
}

/// Parse one `.desktop` file's `[Desktop Entry]` group.
/// Returns (AppEntry, mime_types) or None if it must be skipped.
pub fn parse_desktop(contents: &str) -> Option<(AppEntry, Vec<String>)> {
    let mut in_group = false;
    let mut name = None;
    let mut exec = None;
    let mut mimes: Vec<String> = Vec::new();
    let mut typ = String::from("Application");
    let mut no_display = false;
    let mut hidden = false;
    let mut terminal = false;
    for raw in contents.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            in_group = line == "[Desktop Entry]";
            continue;
        }
        if !in_group || line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (k, v) = match line.split_once('=') {
            Some(kv) => (kv.0.trim(), kv.1.trim()),
            None => continue,
        };
        match k {
            "Name" if name.is_none() => name = Some(v.to_string()),
            "Exec" => exec = Some(v.to_string()),
            "Type" => typ = v.to_string(),
            "NoDisplay" => no_display = v == "true",
            "Hidden" => hidden = v == "true",
            "Terminal" => terminal = v == "true",
            "MimeType" => {
                mimes = v
                    .split(';')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_ascii_lowercase())
                    .collect();
            }
            _ => {}
        }
    }
    if typ != "Application" || no_display || hidden {
        return None;
    }
    Some((
        AppEntry {
            name: name?,
            exec: exec?,
            terminal,
        },
        mimes,
    ))
}

/// Discover apps for `path`, MIME-ranked. Always non-panicking.
pub fn apps_for(path: &Path) -> Vec<AppEntry> {
    let mime = mime_guess::from_path(path)
        .first()
        .map(|m| m.essence_str().to_ascii_lowercase());
    let mut seen = std::collections::HashSet::new();
    let mut matched: Vec<AppEntry> = Vec::new();
    let mut rest: Vec<AppEntry> = Vec::new();
    for dir in app_dirs() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for ent in rd.filter_map(Result::ok) {
            let p = ent.path();
            if p.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let id = match p.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            if !seen.insert(id) {
                continue; // earlier dir (user) wins
            }
            let contents = match std::fs::read_to_string(&p) {
                Ok(c) => c,
                Err(_) => continue,
            };
            if let Some((entry, mimes)) = parse_desktop(&contents) {
                let is_match = mime
                    .as_ref()
                    .map(|m| mimes.iter().any(|x| x == m))
                    .unwrap_or(false);
                if is_match {
                    matched.push(entry);
                } else {
                    rest.push(entry);
                }
            }
        }
    }
    let by_name =
        |a: &AppEntry, b: &AppEntry| a.name.to_lowercase().cmp(&b.name.to_lowercase());
    matched.sort_by(by_name);
    rest.sort_by(by_name);
    matched.extend(rest);
    matched
}

/// Build the argv to launch `app` on `file`. Strips `%i %c %k`, replaces
/// `%f %F %u %U` with the file path, splits the remainder on whitespace.
/// If `app.terminal`, wraps as `$TERM -e <argv>` (falls back to direct).
/// Resolve a real terminal-emulator binary: `$TERMINAL` if set, else the
/// first known emulator found on `$PATH`. (`$TERM` is a terminfo name, not
/// an executable, so it must NOT be used here.)
fn terminal_cmd() -> Option<String> {
    if let Ok(t) = std::env::var("TERMINAL") {
        if !t.is_empty() {
            return Some(t);
        }
    }
    let path = std::env::var("PATH").unwrap_or_default();
    let candidates = [
        "x-terminal-emulator",
        "kitty",
        "alacritty",
        "ghostty",
        "wezterm",
        "foot",
        "gnome-terminal",
        "konsole",
        "xterm",
    ];
    for cand in candidates {
        for dir in path.split(':').filter(|s| !s.is_empty()) {
            if std::path::Path::new(dir).join(cand).is_file() {
                return Some(cand.to_string());
            }
        }
    }
    None
}

/// The substituted argv *without* any terminal wrapping — the bare program
/// and its file argument. Terminal apps run this in-place (TUI suspended).
pub fn argv_bare(app: &AppEntry, file: &Path) -> Vec<String> {
    let fpath = file.to_string_lossy().to_string();
    let mut out: Vec<String> = Vec::new();
    for tok in app.exec.split_whitespace() {
        match tok {
            "%f" | "%F" | "%u" | "%U" => out.push(fpath.clone()),
            "%i" | "%c" | "%k" => {}
            _ => out.push(tok.to_string()),
        }
    }
    if !app.exec.contains('%') {
        out.push(fpath.clone());
    }
    out
}

pub fn argv(app: &AppEntry, file: &Path) -> Vec<String> {
    let out = argv_bare(app, file);
    if app.terminal {
        if let Some(term) = terminal_cmd() {
            let mut wrapped = vec![term, "-e".to_string()];
            wrapped.extend(out);
            return wrapped;
        }
    }
    out
}

/// Spawn `app` on `file`, detached. Errors bubble up as io::Error.
pub fn spawn(app: &AppEntry, file: &Path) -> std::io::Result<()> {
    let v = argv(app, file);
    if v.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "empty exec",
        ));
    }
    // Detach stdio: a GUI app inherits the TUI's terminal otherwise, and any
    // diagnostics it writes (e.g. a relaunched terminal emulator's startup
    // logs) corrupt the explorenova frame.
    let mut child = std::process::Command::new(&v[0])
        .args(&v[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    // Reap off-thread so the exited app doesn't linger as a zombie for
    // the rest of the session (a dropped Child is never waited on).
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIREFOX: &str = "[Desktop Entry]\nType=Application\nName=Firefox\nExec=firefox %u\nMimeType=text/html;image/png;\n";
    const HIDDEN: &str = "[Desktop Entry]\nType=Application\nName=Sneaky\nExec=sneaky\nNoDisplay=true\n";
    const LINK: &str = "[Desktop Entry]\nType=Link\nName=Bookmark\nURL=http://x\n";
    const TERM_APP: &str =
        "[Desktop Entry]\nType=Application\nName=Vim\nExec=vim %F\nTerminal=true\n";

    #[test]
    fn parses_name_exec_mime() {
        let (e, m) = parse_desktop(FIREFOX).unwrap();
        assert_eq!(e.name, "Firefox");
        assert_eq!(e.exec, "firefox %u");
        assert!(m.contains(&"image/png".to_string()));
        assert!(!e.terminal);
    }

    #[test]
    fn skips_nodisplay_and_non_application() {
        assert!(parse_desktop(HIDDEN).is_none());
        assert!(parse_desktop(LINK).is_none());
    }

    #[test]
    fn terminal_flag_detected() {
        let (e, _) = parse_desktop(TERM_APP).unwrap();
        assert!(e.terminal);
    }

    #[test]
    fn argv_replaces_field_codes_and_appends_when_absent() {
        let a = AppEntry {
            name: "X".into(),
            exec: "tool %U --flag".into(),
            terminal: false,
        };
        let v = argv(&a, Path::new("/tmp/a.txt"));
        assert_eq!(v, vec!["tool", "/tmp/a.txt", "--flag"]);

        let b = AppEntry {
            name: "Y".into(),
            exec: "viewer".into(),
            terminal: false,
        };
        assert_eq!(
            argv(&b, Path::new("/tmp/p.png")),
            vec!["viewer", "/tmp/p.png"]
        );
    }

    #[test]
    fn argv_wraps_terminal_app_via_terminal_env() {
        std::env::set_var("TERMINAL", "fake_term");
        let a = AppEntry {
            name: "Vim".into(),
            exec: "vim %F".into(),
            terminal: true,
        };
        let v = argv(&a, Path::new("/tmp/x.rs"));
        assert_eq!(v, vec!["fake_term", "-e", "vim", "/tmp/x.rs"]);
        std::env::remove_var("TERMINAL");
    }
}
