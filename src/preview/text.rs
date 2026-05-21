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
        let ranges = h.highlight_line(line, ss).ok()?;
        let spans: Vec<Span<'static>> = ranges
            .into_iter()
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
    fn oversize_returns_none() {
        let d = tempdir().unwrap();
        let f = d.path().join("big.txt");
        fs::write(&f, vec![b'x'; 4096]).unwrap();
        assert!(highlighted(&f, 100).is_none());
    }
}
