//! Command-template expansion for the script-integration feature.
//!
//! Pure, `App`-free engine shared by every script trigger (the `!` ad-hoc
//! prompt, `[commands]` key bindings, the scripts-dir picker). The caller
//! resolves the selection and directory first and passes them in, so this
//! module is trivially unit-testable.
//!
//! Placeholders in a template string, each path-derived value SHELL-QUOTED:
//!   {paths} -> all targets, quoted, space-joined
//!   {file}  -> first target, quoted ("" if none)
//!   {dir}   -> the resolved dir, quoted
//!   {name}  -> first target's file name, quoted ("" if none)
//!   {ext}   -> first target's extension, quoted ("" if none/dotfile)
//! Unknown `{foo}` tokens pass through verbatim.
//!
//! ponytail: only these five tokens are magic, so a user can't pass a literal
//! `{paths}` string to the shell. Acceptable ceiling for a file-manager command.

use crate::keymap::Keymap;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// Validate `[commands]` against the final (post-user-override) keymap. Drops any
/// key that would shadow a builtin binding and returns a warning for each drop.
/// Pure: takes the already-built `Keymap`, so it must run AFTER `Keymap::build`.
pub fn validate_commands(
    raw: &BTreeMap<String, String>,
    keymap: &Keymap,
) -> (HashMap<String, String>, Vec<String>) {
    let mut valid = HashMap::new();
    let mut warnings = Vec::new();
    for (key, template) in raw {
        if keymap.would_shadow(key) {
            warnings.push(format!("command key '{key}' conflicts with a builtin; ignored"));
        } else {
            valid.insert(key.clone(), template.clone());
        }
    }
    (valid, warnings)
}

/// Resolved substitution inputs. `dir` is already resolved (caller falls back
/// to the tree root when there's no sensible selection dir), so `expand` never
/// deals with `Option`.
pub struct Subst {
    pub targets: Vec<PathBuf>,
    pub dir: PathBuf,
}

/// POSIX single-quote a string so the shell treats it as one literal argument.
/// Wrap in `'...'`; an embedded `'` becomes `'\''` (close, escaped quote, reopen).
/// This neutralizes spaces, `;`, `$`, `"`, `*`, and `'` itself — the whole
/// security story for path interpolation.
pub fn shell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

fn quote_path(p: &Path) -> String {
    shell_quote(&p.to_string_lossy())
}

/// List executable files directly inside `dir`, sorted by path. Flat (no
/// recursion); a missing/unreadable dir yields an empty list (not an error).
///
/// ponytail: Unix-only `+x` bit check — explorenova targets Linux. A Windows
/// port would need a different executability rule.
#[cfg(unix)]
pub fn scan_scripts_dir(dir: &Path) -> Vec<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let mut out: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
            .filter(|e| {
                e.metadata()
                    .map(|m| m.permissions().mode() & 0o111 != 0)
                    .unwrap_or(false)
            })
            .map(|e| e.path())
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort();
    out
}

/// The five known placeholders. Any token here makes a template selection-aware.
const TOKENS: [&str; 5] = ["{paths}", "{file}", "{dir}", "{name}", "{ext}"];

/// Does this template reference the selection at all? Used so a fileless command
/// (`mkdir foo`) still runs with an empty selection, while a selection-dependent
/// one (`rm {paths}`) is blocked with a "no selection" message instead of running
/// with empty args.
pub fn references_selection(template: &str) -> bool {
    TOKENS.iter().any(|t| template.contains(t))
}

