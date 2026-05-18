//! Reusable centered-overlay geometry + list selection helper.

use ratatui::layout::Rect;

/// A centered rect `pct_w`% wide and `lines + 2` tall (content + borders),
/// clamped inside `area`.
pub fn centered_rect(pct_w: u16, lines: u16, area: Rect) -> Rect {
    let w = (area.width as u32 * pct_w as u32 / 100) as u16;
    let w = w.clamp(10, area.width);
    let h = (lines + 2).min(area.height.max(3));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect { x, y, width: w, height: h }
}

/// Move `sel` within `0..len` with wrap-around. Returns the new index.
pub fn wrap_nav(sel: usize, len: usize, down: bool) -> usize {
    if len == 0 {
        return 0;
    }
    if down {
        (sel + 1) % len
    } else {
        (sel + len - 1) % len
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    #[test]
    fn centered_is_within_and_centered() {
        let area = Rect { x: 0, y: 0, width: 100, height: 40 };
        let r = centered_rect(50, 3, area);
        assert_eq!(r.width, 50);
        assert_eq!(r.height, 5); // 3 + 2 borders
        assert_eq!(r.x, 25);
        assert_eq!(r.y, (40 - 5) / 2);
        assert!(r.x + r.width <= area.width);
        assert!(r.y + r.height <= area.height);
    }

    #[test]
    fn wrap_nav_cycles() {
        assert_eq!(wrap_nav(0, 3, false), 2);
        assert_eq!(wrap_nav(2, 3, true), 0);
        assert_eq!(wrap_nav(1, 3, true), 2);
        assert_eq!(wrap_nav(0, 0, true), 0);
    }
}
