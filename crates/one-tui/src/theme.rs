//! Theme resolution — converts string color names from config into ratatui Colors.

use one_core::config::ThemeColors;
use ratatui::style::Color;

/// Pre-resolved theme colors for efficient rendering.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub user_text: Color,
    pub assistant_text: Color,
    pub tool_call: Color,
    pub error: Color,
    pub border: Color,
    pub highlight: Color,
    pub muted: Color,
    pub diff_add: Color,
    pub diff_remove: Color,
}

impl Theme {
    /// Resolve a ThemeColors config into ratatui Colors.
    pub fn from_config(colors: &ThemeColors) -> Self {
        Self {
            user_text: parse_color(&colors.user_text),
            assistant_text: parse_color(&colors.assistant_text),
            tool_call: parse_color(&colors.tool_call),
            error: parse_color(&colors.error),
            border: parse_color(&colors.border),
            highlight: parse_color(&colors.highlight),
            muted: parse_color(&colors.muted),
            diff_add: parse_color(&colors.diff_add),
            diff_remove: parse_color(&colors.diff_remove),
        }
    }

    /// Default dark theme.
    pub fn dark() -> Self {
        Self::from_config(&ThemeColors::dark())
    }

    /// Light theme.
    pub fn light() -> Self {
        Self::from_config(&ThemeColors::light())
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

/// Parse a color name or hex value into a ratatui Color.
pub fn parse_color(name: &str) -> Color {
    match name.to_lowercase().as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" | "purple" => Color::Magenta,
        "cyan" => Color::Cyan,
        "white" => Color::White,
        "gray" | "grey" => Color::DarkGray,
        "light_red" => Color::LightRed,
        "light_green" => Color::LightGreen,
        "light_yellow" => Color::LightYellow,
        "light_blue" => Color::LightBlue,
        "light_magenta" => Color::LightMagenta,
        "light_cyan" => Color::LightCyan,
        s if s.starts_with('#') && s.len() == 7 => {
            let r = u8::from_str_radix(&s[1..3], 16).unwrap_or(255);
            let g = u8::from_str_radix(&s[3..5], 16).unwrap_or(255);
            let b = u8::from_str_radix(&s[5..7], 16).unwrap_or(255);
            Color::Rgb(r, g, b)
        }
        _ => Color::White,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_named_colors() {
        assert_eq!(parse_color("red"), Color::Red);
        assert_eq!(parse_color("cyan"), Color::Cyan);
        assert_eq!(parse_color("gray"), Color::DarkGray);
        assert_eq!(parse_color("BLUE"), Color::Blue);
    }

    #[test]
    fn test_parse_hex_color() {
        assert_eq!(parse_color("#FF0000"), Color::Rgb(255, 0, 0));
        assert_eq!(parse_color("#00ff00"), Color::Rgb(0, 255, 0));
        assert_eq!(parse_color("#1a2b3c"), Color::Rgb(26, 43, 60));
    }

    #[test]
    fn test_parse_unknown_defaults_white() {
        assert_eq!(parse_color("nonexistent"), Color::White);
    }

    #[test]
    fn test_theme_from_config() {
        let theme = Theme::from_config(&ThemeColors::dark());
        assert_eq!(theme.error, Color::Red);
        assert_eq!(theme.highlight, Color::Cyan);
    }

    #[test]
    fn test_light_theme() {
        let theme = Theme::light();
        assert_eq!(theme.user_text, Color::Black);
        assert_eq!(theme.highlight, Color::Blue);
    }
}
