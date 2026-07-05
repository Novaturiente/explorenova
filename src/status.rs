use crate::app::App;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::path::Path;
use std::time::Duration;

pub fn human_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "K", "M", "G", "T"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{}{}", n, UNITS[0])
    } else if v >= 10.0 {
        format!("{:.0}{}", v, UNITS[i])
    } else {
        format!("{:.1}{}", v, UNITS[i])
    }
}

pub fn truncate_left(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    // Below 2 cols the "…/" prefix wouldn't fit; return the bare tail so
    // the result never exceeds `max`.
    if max < 2 {
        let total = s.chars().count();
        return s.chars().skip(total.saturating_sub(max)).collect();
    }
    let keep = max.saturating_sub(2);
    // Byte-safe truncation: skip from the start by char count.
    let mut t = String::with_capacity(max + 2);
    t.push_str("…/");
    let total = s.chars().count();
    let skip = total.saturating_sub(keep);
    let tail: String = s.chars().skip(skip).collect();
    t.push_str(&tail);
    t
}

#[cfg(unix)]
pub fn disk_free_bytes(path: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut buf: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c.as_ptr(), &mut buf) };
    if rc != 0 {
        return None;
    }
    // f_bavail/f_frsize widths vary by libc; keep explicit casts for
    // portability across unix targets.
    #[allow(clippy::unnecessary_cast)]
    Some(buf.f_bavail as u64 * buf.f_frsize as u64)
}

#[cfg(not(unix))]
pub fn disk_free_bytes(_path: &Path) -> Option<u64> {
    None
}

fn cursor_meta(app: &App) -> (String, String, String, String) {
    let Some(&id) = app.visible.get(app.selected) else {
        return ("".into(), "".into(), "".into(), "".into());
    };
    let Some(n) = app.arena.nodes.get(id) else {
        return ("".into(), "".into(), "".into(), "".into());
    };
    let path_s = n.path.display().to_string();
    let perms = format_perms(n);
    let size = if n.is_dir() {
        "-".into()
    } else {
        human_bytes(n.size)
    };
    let mtime = format_mtime(n.mtime);
    (path_s, perms, size, mtime)
}

#[cfg(unix)]
fn format_perms(n: &crate::tree::node::Node) -> String {
    let mode = n.mode;
    let kind = match n.kind {
        crate::tree::node::NodeKind::Dir => 'd',
        crate::tree::node::NodeKind::Symlink => 'l',
        _ => '-',
    };
    let bits = |s: u32| {
        let r = if (mode >> s) & 0o4 != 0 { 'r' } else { '-' };
        let w = if (mode >> s) & 0o2 != 0 { 'w' } else { '-' };
        let x = if (mode >> s) & 0o1 != 0 { 'x' } else { '-' };
        format!("{r}{w}{x}")
    };
    format!("{kind}{}{}{}", bits(6), bits(3), bits(0))
}

#[cfg(not(unix))]
fn format_perms(_n: &crate::tree::node::Node) -> String {
    "".into()
}

fn format_mtime(t: std::time::SystemTime) -> String {
    use std::time::UNIX_EPOCH;
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d) = days_to_ymd(secs / 86400);
    format!("{:04}-{:02}-{:02}", y, m, d)
}

// Civil-from-days (Howard Hinnant). Pure integer date conversion.
fn days_to_ymd(days_since_epoch: u64) -> (i32, u32, u32) {
    let z = days_since_epoch as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

pub fn render(area: Rect, frame: &mut Frame, app: &App) {
    use ratatui::style::Color;
    use ratatui::text::Line;

    let segs = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(60),
            Constraint::Percentage(20),
            Constraint::Percentage(20),
        ])
        .split(area);

    // Left: VISUAL flag (if any) + transient status (<=3s) or cursor meta
    let mut left_spans: Vec<Span> = Vec::new();
    if app.visual_mode {
        left_spans.push(Span::styled(
            "-- VISUAL --  ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }
    // Errors don't fade on the timer — they stay until the next keypress
    // (handle_key clears them), so a failure can't vanish unread.
    let showing_status = !app.status.is_empty()
        && (app.status_err || app.status_set_at.elapsed() < Duration::from_secs(3));
    let body = if showing_status {
        app.status.clone()
    } else {
        let (p, perms, size, mt) = cursor_meta(app);
        let max = segs[0].width.saturating_sub(
            left_spans.iter().map(|s| s.content.chars().count()).sum::<usize>() as u16,
        ) as usize;
        let path_max = max
            .saturating_sub(perms.len() + size.len() + mt.len() + 6)
            .max(8);
        format!("{}  {}  {}  {}", truncate_left(&p, path_max), perms, size, mt)
    };
    // Error status text paints red + bold; everything else uses default fg.
    if showing_status && app.status_err {
        left_spans.push(Span::styled(
            body,
            Style::default()
                .fg(app.theme.git_conflict)
                .add_modifier(Modifier::BOLD),
        ));
    } else {
        left_spans.push(Span::raw(body));
    }
    frame.render_widget(Paragraph::new(Line::from(left_spans)), segs[0]);

    // Center: marks summary (or blank)
    let center = if app.marks.is_empty() {
        String::new()
    } else {
        format!("{} sel • {}", app.marks.len(), human_bytes(app.marks_bytes()))
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            center,
            Style::default().add_modifier(Modifier::BOLD),
        )),
        segs[1],
    );

    // Right: pending chord (showcmd) + items • free
    let items = app.visible.len();
    let free = app
        .disk_free
        .as_ref()
        .map(|(_, b, _)| human_bytes(*b))
        .unwrap_or_else(|| "?".into());
    let mut right_spans: Vec<Span> = Vec::new();
    if !app.pending_chord.is_empty() {
        right_spans.push(Span::styled(
            format!("{}  ", app.pending_chord),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }
    right_spans.push(Span::raw(format!("{} items • {} free", items, free)));
    frame.render_widget(Paragraph::new(Line::from(right_spans)), segs[2]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_bytes_boundaries() {
        assert_eq!(human_bytes(0), "0B");
        assert_eq!(human_bytes(1023), "1023B");
        assert_eq!(human_bytes(1024), "1.0K");
        assert_eq!(human_bytes(10 * 1024), "10K");
        assert_eq!(human_bytes(1024 * 1024), "1.0M");
        assert_eq!(human_bytes(50 * 1024 * 1024 * 1024), "50G");
    }

    #[test]
    fn truncate_left_keeps_tail() {
        let s = "/home/nova/personal/explorer/src/app.rs";
        let t = truncate_left(s, 20);
        assert!(t.starts_with("…/"));
        assert!(t.ends_with("src/app.rs"));
        assert!(t.chars().count() <= 20);
    }

    #[test]
    fn truncate_left_tiny_width_never_overflows() {
        // Below 2 cols there's no room for "…/"; result must still fit.
        assert!(truncate_left("/long/path", 1).chars().count() <= 1);
        assert!(truncate_left("/long/path", 0).is_empty());
        assert!(truncate_left("/long/path", 2).chars().count() <= 2);
    }

    #[test]
    fn truncate_left_shorter_than_max_returns_unchanged() {
        assert_eq!(truncate_left("/a/b", 20), "/a/b");
    }

    #[test]
    fn days_to_ymd_known_dates() {
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
        // 2026-05-24 is 20597 days since 1970-01-01.
        assert_eq!(days_to_ymd(20597), (2026, 5, 24));
    }
}
