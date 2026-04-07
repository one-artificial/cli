/// Manages the text input line at the bottom of the TUI.
/// Supports optional vim keybindings (normal/insert mode) and input history.
pub struct InputState {
    buffer: String,
    cursor: usize,
    /// Temporary placeholder shown instead of input (e.g. "Press Ctrl+C again to exit")
    pub placeholder: Option<String>,
    /// Vim mode state (None = vim disabled)
    pub vim: Option<VimState>,
    /// Previous inputs for up/down arrow recall
    history: Vec<String>,
    /// Current position in history (None = current input, Some(idx) = browsing)
    pub(crate) history_index: Option<usize>,
    /// Saved current input while browsing history
    history_draft: String,
}

/// Vim mode state for the input widget.
#[derive(Debug, Clone)]
pub struct VimState {
    pub mode: VimMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimMode {
    Normal,
    Insert,
}

impl InputState {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            cursor: 0,
            placeholder: None,
            vim: None,
            history: Vec::new(),
            history_index: None,
            history_draft: String::new(),
        }
    }

    /// Enable vim keybindings.
    pub fn enable_vim(&mut self) {
        self.vim = Some(VimState {
            mode: VimMode::Insert, // Start in insert mode (natural for chat)
        });
    }

    /// Whether vim mode is active and in normal mode.
    pub fn is_vim_normal(&self) -> bool {
        self.vim
            .as_ref()
            .map(|v| v.mode == VimMode::Normal)
            .unwrap_or(false)
    }

    /// Get current vim mode label for status display.
    pub fn vim_mode_label(&self) -> Option<&'static str> {
        self.vim.as_ref().map(|v| match v.mode {
            VimMode::Normal => "NORMAL",
            VimMode::Insert => "INSERT",
        })
    }

    pub fn value(&self) -> &str {
        &self.buffer
    }

    pub fn set_placeholder(&mut self, text: &str) {
        self.placeholder = Some(text.to_string());
    }

    pub fn clear_placeholder(&mut self) {
        self.placeholder = None;
    }

    pub fn cursor_position(&self) -> usize {
        self.cursor
    }

    pub fn insert(&mut self, c: char) {
        self.buffer.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            let prev = self.buffer[..self.cursor]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.buffer.remove(prev);
            self.cursor = prev;
        }
    }

    pub fn delete_char(&mut self) {
        if self.cursor < self.buffer.len() {
            self.buffer.remove(self.cursor);
        }
    }

    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor = self.buffer[..self.cursor]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
        }
    }

    pub fn move_right(&mut self) {
        if self.cursor < self.buffer.len() {
            self.cursor = self.buffer[self.cursor..]
                .char_indices()
                .nth(1)
                .map(|(i, _)| self.cursor + i)
                .unwrap_or(self.buffer.len());
        }
    }

    pub fn move_word_forward(&mut self) {
        // Move to start of next word
        let rest = &self.buffer[self.cursor..];
        // Skip current word chars
        let skip_word = rest
            .find(|c: char| !c.is_alphanumeric() && c != '_')
            .unwrap_or(rest.len());
        // Skip whitespace
        let after_word = &rest[skip_word..];
        let skip_space = after_word
            .find(|c: char| c.is_alphanumeric() || c == '_')
            .unwrap_or(after_word.len());
        self.cursor = (self.cursor + skip_word + skip_space).min(self.buffer.len());
    }

    pub fn move_word_backward(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let before = &self.buffer[..self.cursor];
        // Skip trailing whitespace
        let trimmed = before.trim_end();
        if trimmed.is_empty() {
            self.cursor = 0;
            return;
        }
        // Find start of current word
        let word_start = trimmed
            .rfind(|c: char| !c.is_alphanumeric() && c != '_')
            .map(|i| i + 1)
            .unwrap_or(0);
        self.cursor = word_start;
    }

    pub fn move_to_start(&mut self) {
        self.cursor = 0;
    }

    pub fn move_to_end(&mut self) {
        self.cursor = self.buffer.len();
    }

    pub fn delete_line(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
    }

    /// Delete from cursor to end of line (Ctrl+K / vim D).
    pub fn kill_to_end(&mut self) {
        self.buffer.truncate(self.cursor);
    }

    /// Delete the word before the cursor (Ctrl+W).
    pub fn delete_word_backward(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let before = &self.buffer[..self.cursor];
        // Skip trailing whitespace
        let trimmed = before.trim_end();
        if trimmed.is_empty() {
            self.buffer = self.buffer[self.cursor..].to_string();
            self.cursor = 0;
            return;
        }
        // Find start of current word
        let word_start = trimmed
            .rfind(|c: char| !c.is_alphanumeric() && c != '_')
            .map(|i| i + 1)
            .unwrap_or(0);
        let after = self.buffer[self.cursor..].to_string();
        self.buffer = format!("{}{}", &self.buffer[..word_start], after);
        self.cursor = word_start;
    }

    /// Handle a vim normal mode key. Returns true if the key was consumed.
    pub fn handle_vim_normal(&mut self, c: char) -> bool {
        if !self.is_vim_normal() {
            return false;
        }

        match c {
            'i' => {
                self.vim.as_mut().unwrap().mode = VimMode::Insert;
                true
            }
            'a' => {
                self.move_right();
                self.vim.as_mut().unwrap().mode = VimMode::Insert;
                true
            }
            'A' => {
                self.move_to_end();
                self.vim.as_mut().unwrap().mode = VimMode::Insert;
                true
            }
            'I' => {
                self.move_to_start();
                self.vim.as_mut().unwrap().mode = VimMode::Insert;
                true
            }
            'h' => {
                self.move_left();
                true
            }
            'l' => {
                self.move_right();
                true
            }
            'w' => {
                self.move_word_forward();
                true
            }
            'b' => {
                self.move_word_backward();
                true
            }
            '0' => {
                self.move_to_start();
                true
            }
            '$' => {
                self.move_to_end();
                true
            }
            'x' => {
                self.delete_char();
                true
            }
            'D' => {
                self.buffer.truncate(self.cursor);
                true
            }
            'C' => {
                self.buffer.truncate(self.cursor);
                self.vim.as_mut().unwrap().mode = VimMode::Insert;
                true
            }
            _ => false,
        }
    }

    /// Handle vim escape (enter normal mode).
    pub fn handle_vim_escape(&mut self) -> bool {
        if let Some(ref mut vim) = self.vim
            && vim.mode == VimMode::Insert
        {
            vim.mode = VimMode::Normal;
            // Move cursor back one (vim behavior)
            self.move_left();
            return true;
        }
        false
    }

    /// Replace the entire buffer and move cursor to end.
    pub fn set_value(&mut self, value: String) {
        self.cursor = value.len();
        self.buffer = value;
    }

    /// Navigate up in input history. Returns true if handled.
    pub fn history_up(&mut self) -> bool {
        if self.history.is_empty() {
            return false;
        }

        match self.history_index {
            None => {
                // Save current input as draft and show most recent history
                self.history_draft = self.buffer.clone();
                self.history_index = Some(self.history.len() - 1);
            }
            Some(idx) if idx > 0 => {
                self.history_index = Some(idx - 1);
            }
            _ => return false, // Already at oldest
        }

        if let Some(idx) = self.history_index {
            self.buffer = self.history[idx].clone();
            self.cursor = self.buffer.len();
        }
        true
    }

    /// Navigate down in input history. Returns true if handled.
    pub fn history_down(&mut self) -> bool {
        match self.history_index {
            None => return false, // Not browsing history
            Some(idx) => {
                if idx + 1 < self.history.len() {
                    self.history_index = Some(idx + 1);
                    self.buffer = self.history[idx + 1].clone();
                    self.cursor = self.buffer.len();
                } else {
                    // Return to draft
                    self.history_index = None;
                    self.buffer = std::mem::take(&mut self.history_draft);
                    self.cursor = self.buffer.len();
                }
            }
        }
        true
    }

    /// Submits the current buffer and resets. Returns None if empty.
    pub fn submit(&mut self) -> Option<String> {
        if self.buffer.trim().is_empty() {
            return None;
        }
        let msg = std::mem::take(&mut self.buffer);
        self.cursor = 0;
        // Reset history browsing
        self.history_index = None;
        self.history_draft.clear();
        // Add to history (skip duplicates of the last entry)
        if self.history.last().map(|h| h.as_str()) != Some(&msg) {
            self.history.push(msg.clone());
            // Cap history at 100 entries
            if self.history.len() > 100 {
                self.history.remove(0);
            }
        }
        // Reset to insert mode on submit
        if let Some(ref mut vim) = self.vim {
            vim.mode = VimMode::Insert;
        }
        Some(msg)
    }
}

