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
        Command::new(prog).args(args).status().map(|_| ())
    })
}

pub fn drop_to_shell(cwd: &Path) -> io::Result<()> {
    let sh = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    around_terminal(|| Command::new(sh).current_dir(cwd).status().map(|_| ()))
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
