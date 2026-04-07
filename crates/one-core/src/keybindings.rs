//! Keybinding customization — load key mappings from keybindings.json.
//!
//! Supports loading from:
//! - ~/.one/keybindings.json (user-level)
//! - ~/.claude/keybindings.json (CC-compatible)
//!
//! Actions: submit, newline, cancel, clear, scroll_up, scroll_down,
//! tab_next, tab_prev, interrupt, history_prev, history_next.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A keybinding action the user can trigger.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    Submit,
    Newline,
    Cancel,
    Clear,
    ScrollUp,
    ScrollDown,
    TabNext,
    TabPrev,
    Interrupt,
    HistoryPrev,
    HistoryNext,
    Autocomplete,
}

/// A key combination: modifier + key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyCombo {
    pub key: String,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub shift: bool,
}

impl KeyCombo {
    /// Parse a string like "ctrl+enter", "alt+j", "shift+tab" into a KeyCombo.
    pub fn parse(s: &str) -> Self {
        let parts: Vec<&str> = s.split('+').collect();
        let mut combo = KeyCombo {
            key: String::new(),
            ctrl: false,
            alt: false,
            shift: false,
        };

        for part in &parts {
            match part.to_lowercase().as_str() {
                "ctrl" | "control" => combo.ctrl = true,
                "alt" | "meta" | "option" => combo.alt = true,
                "shift" => combo.shift = true,
                other => combo.key = other.to_string(),
            }
        }

        combo
    }

    /// Check if this combo matches a crossterm KeyEvent.
    pub fn matches_crossterm(
        &self,
        code: crossterm::event::KeyCode,
        modifiers: crossterm::event::KeyModifiers,
    ) -> bool {
        use crossterm::event::{KeyCode, KeyModifiers};

        let ctrl_match = self.ctrl == modifiers.contains(KeyModifiers::CONTROL);
        let alt_match = self.alt == modifiers.contains(KeyModifiers::ALT);
        let shift_match = self.shift == modifiers.contains(KeyModifiers::SHIFT);

        if !ctrl_match || !alt_match || !shift_match {
            return false;
        }

        match code {
            KeyCode::Enter => self.key == "enter",
            KeyCode::Esc => self.key == "escape" || self.key == "esc",
            KeyCode::Tab => self.key == "tab",
            KeyCode::BackTab => self.key == "tab" && self.shift,
            KeyCode::Up => self.key == "up",
            KeyCode::Down => self.key == "down",
            KeyCode::Left => self.key == "left",
            KeyCode::Right => self.key == "right",
            KeyCode::PageUp => self.key == "pageup",
            KeyCode::PageDown => self.key == "pagedown",
            KeyCode::Home => self.key == "home",
            KeyCode::End => self.key == "end",
            KeyCode::Delete => self.key == "delete",
            KeyCode::Backspace => self.key == "backspace",
            KeyCode::Char(c) => self.key == c.to_string(),
            _ => false,
        }
    }
}

/// Keybinding configuration.
#[derive(Debug, Clone, Default)]
pub struct KeybindingConfig {
    bindings: HashMap<Action, Vec<KeyCombo>>,
}

impl KeybindingConfig {
    /// Load keybindings with defaults, optionally overridden from file.
    pub fn load() -> Self {
        let mut config = Self::defaults();

        // Try loading from ~/.one/keybindings.json
        if let Some(home) = dirs_next::home_dir() {
            let one_path = home.join(".one").join("keybindings.json");
            if let Some(overrides) = load_keybindings_file(&one_path) {
                config.apply_overrides(&overrides);
            }

            // CC-compatible: ~/.claude/keybindings.json
            let cc_path = home.join(".claude").join("keybindings.json");
            if let Some(overrides) = load_keybindings_file(&cc_path) {
                config.apply_overrides(&overrides);
            }
        }

        config
    }

