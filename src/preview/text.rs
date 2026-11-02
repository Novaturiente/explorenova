use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use std::path::Path;
use std::sync::OnceLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

fn assets() -> &'static (SyntaxSet, ThemeSet) {
    static ASSETS: OnceLock<(SyntaxSet, ThemeSet)> = OnceLock::new();
    ASSETS.get_or_init(|| {
        (
            SyntaxSet::load_defaults_newlines(),
            ThemeSet::load_defaults(),
        )
    })
}

/// Build a styled `ratatui::text::Text` from `path`, or `None` if the
/// file isn't valid UTF-8 / exceeds `max_bytes`. Each syntect range is
/// mapped to a ratatui `Span::styled` so ratatui knows the visible
/// width of each cell — fixes the historical overflow where raw ANSI
/// escapes in the rendered string spilled into adjacent panes.
pub fn highlighted(path: &Path, max_bytes: u64) -> Option<Text<'static>> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() as u64 > max_bytes {
        return None;
    }
    let content = String::from_utf8(bytes).ok()?;
    let (ss, ts) = assets();
    let syntax = path
        .extension()
        .and_then(|e| e.to_str())
        .and_then(|e| ss.find_syntax_by_extension(e))
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let theme = &ts.themes["base16-ocean.dark"];
    let mut h = HighlightLines::new(syntax, theme);
    let mut lines: Vec<Line<'static>> = Vec::new();
    for line in content.lines().take(2000) {
        // lines() only strips the \r of a trailing CRLF. A bare \r, \t,
        // ESC, or BEL inside the line reaches the terminal raw through
        // the Span and repaints other panes with file content (the
        // xlsx/archive path sanitizes; this path must too).
        // The \n goes back on: the defaults are the newline-terminated
        // grammars, where a `#` comment ends at \n. Without it the comment
        // never closed and a whole .sh/.py file rendered as one comment.
        let line = format!("{}\n", super::sanitize_plain(line));
        let ranges = h.highlight_line(&line, ss).ok()?;
        let spans: Vec<Span<'static>> = ranges
            .into_iter()
            .map(|(st, s)| (st, s.trim_end_matches('\n')))
            .filter(|(_, s)| !s.is_empty())
            .map(|(st, s)| {
                let c = st.foreground;
                Span::styled(
                    s.to_string(),
                    Style::default().fg(Color::Rgb(c.r, c.g, c.b)),
                )
            })
            .collect();
        lines.push(Line::from(spans));
    }
    Some(Text::from(lines))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn highlights_returns_lines() {
        let d = tempdir().unwrap();
        let f = d.path().join("a.txt");
        fs::write(&f, "hello\nworld").unwrap();
        let t = highlighted(&f, 1024).unwrap();
        assert_eq!(t.lines.len(), 2);
        // Spans concatenated equal the original line content.
        let l0: String = t.lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(l0.contains("hello"));
    }

    #[test]
    fn control_chars_never_reach_spans() {
        // \t jumps tab stops, \r returns the cursor to col 0, \x1b starts
        // a live ANSI escape — any of them in a Span corrupts the frame
        // (file content painted over other panes).
        let d = tempdir().unwrap();
        let f = d.path().join("raw-log.tsv");
        fs::write(&f, "a\tb\rc\x1b[31mred\x07bell\nplain").unwrap();
        let t = highlighted(&f, 1024).unwrap();
        for line in &t.lines {
            for sp in &line.spans {
                assert!(
                    !sp.content.chars().any(|c| (c as u32) < 0x20 || c == '\u{7f}'),
                    "control char leaked into span: {:?}",
                    sp.content
                );
            }
        }
    }

    #[test]
    fn hash_comment_ends_at_line_end() {
        let d = tempdir().unwrap();
        let f = d.path().join("s.sh");
        fs::write(&f, "# c\necho hi\n").unwrap();
        let t = highlighted(&f, 1024).unwrap();
        let comment = t.lines[0].spans[0].style.fg;
        let echo = t.lines[1].spans.iter().find(|s| s.content.contains("echo")).unwrap();
        assert_ne!(echo.style.fg, comment, "line after a # comment must not stay comment-coloured");
    }

    #[test]
    fn oversize_returns_none() {
        let d = tempdir().unwrap();
        let f = d.path().join("big.txt");
        fs::write(&f, vec![b'x'; 4096]).unwrap();
        assert!(highlighted(&f, 100).is_none());
    }
}