/// Expand the five known placeholders in `template`. Path-derived values are
/// shell-quoted; unknown tokens are left untouched.
pub fn expand(template: &str, s: &Subst) -> String {
    let first = s.targets.first();

    let paths = s
        .targets
        .iter()
        .map(|p| quote_path(p))
        .collect::<Vec<_>>()
        .join(" ");
    let file = first.map(|p| quote_path(p)).unwrap_or_default();
    let dir = quote_path(&s.dir);
    let name = first
        .and_then(|p| p.file_name())
        .map(|n| shell_quote(&n.to_string_lossy()))
        .unwrap_or_default();
    let ext = first
        .and_then(|p| p.extension())
        .map(|e| shell_quote(&e.to_string_lossy()))
        .unwrap_or_default();

    // Sequential replace: any token not present is simply not substituted, so
    // unknown `{foo}` passes through. Order doesn't matter (tokens disjoint).
    template
        .replace("{paths}", &paths)
        .replace("{file}", &file)
        .replace("{dir}", &dir)
        .replace("{name}", &name)
        .replace("{ext}", &ext)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subst(paths: &[&str], dir: &str) -> Subst {
        Subst {
            targets: paths.iter().map(PathBuf::from).collect(),
            dir: PathBuf::from(dir),
        }
    }

    #[test]
    fn quote_wraps_and_escapes() {
        assert_eq!(shell_quote("plain"), "'plain'");
        assert_eq!(shell_quote("a b"), "'a b'");
        // Injection attempts stay inside the quotes.
        assert_eq!(shell_quote("a;rm -rf x"), "'a;rm -rf x'");
        assert_eq!(shell_quote("$(whoami)"), "'$(whoami)'");
        assert_eq!(shell_quote("a\"b"), "'a\"b'");
        // Embedded single quote: close, escaped quote, reopen.
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn paths_zero_one_many() {
        assert_eq!(expand("x {paths}", &subst(&[], "/d")), "x ");
        assert_eq!(expand("x {paths}", &subst(&["/a/b"], "/d")), "x '/a/b'");
        assert_eq!(
            expand("x {paths}", &subst(&["/a", "/b c"], "/d")),
            "x '/a' '/b c'"
        );
    }

    #[test]
    fn file_empty_when_no_targets() {
        assert_eq!(expand("e {file}", &subst(&[], "/d")), "e ");
        assert_eq!(expand("e {file}", &subst(&["/a/b.txt"], "/d")), "e '/a/b.txt'");
    }

    #[test]
    fn dir_always_quoted() {
        assert_eq!(expand("{dir}", &subst(&[], "/home/me dir")), "'/home/me dir'");
    }

    #[test]
    fn name_and_ext() {
        let s = subst(&["/a/report.tar.gz"], "/d");
        assert_eq!(expand("{name}", &s), "'report.tar.gz'");
        assert_eq!(expand("{ext}", &s), "'gz'");
    }

    #[test]
    fn ext_empty_for_dotfile_and_extensionless() {
        assert_eq!(expand("[{ext}]", &subst(&["/a/Makefile"], "/d")), "[]");
        assert_eq!(expand("[{ext}]", &subst(&["/a/.gitignore"], "/d")), "[]");
    }

    #[test]
    fn name_ext_are_quoted_against_injection() {
        // A filename crafted to inject must be neutralized through {name}.
        let s = subst(&["/tmp/a;rm -rf x"], "/d");
        assert_eq!(expand("echo {name}", &s), "echo 'a;rm -rf x'");
    }

    #[test]
    fn validate_drops_builtin_collisions_keeps_free_keys() {
        let km = Keymap::build(&BTreeMap::new());
        let mut raw = BTreeMap::new();
        raw.insert("x".to_string(), "a".to_string()); // builtin Trash
        raw.insert("g".to_string(), "b".to_string()); // prefix of gg
        raw.insert("ctrl-d".to_string(), "c".to_string()); // builtin HalfDown
        raw.insert("c".to_string(), "ok1".to_string()); // free (not a real prefix of ctrl-d)
        raw.insert("z".to_string(), "ok2".to_string()); // free
        let (valid, warnings) = validate_commands(&raw, &km);
        assert_eq!(valid.get("c"), Some(&"ok1".to_string()));
        assert_eq!(valid.get("z"), Some(&"ok2".to_string()));
        assert!(!valid.contains_key("x"));
        assert!(!valid.contains_key("g"));
        assert!(!valid.contains_key("ctrl-d"));
        assert_eq!(warnings.len(), 3);
    }

    #[test]
    fn validate_uses_post_override_keymap() {
        // User remapped quit to `Q`; a command on `Q` must now collide.
        let mut user = BTreeMap::new();
        user.insert("quit".to_string(), vec!["Q".to_string()]);
        let km = Keymap::build(&user);
        let mut raw = BTreeMap::new();
        raw.insert("Q".to_string(), "x".to_string());
        let (valid, warnings) = validate_commands(&raw, &km);
        assert!(valid.is_empty());
        assert_eq!(warnings.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn scan_returns_only_executables_sorted() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let mk = |name: &str, mode: u32| {
            let p = dir.path().join(name);
            std::fs::write(&p, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
            p
        };
        let b = mk("b.sh", 0o755);
        let a = mk("a.sh", 0o755);
        mk("plain.txt", 0o644); // not executable -> excluded
        let got = scan_scripts_dir(dir.path());
        assert_eq!(got, vec![a, b]); // sorted, only the +x files
    }

    #[cfg(unix)]
    #[test]
    fn scan_missing_dir_is_empty() {
        assert!(scan_scripts_dir(Path::new("/no/such/dir/xyz")).is_empty());
    }

    #[test]
    fn references_selection_detects_tokens() {
        assert!(references_selection("rm {paths}"));
        assert!(references_selection("x {ext} y"));
        // Fileless commands don't reference the selection -> still runnable.
        assert!(!references_selection("mkdir foo"));
        assert!(!references_selection("ls -la"));
        assert!(!references_selection("awk '{print $1}'"));
    }

    #[test]
    fn unknown_token_passes_through() {
        assert_eq!(expand("a {foo} b", &subst(&["/x"], "/d")), "a {foo} b");
    }

    #[test]
    fn identity_on_placeholder_free_strings() {
        // CRITICAL regression: guards the existing `!` ShellOnce behavior —
        // a command with no placeholders (or a stray brace) is unchanged.
        let s = subst(&["/x"], "/d");
        assert_eq!(expand("ls -la", &s), "ls -la");
        assert_eq!(expand("echo hello", &s), "echo hello");
        assert_eq!(expand("awk '{print $1}'", &s), "awk '{print $1}'");
    }
}
