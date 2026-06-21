use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

/// Build the argv for a one-shot shell command (for testing without spawning).
pub fn shell_argv(cmd: &str) -> (String, Vec<String>) {
    let sh = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    (sh, vec!["-c".into(), cmd.into()])
}

/// Leave the TUI, run `f`, wait for a keypress, restore the TUI.
fn around_terminal<F: FnOnce() -> io::Result<()>>(f: F) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    let r = f();
    print!("\n[press Enter to return to explorenova] ");
    let _ = io::stdout().flush();
    let mut s = String::new();
    let _ = io::stdin().read_line(&mut s);
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    // Drain input that leaked while we were suspended: the dismiss Enter (which
    // read_line doesn't always fully consume under terminal keyboard protocols
    // a child like nvim leaves behind) and any keys typed during the child.
    // Otherwise the event loop replays them as actions — a trailing Enter fires
    // Toggle, re-opening the cursor file via xdg-open.
    // ponytail: zero-timeout poll loop; may swallow a keypress the user makes in
    // the same instant the TUI returns. Acceptable vs replaying a stale Enter.
    while crossterm::event::poll(std::time::Duration::from_millis(0)).unwrap_or(false) {
        let _ = crossterm::event::read();
    }
    r
}

pub fn run_once(cwd: &Path, cmd: &str) -> io::Result<()> {
    let (sh, args) = shell_argv(cmd);
    around_terminal(|| {
        Command::new(sh).args(args).current_dir(cwd).status().map(|_| ())
    })
}

/// Suspend the TUI, run `prog args` with inherited stdio (so the child can
/// prompt the user directly), wait for Enter, restore the TUI. Used for
/// `gio mount`, whose own auth prompt is the password UI.
pub fn run_interactive(prog: &str, args: &[&str]) -> io::Result<()> {
    around_terminal(|| {
        Command::new(prog)
            .args(args)
            .status()
            // Name the binary so a missing tool reads "gio: No such file…"
            // instead of a bare OS error with no context.
            .map_err(|e| io::Error::new(e.kind(), format!("{prog}: {e}")))
            .map(|_| ())
    })
}

pub fn drop_to_shell(cwd: &Path) -> io::Result<()> {
    let sh = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    around_terminal(|| Command::new(sh).current_dir(cwd).status().map(|_| ()))
}

/// Resolve the user's editor: `$VISUAL`, `$EDITOR`, else `vi`.
pub fn editor() -> String {
    std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".into())
}

/// Write `lines` to a temp file, open it in `$EDITOR` (TUI suspended),
/// then read the edited lines back. The temp file lives under `dir` so
/// editors that show the path show a sensible one.
pub fn edit_lines(dir: &Path, lines: &[String]) -> io::Result<Vec<String>> {
    use std::io::Write as _;
    let tmp = dir.join(".explorenova-bulk-rename.tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        for l in lines {
            writeln!(f, "{l}")?;
        }
    }
    let ed = editor();
    let quoted = crate::command::shell_quote(&tmp.to_string_lossy());
    let r = around_terminal(|| {
        let (sh, args) = shell_argv(&format!("{ed} {quoted}"));
        Command::new(sh).args(args).current_dir(dir).status().map(|_| ())
    });
    let out = std::fs::read_to_string(&tmp).unwrap_or_default();
    let _ = std::fs::remove_file(&tmp);
    r?;
    Ok(out.lines().map(|s| s.to_string()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_argv_wraps_in_dash_c() {
        std::env::set_var("SHELL", "/bin/bash");
        let (sh, args) = shell_argv("ls -la");
        assert_eq!(sh, "/bin/bash");
        assert_eq!(args, vec!["-c", "ls -la"]);
    }
}
