use std::path::Path;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::as_24_bit_terminal_escaped;

/// Returns ANSI-highlighted text, or None if the file is not valid UTF-8.
pub fn highlighted(path: &Path, max_bytes: u64) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() as u64 > max_bytes {
        return None;
    }
    let content = String::from_utf8(bytes).ok()?;
    let ss = SyntaxSet::load_defaults_newlines();
    let ts = ThemeSet::load_defaults();
    let syntax = path
        .extension()
        .and_then(|e| e.to_str())
        .and_then(|e| ss.find_syntax_by_extension(e))
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let theme = &ts.themes["base16-ocean.dark"];
    let mut h = HighlightLines::new(syntax, theme);
    let mut out = String::new();
    for line in content.lines().take(2000) {
        let ranges = h.highlight_line(line, &ss).ok()?;
        out.push_str(&as_24_bit_terminal_escaped(&ranges[..], false));
        out.push('\n');
    }
    out.push_str("\x1b[0m");
    Some(out)
}
