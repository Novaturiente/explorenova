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

/// Keep the tail of `s`, dropping the head behind a `…`, so the result
/// fits in `max` DISPLAY CELLS. Counting cells rather than chars is what
/// keeps a CJK path (2 cells per glyph) inside its segment. Shared with
/// the tree pane's title — there used to be a second, char-counting copy
/// of this in `ui::panes_render`.
pub fn truncate_left(s: &str, max: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    if s.width() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let budget = max - 1; // the ellipsis costs one cell
    let mut tail_rev: Vec<char> = Vec::new();
    let mut used = 0usize;
    for c in s.chars().rev() {
        let cw = c.width().unwrap_or(0);
        if used + cw > budget {
            break;
        }
        tail_rev.push(c);
        used += cw;
    }
    let tail: String = tail_rev.into_iter().rev().collect();
    format!("…{tail}")
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
    // Sanitize: a control char in the path (rare, but filenames allow it)
    // would otherwise corrupt the status line the same way it would a
    // tree row.
    let path_s = crate::ui::display_name(&n.path.display().to_string());
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
    let kind = match n.kind {
        crate::tree::node::NodeKind::Dir => 'd',
        crate::tree::node::NodeKind::Symlink => 'l',
        _ => '-',
    };
    perms_string(kind, n.mode)
}

/// `drwxr-xr-x`-style string from a type char and raw mode bits.
pub fn perms_string(kind: char, mode: u32) -> String {
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

/// Seconds to add to a UTC timestamp to get local wall-clock time, as
/// libc reports it for that instant (so it follows $TZ *and* whether DST
/// was in effect then). 0 if libc can't say.
#[cfg(unix)]
fn local_offset_secs(secs: u64) -> i64 {
    let t = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // localtime_r, not gmtime_r: the whole point is the zone offset.
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return 0;
    }
    tm.tm_gmtoff as i64
}

#[cfg(not(unix))]
fn local_offset_secs(_secs: u64) -> i64 {
    0
}

/// Local `YYYY-MM-DD HH:MM` for the metadata panel.
pub fn format_datetime(t: std::time::SystemTime) -> String {
    let secs = t
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let local = secs.saturating_add_signed(local_offset_secs(secs));
    let (y, m, d) = days_to_ymd(local / 86400);
    let day = local % 86400;
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", day / 3600, day % 3600 / 60)
}

fn format_mtime(t: std::time::SystemTime) -> String {
    use std::time::UNIX_EPOCH;
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Epoch seconds are UTC; rendering them raw made an evening edit show
    // tomorrow's date east of Greenwich (and yesterday's west of it).
    let local = secs.saturating_add_signed(local_offset_secs(secs));
    let (y, m, d) = days_to_ymd(local / 86400);
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
        // Status messages embed file/dir names from op results (trash,
        // rename, paste failures, …) — sanitize the same as any other
        // filename-derived chrome text.
        crate::ui::display_name(&app.status)
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

    // Center: running op (the footer may be hidden), else marks summary.
    let center = if let Some(op) = &app.op {
        let spin = crate::ui::modal::spinner_char(op.started_at.elapsed().as_millis());
        let pct = op.progress().0.map(|p| format!(" {p}%")).unwrap_or_default();
        format!("{spin} {}{pct}", op.kind.verb())
    } else if app.marks.is_empty() {
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
        use unicode_width::UnicodeWidthStr;
        let s = "/home/nova/personal/explorer/src/app.rs";
        let t = truncate_left(s, 20);
        assert!(t.starts_with('…'));
        assert!(t.ends_with("src/app.rs"));
        assert!(t.width() <= 20);
    }

    #[test]
    fn truncate_left_counts_cells_not_chars() {
        use unicode_width::UnicodeWidthStr;
        // 10 CJK chars = 20 cells. A char-counting truncator returned all
        // ten for max=12 and overflowed the status segment by 8 columns.
        let s = "/一二三四五六七八九十";
        let t = truncate_left(s, 12);
        assert!(t.width() <= 12, "{t:?} is {} cells", t.width());
        assert!(t.ends_with('十'));
    }

    #[test]
    fn truncate_left_tiny_width_never_overflows() {
        use unicode_width::UnicodeWidthStr;
        assert!(truncate_left("/long/path", 1).width() <= 1);
        assert!(truncate_left("/long/path", 0).is_empty());
        assert!(truncate_left("/long/path", 2).width() <= 2);
    }

    #[test]
    fn truncate_left_shorter_than_max_returns_unchanged() {
        assert_eq!(truncate_left("/a/b", 20), "/a/b");
    }

    #[test]
    #[cfg(unix)]
    fn mtime_renders_in_local_time_not_utc() {
        // 1700000000 = 2023-11-14 22:13:20 UTC. At UTC+5:30 the local date
        // is already the 15th — the bug rendered the 14th.
        // ponytail: libc caches the zone on its first localtime_r call and
        // $TZ is process-global, so this sets TZ up front and stays the
        // only test in the crate that reads a wall-clock date.
        std::env::set_var("TZ", "XXX-5:30");
        let t = std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        assert_eq!(format_mtime(t), "2023-11-15");
        assert_eq!(local_offset_secs(1_700_000_000), 5 * 3600 + 1800);
    }

    #[test]
    fn days_to_ymd_known_dates() {
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
        // 2026-05-24 is 20597 days since 1970-01-01.
        assert_eq!(days_to_ymd(20597), (2026, 5, 24));
    }
}