impl Default for InputState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_input() {
        let mut input = InputState::new();
        input.insert('h');
        input.insert('i');
        assert_eq!(input.value(), "hi");
        assert_eq!(input.cursor_position(), 2);
    }

    #[test]
    fn test_vim_mode_toggle() {
        let mut input = InputState::new();
        input.enable_vim();
        assert!(!input.is_vim_normal()); // Starts in insert

        input.handle_vim_escape();
        assert!(input.is_vim_normal());
        assert_eq!(input.vim_mode_label(), Some("NORMAL"));

        input.handle_vim_normal('i');
        assert!(!input.is_vim_normal());
        assert_eq!(input.vim_mode_label(), Some("INSERT"));
    }

    #[test]
    fn test_vim_movement() {
        let mut input = InputState::new();
        input.enable_vim();
        input.set_value("hello world".to_string());
        input.handle_vim_escape();

        // Move to start
        input.handle_vim_normal('0');
        assert_eq!(input.cursor_position(), 0);

        // Move to end
        input.handle_vim_normal('$');
        assert_eq!(input.cursor_position(), 11);

        // Word forward from start
        input.handle_vim_normal('0');
        input.handle_vim_normal('w');
        assert!(input.cursor_position() > 0); // moved past "hello"
    }

    #[test]
    fn test_vim_delete() {
        let mut input = InputState::new();
        input.enable_vim();
        input.set_value("hello".to_string());
        input.handle_vim_escape();

        input.handle_vim_normal('0');
        input.handle_vim_normal('x');
        assert_eq!(input.value(), "ello");
    }

    #[test]
    fn test_history_basic() {
        let mut input = InputState::new();

        // Submit some messages
        input.set_value("first".to_string());
        assert_eq!(input.submit(), Some("first".to_string()));

        input.set_value("second".to_string());
        assert_eq!(input.submit(), Some("second".to_string()));

        input.set_value("third".to_string());
        assert_eq!(input.submit(), Some("third".to_string()));

        // Now browse history with Up
        assert!(input.history_up()); // → "third"
        assert_eq!(input.value(), "third");

        assert!(input.history_up()); // → "second"
        assert_eq!(input.value(), "second");

        assert!(input.history_up()); // → "first"
        assert_eq!(input.value(), "first");

        // Can't go further up
        assert!(!input.history_up());

        // Go back down
        assert!(input.history_down()); // → "second"
        assert_eq!(input.value(), "second");

        assert!(input.history_down()); // → "third"
        assert_eq!(input.value(), "third");

        assert!(input.history_down()); // → empty draft
        assert_eq!(input.value(), "");
    }

    #[test]
    fn test_history_preserves_draft() {
        let mut input = InputState::new();

        input.set_value("past".to_string());
        input.submit();

        // Start typing something new
        input.insert('n');
        input.insert('e');
        input.insert('w');
        assert_eq!(input.value(), "new");

        // Go up into history
        input.history_up();
        assert_eq!(input.value(), "past");

        // Come back down — draft is preserved
        input.history_down();
        assert_eq!(input.value(), "new");
    }

    #[test]
    fn test_history_no_duplicates() {
        let mut input = InputState::new();

        input.set_value("same".to_string());
        input.submit();
        input.set_value("same".to_string());
        input.submit();

        // Only one entry (deduplicated)
        input.history_up();
        assert_eq!(input.value(), "same");
        assert!(!input.history_up()); // Can't go further
    }
}