    /// Default keybindings matching CC's defaults.
    pub fn defaults() -> Self {
        let mut bindings = HashMap::new();

        bindings.insert(Action::Submit, vec![KeyCombo::parse("enter")]);
        bindings.insert(Action::Newline, vec![KeyCombo::parse("shift+enter")]);
        bindings.insert(Action::Cancel, vec![KeyCombo::parse("ctrl+c")]);
        bindings.insert(Action::Clear, vec![KeyCombo::parse("ctrl+l")]);
        bindings.insert(Action::ScrollUp, vec![KeyCombo::parse("pageup")]);
        bindings.insert(Action::ScrollDown, vec![KeyCombo::parse("pagedown")]);
        bindings.insert(Action::TabNext, vec![KeyCombo::parse("ctrl+right")]);
        bindings.insert(Action::TabPrev, vec![KeyCombo::parse("ctrl+left")]);
        bindings.insert(Action::Interrupt, vec![KeyCombo::parse("escape")]);
        bindings.insert(Action::HistoryPrev, vec![KeyCombo::parse("up")]);
        bindings.insert(Action::HistoryNext, vec![KeyCombo::parse("down")]);
        bindings.insert(Action::Autocomplete, vec![KeyCombo::parse("tab")]);

        Self { bindings }
    }

    /// Check if a key event matches an action.
    pub fn matches(
        &self,
        action: &Action,
        code: crossterm::event::KeyCode,
        modifiers: crossterm::event::KeyModifiers,
    ) -> bool {
        self.bindings
            .get(action)
            .map(|combos| combos.iter().any(|c| c.matches_crossterm(code, modifiers)))
            .unwrap_or(false)
    }

    fn apply_overrides(&mut self, overrides: &HashMap<String, Vec<String>>) {
        for (action_name, key_strings) in overrides {
            let action = match action_name.as_str() {
                "submit" => Action::Submit,
                "newline" => Action::Newline,
                "cancel" => Action::Cancel,
                "clear" => Action::Clear,
                "scrollUp" | "scroll_up" => Action::ScrollUp,
                "scrollDown" | "scroll_down" => Action::ScrollDown,
                "tabNext" | "tab_next" => Action::TabNext,
                "tabPrev" | "tab_prev" => Action::TabPrev,
                "interrupt" => Action::Interrupt,
                "historyPrev" | "history_prev" => Action::HistoryPrev,
                "historyNext" | "history_next" => Action::HistoryNext,
                "autocomplete" => Action::Autocomplete,
                _ => continue,
            };

            let combos: Vec<KeyCombo> = key_strings.iter().map(|s| KeyCombo::parse(s)).collect();
            if !combos.is_empty() {
                self.bindings.insert(action, combos);
            }
        }
    }
}

fn load_keybindings_file(path: &PathBuf) -> Option<HashMap<String, Vec<String>>> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    #[test]
    fn test_parse_simple_key() {
        let combo = KeyCombo::parse("enter");
        assert_eq!(combo.key, "enter");
        assert!(!combo.ctrl);
        assert!(!combo.alt);
    }

    #[test]
    fn test_parse_combo() {
        let combo = KeyCombo::parse("ctrl+c");
        assert_eq!(combo.key, "c");
        assert!(combo.ctrl);
        assert!(!combo.alt);
    }

    #[test]
    fn test_parse_triple_combo() {
        let combo = KeyCombo::parse("ctrl+shift+enter");
        assert_eq!(combo.key, "enter");
        assert!(combo.ctrl);
        assert!(combo.shift);
    }

    #[test]
    fn test_matches_enter() {
        let combo = KeyCombo::parse("enter");
        assert!(combo.matches_crossterm(KeyCode::Enter, KeyModifiers::NONE));
        assert!(!combo.matches_crossterm(KeyCode::Enter, KeyModifiers::CONTROL));
    }

    #[test]
    fn test_matches_ctrl_c() {
        let combo = KeyCombo::parse("ctrl+c");
        assert!(combo.matches_crossterm(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(!combo.matches_crossterm(KeyCode::Char('c'), KeyModifiers::NONE));
    }

    #[test]
    fn test_default_bindings() {
        let config = KeybindingConfig::defaults();
        assert!(config.matches(&Action::Submit, KeyCode::Enter, KeyModifiers::NONE));
        assert!(config.matches(&Action::Cancel, KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(!config.matches(&Action::Submit, KeyCode::Char('a'), KeyModifiers::NONE));
    }

    #[test]
    fn test_override_bindings() {
        let mut config = KeybindingConfig::defaults();
        let mut overrides = HashMap::new();
        overrides.insert("submit".to_string(), vec!["ctrl+enter".to_string()]);
        config.apply_overrides(&overrides);

        // Now submit requires ctrl+enter, not plain enter
        assert!(config.matches(&Action::Submit, KeyCode::Enter, KeyModifiers::CONTROL));
        assert!(!config.matches(&Action::Submit, KeyCode::Enter, KeyModifiers::NONE));
    }
}
