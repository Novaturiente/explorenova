//! A minimal single-line caret text editor (UTF-8 safe).

#[derive(Debug, Default, Clone)]
pub struct TextInput {
    chars: Vec<char>,
    caret: usize,
}

impl TextInput {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the content; caret goes to the end.
    pub fn set(&mut self, seed: &str) {
        self.chars = seed.chars().collect();
        self.caret = self.chars.len();
    }

    pub fn clear(&mut self) {
        self.chars.clear();
        self.caret = 0;
    }

    pub fn value(&self) -> String {
        self.chars.iter().collect()
    }

    /// Caret position as a char index in `0..=len`.
    pub fn caret(&self) -> usize {
        self.caret
    }

    pub fn insert(&mut self, c: char) {
        self.chars.insert(self.caret, c);
        self.caret += 1;
    }

    pub fn backspace(&mut self) {
        if self.caret > 0 {
            self.caret -= 1;
            self.chars.remove(self.caret);
        }
    }

    pub fn delete(&mut self) {
        if self.caret < self.chars.len() {
            self.chars.remove(self.caret);
        }
    }

    pub fn left(&mut self) {
        self.caret = self.caret.saturating_sub(1);
    }

    pub fn right(&mut self) {
        if self.caret < self.chars.len() {
            self.caret += 1;
        }
    }

    pub fn home(&mut self) {
        self.caret = 0;
    }

    pub fn end(&mut self) {
        self.caret = self.chars.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_value() {
        let mut t = TextInput::new();
        t.insert('a');
        t.insert('b');
        assert_eq!(t.value(), "ab");
        assert_eq!(t.caret(), 2);
    }

    #[test]
    fn set_puts_caret_at_end_then_backspace() {
        let mut t = TextInput::new();
        t.set("main.rs");
        assert_eq!(t.caret(), 7);
        t.backspace();
        assert_eq!(t.value(), "main.r");
        assert_eq!(t.caret(), 6);
    }

    #[test]
    fn backspace_at_start_is_noop() {
        let mut t = TextInput::new();
        t.set("x");
        t.home();
        t.backspace();
        assert_eq!(t.value(), "x");
        assert_eq!(t.caret(), 0);
    }

    #[test]
    fn delete_at_end_is_noop_else_removes_at_caret() {
        let mut t = TextInput::new();
        t.set("ab");
        t.delete();
        assert_eq!(t.value(), "ab");
        t.home();
        t.delete();
        assert_eq!(t.value(), "b");
    }

    #[test]
    fn left_right_home_end_clamped() {
        let mut t = TextInput::new();
        t.set("ab");
        t.right();
        assert_eq!(t.caret(), 2);
        t.left();
        t.left();
        t.left();
        assert_eq!(t.caret(), 0);
        t.end();
        assert_eq!(t.caret(), 2);
        t.home();
        assert_eq!(t.caret(), 0);
    }

    #[test]
    fn insert_mid_caret() {
        let mut t = TextInput::new();
        t.set("ac");
        t.left();
        t.insert('b');
        assert_eq!(t.value(), "abc");
        assert_eq!(t.caret(), 2);
    }

    #[test]
    fn utf8_multibyte_safe() {
        let mut t = TextInput::new();
        t.insert('é');
        t.insert('ñ');
        assert_eq!(t.value(), "éñ");
        t.backspace();
        assert_eq!(t.value(), "é");
        assert_eq!(t.caret(), 1);
    }
}
