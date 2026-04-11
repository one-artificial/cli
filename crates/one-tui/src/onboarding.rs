use std::io;

use anyhow::Result;
use crossterm::{
    event::{self, Event as CrosstermEvent, KeyCode},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Paragraph},
};

use one_core::config::AppConfig;

// ── Owl artscape ─────────────────────────────────────────────────

/// 12-line artscape: owl in the upper right, stars and clouds scattered.
const ARTSCAPE: &[&str] = &[
    "     *                                       \u{2584}\u{2593}\u{2588}\u{2588}\u{2593}\u{2584}",
    "                                 *         \u{2584}\u{2588}\u{2591}\u{2584}\u{2593}\u{2593}\u{2584}\u{2591}\u{2588}\u{2584}",
    "            \u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}                        \u{2588}\u{2593}  \u{2588}  \u{2588}  \u{2593}\u{2588}",
    "    \u{2591}\u{2591}\u{2591}   \u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}                      \u{2588}\u{2592}   \u{2590}\u{258C}   \u{2592}\u{2588}",
    "   \u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}    *               \u{2588}\u{2593}\u{2591}    \u{2591}\u{2593}\u{2588}",
    "                                            \u{2580}\u{2588}\u{2588}\u{2593}\u{2593}\u{2588}\u{2588}\u{2580}",
    " *                                 \u{2591}\u{2591}\u{2591}\u{2591}",
    "                                 \u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}",
    "                               \u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}",
    "                                                        *",
    "              *                          *",
    "                       *",
];

// ── Gradient ─────────────────────────────────────────────────────

/// Color stops for artscape gradient. Format: (row_fraction, r, g, b).
const GRADIENT: &[(f32, u8, u8, u8)] = &[
    (0.00, 0x33, 0x88, 0xFF), // blue
    (0.25, 0x33, 0xCC, 0xFF), // cyan
    (0.45, 0x77, 0x44, 0xFF), // blue-purple
    (0.65, 0xCC, 0x33, 0xCC), // magenta
    (0.85, 0xFF, 0x44, 0x88), // pink
    (1.00, 0xFF, 0x66, 0x44), // orange-pink
];

/// Interpolate the gradient color for a given fraction (0.0 = top, 1.0 = bottom).
fn gradient_color(frac: f32, dim: f32) -> Color {
    let frac = frac.clamp(0.0, 1.0);

    let mut lower = GRADIENT[0];
    let mut upper = GRADIENT[GRADIENT.len() - 1];
    for window in GRADIENT.windows(2) {
        if frac >= window[0].0 && frac <= window[1].0 {
            lower = window[0];
            upper = window[1];
            break;
        }
    }

    let range = (upper.0 - lower.0).max(0.001);
    let t = (frac - lower.0) / range;

    let lerp = |a: u8, b: u8| -> u8 {
        let v = a as f32 + (b as f32 - a as f32) * t;
        (v * dim).clamp(0.0, 255.0) as u8
    };

    Color::Rgb(
        lerp(lower.1, upper.1),
        lerp(lower.2, upper.2),
        lerp(lower.3, upper.3),
    )
}

// ── Theme options ────────────────────────────────────────────────

struct ThemeOption {
    label: &'static str,
    config_name: &'static str,
    /// Screen background fill — the primary way the UI visibly shifts per theme.
    bg: Color,
    /// Primary text color on top of `bg`.
    fg: Color,
    /// Accent/highlight: header, separators, selected items.
    accent: Color,
    /// Diff addition color for the inline preview panel.
    diff_add: Color,
    /// Diff deletion color for the inline preview panel.
    diff_remove: Color,
}

const THEME_OPTIONS: &[ThemeOption] = &[
    ThemeOption {
        label: "Dark mode",
        config_name: "dark",
        bg: Color::Rgb(0x14, 0x16, 0x1A),
        fg: Color::Rgb(0xDC, 0xDC, 0xDC),
        accent: Color::Cyan,
        diff_add: Color::Green,
        diff_remove: Color::Red,
    },
    ThemeOption {
        label: "Light mode",
        config_name: "light",
        bg: Color::Rgb(0xF5, 0xF5, 0xF5),
        fg: Color::Rgb(0x1A, 0x1A, 0x1A),
        accent: Color::Blue,
        diff_add: Color::Green,
        diff_remove: Color::Red,
    },
    ThemeOption {
        label: "Dark mode (colorblind-friendly)",
        config_name: "dark_colorblind",
        bg: Color::Rgb(0x14, 0x16, 0x1A),
        fg: Color::Rgb(0xDC, 0xDC, 0xDC),
        accent: Color::Rgb(0x44, 0x88, 0xFF),
        diff_add: Color::Rgb(0x44, 0x88, 0xFF),
        diff_remove: Color::Rgb(0xFF, 0x88, 0x00),
    },
    ThemeOption {
        label: "Light mode (colorblind-friendly)",
        config_name: "light_colorblind",
        bg: Color::Rgb(0xF5, 0xF5, 0xF5),
        fg: Color::Rgb(0x1A, 0x1A, 0x1A),
        accent: Color::Rgb(0x00, 0x88, 0xFF),
        diff_add: Color::Rgb(0x00, 0x55, 0xCC),
        diff_remove: Color::Rgb(0xCC, 0x66, 0x00),
    },
    ThemeOption {
        label: "Dark mode (ANSI colors only)",
        config_name: "dark_ansi",
        bg: Color::Black,
        fg: Color::White,
        accent: Color::LightCyan,
        diff_add: Color::LightGreen,
        diff_remove: Color::LightRed,
    },
];

// ── Syntax theme options ─────────────────────────────────────────

struct SyntaxThemeOption {
    label: &'static str,
    config_name: &'static str,
    keyword: Color,
    string: Color,
    /// Comment color — medium-darkness values chosen to be readable on both
    /// dark and light backgrounds (avoids the near-white plain text problem).
    comment: Color,
}

const SYNTAX_THEMES: &[SyntaxThemeOption] = &[
    SyntaxThemeOption {
        label: "Monokai",
        config_name: "monokai",
        keyword: Color::Rgb(0xF9, 0x26, 0x72),
        string: Color::Rgb(0xE6, 0xDB, 0x74),
        comment: Color::Rgb(0x75, 0x71, 0x5E),
    },
    SyntaxThemeOption {
        label: "One Dark",
        config_name: "one_dark",
        keyword: Color::Rgb(0xC6, 0x78, 0xDD),
        string: Color::Rgb(0x98, 0xC3, 0x79),
        comment: Color::Rgb(0x5C, 0x63, 0x70),
    },
    SyntaxThemeOption {
        label: "Dracula",
        config_name: "dracula",
        keyword: Color::Rgb(0xFF, 0x79, 0xC6),
        string: Color::Rgb(0xF1, 0xFA, 0x8C),
        comment: Color::Rgb(0x62, 0x72, 0xA4),
    },
    SyntaxThemeOption {
        label: "Plain",
        config_name: "plain",
        keyword: Color::Gray,
        string: Color::Gray,
        comment: Color::DarkGray,
    },
];

// ── Provider definitions ─────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provider {
    Anthropic,
    OpenAi,
    Google,
    Ollama,
    Skip,
}

impl Provider {
    fn all() -> &'static [Provider] {
        &[
            Provider::Anthropic,
            Provider::OpenAi,
            Provider::Google,
            Provider::Ollama,
            Provider::Skip,
        ]
    }

    fn label(&self) -> &'static str {
        match self {
            Provider::Anthropic => "Anthropic (Claude)",
            Provider::OpenAi => "OpenAI (GPT)",
            Provider::Google => "Google (Gemini)",
            Provider::Ollama => "Ollama (local, no auth)",
            Provider::Skip => "Skip for now",
        }
    }

    fn hint(&self) -> &'static str {
        match self {
            Provider::Anthropic => "Best experience: thinking, prompt caching, extended context",
            Provider::OpenAi => "GPT-4o and o-series models",
            Provider::Google => "Gemini models via AI Studio",
            Provider::Ollama => "Free, private, runs on your machine",
            Provider::Skip => "Auto-detect later, or use /login",
        }
    }

    fn config_name(&self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::OpenAi => "openai",
            Provider::Google => "google",
            Provider::Ollama => "ollama",
            Provider::Skip => "",
        }
    }

    fn default_model(&self) -> &'static str {
        match self {
            Provider::Anthropic => "claude-sonnet-4-20250514",
            Provider::OpenAi => "gpt-4o",
            Provider::Google => "gemini-2.0-flash",
            Provider::Ollama => "llama3",
            Provider::Skip => "",
        }
    }

    fn env_var(&self) -> &'static str {
        match self {
            Provider::Anthropic => "ANTHROPIC_API_KEY",
            Provider::OpenAi => "OPENAI_API_KEY",
            Provider::Google => "GOOGLE_API_KEY",
            _ => "",
        }
    }

    fn key_url(&self) -> &'static str {
        match self {
            Provider::Anthropic => "console.anthropic.com/settings/keys",
            Provider::OpenAi => "platform.openai.com/api-keys",
            Provider::Google => "aistudio.google.com/apikey",
            _ => "",
        }
    }

    fn key_prefix(&self) -> &'static str {
        match self {
            Provider::Anthropic => "sk-ant-",
            Provider::OpenAi => "sk-",
            _ => "",
        }
    }

    fn needs_api_key(&self) -> bool {
        matches!(
            self,
            Provider::Anthropic | Provider::OpenAi | Provider::Google
        )
    }

    /// Check whether an API key already exists in the environment.
    fn has_env_key(&self) -> bool {
        let var = self.env_var();
        !var.is_empty() && std::env::var(var).ok().filter(|v| !v.is_empty()).is_some()
    }

    /// Check whether a provider is configured (has an API key or is local).
    fn is_configured(&self, config: &AppConfig) -> bool {
        match self {
            Provider::Anthropic => {
                self.has_env_key() || config.provider.anthropic.api_key.is_some()
            }
            Provider::OpenAi => self.has_env_key() || config.provider.openai.api_key.is_some(),
            Provider::Google => self.has_env_key() || config.provider.google.api_key.is_some(),
            Provider::Ollama => true, // Local, always available
            Provider::Skip => true,   // Skip doesn't need configuration
        }
    }
}

// ── Onboarding steps ─────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    ThemeSelect,
    Login,
    ProviderSelect,
    ApiKeyInput,
    SecurityNote,
    Done,
}

// ── Public entry point ───────────────────────────────────────────

/// Run the onboarding wizard in a standalone TUI.
/// This completes fully before the chat UI is ever created.
pub async fn run_onboarding(config: &mut AppConfig) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = onboarding_loop(&mut terminal, config).await;

    // Clean exit: leave alternate screen so the chat UI starts fresh
    disable_raw_mode()?;
    crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

// ── Main loop ────────────────────────────────────────────────────

async fn onboarding_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    config: &mut AppConfig,
) -> Result<()> {
    let mut step = Step::ThemeSelect;
    let mut selected_idx: usize = 0;
    let mut selected_syntax_idx: usize = 0;
    let mut confirmed_theme_idx: usize = 0;
    let mut selected_provider = Provider::Skip;
    let mut api_key_buf = String::new();
    let mut cursor_blink = false;
    let mut tick: u16 = 0;

    loop {
        let step_snap = step;
        let idx_snap = selected_idx;
        let syntax_snap = selected_syntax_idx;
        let prov_snap = selected_provider;
        let key_snap = api_key_buf.clone();
        let blink = cursor_blink;
        // Live theme preview: on the theme step use the hovered option's palette so
        // the whole screen visibly shifts as the user navigates; after selection use
        // the confirmed choice so subsequent steps match what was picked.
        let preview_idx = if step_snap == Step::ThemeSelect {
            idx_snap
        } else {
            confirmed_theme_idx
        };
        let bg_snap = THEME_OPTIONS[preview_idx].bg;
        let fg_snap = THEME_OPTIONS[preview_idx].fg;
        let accent_snap = THEME_OPTIONS[preview_idx].accent;

        terminal.draw(|f| {
            draw_screen(
                f,
                step_snap,
                idx_snap,
                syntax_snap,
                prov_snap,
                &key_snap,
                blink,
                bg_snap,
                fg_snap,
                accent_snap,
            );
        })?;

        if step == Step::Done {
            if event::poll(std::time::Duration::from_millis(1200))? {
                let _ = event::read()?;
            }
            break;
        }

        // Advance cursor blink
        tick = tick.wrapping_add(1);
        if tick.is_multiple_of(10) {
            cursor_blink = !cursor_blink;
        }

        if !event::poll(std::time::Duration::from_millis(50))? {
            continue;
        }

        if let CrosstermEvent::Key(key) = event::read()? {
            match step {
                // ── Theme + syntax selection ─────────────────
                Step::ThemeSelect => match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        selected_idx = selected_idx.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if selected_idx < THEME_OPTIONS.len() - 1 {
                            selected_idx += 1;
                        }
                    }
                    KeyCode::Char('t') => {
                        // Cycle syntax theme inline — preview updates immediately
                        selected_syntax_idx = (selected_syntax_idx + 1) % SYNTAX_THEMES.len();
                    }
                    KeyCode::Enter => {
                        let opt = &THEME_OPTIONS[selected_idx];
                        config.ui.theme = opt.config_name.to_string();
                        config.ui.colors =
                            one_core::config::ThemeColors::for_theme(opt.config_name);
                        config.ui.syntax_theme =
                            SYNTAX_THEMES[selected_syntax_idx].config_name.to_string();
                        confirmed_theme_idx = selected_idx;
                        step = Step::Login;
                        selected_idx = 0;
                    }
                    KeyCode::Esc => {
                        one_core::onboarding::mark_onboarding_complete(config)?;
                        step = Step::Done;
                    }
                    _ => {}
                },

                // ── HF login ────────────────────────────────
                Step::Login => match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        selected_idx = selected_idx.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if selected_idx < 1 {
                            selected_idx = 1;
                        }
                    }
                    KeyCode::Enter => {
                        if selected_idx == 0 {
                            // HF OAuth — leave alternate screen for browser
                            disable_raw_mode()?;
                            crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

                            eprintln!("Opening browser for Hugging Face login...");
                            match one_core::oauth::browser_login("huggingface").await {
                                Ok(result) => {
                                    for msg in &result.messages {
                                        eprintln!("{msg}");
                                    }
                                }
                                Err(e) => {
                                    eprintln!("HF login failed: {e}");
                                    eprintln!("You can try again later with /login");
                                }
                            }

                            // Re-enter alternate screen
                            enable_raw_mode()?;
                            crossterm::execute!(terminal.backend_mut(), EnterAlternateScreen)?;
                        }
                        step = Step::ProviderSelect;
                        selected_idx = 0;
                    }
                    KeyCode::Esc => {
                        one_core::onboarding::mark_onboarding_complete(config)?;
                        step = Step::Done;
                    }
                    _ => {}
                },

                // ── Provider selection ──────────────────────
                Step::ProviderSelect => match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        selected_idx = selected_idx.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        // +1 for the "Continue →" entry at the bottom
                        let max = Provider::all().len();
                        if selected_idx < max {
                            selected_idx += 1;
                        }
                    }
                    KeyCode::Enter => {
                        let all = Provider::all();
                        if selected_idx == all.len() {
                            // "Continue →" — advance, picking first configured as default
                            let first = all.iter().find(|p| p.is_configured(config));
                            if let Some(p) = first
                                && config.provider.default_provider.is_empty()
                            {
                                config.provider.default_provider = p.config_name().to_string();
                                config.provider.default_model = p.default_model().to_string();
                            }
                            step = Step::SecurityNote;
                        } else {
                            selected_provider = all[selected_idx];
                            // Set as default provider on selection
                            if !selected_provider.config_name().is_empty() {
                                config.provider.default_provider =
                                    selected_provider.config_name().to_string();
                                config.provider.default_model =
                                    selected_provider.default_model().to_string();
                            }
                            if selected_provider.needs_api_key()
                                && !selected_provider.is_configured(config)
                            {
                                step = Step::ApiKeyInput;
                                api_key_buf.clear();
                            }
                            // Already configured or local (Ollama/Skip): stay on this screen
                        }
                    }
                    KeyCode::Esc => {
                        one_core::onboarding::mark_onboarding_complete(config)?;
                        step = Step::Done;
                    }
                    _ => {}
                },

                // ── API key input ───────────────────────────
                Step::ApiKeyInput => match key.code {
                    KeyCode::Char(c) => {
                        api_key_buf.push(c);
                    }
                    KeyCode::Backspace => {
                        api_key_buf.pop();
                    }
                    KeyCode::Enter => {
                        let trimmed = api_key_buf.trim().to_string();
                        if !trimmed.is_empty() {
                            // Store in config file only — env vars are already resolved
                            // directly so no keychain write needed.
                            match selected_provider {
                                Provider::Anthropic => {
                                    config.provider.anthropic.api_key = Some(trimmed);
                                }
                                Provider::OpenAi => {
                                    config.provider.openai.api_key = Some(trimmed);
                                }
                                Provider::Google => {
                                    config.provider.google.api_key = Some(trimmed);
                                }
                                _ => {}
                            }
                        }
                        // Return to provider list so user can configure more providers
                        step = Step::ProviderSelect;
                    }
                    KeyCode::Esc => {
                        // Cancelled — return to provider list without saving
                        step = Step::ProviderSelect;
                    }
                    _ => {}
                },

                // ── Security note ───────────────────────────
                Step::SecurityNote => {
                    if matches!(key.code, KeyCode::Enter | KeyCode::Esc) {
                        one_core::onboarding::mark_onboarding_complete(config)?;
                        step = Step::Done;
                    }
                }

                Step::Done => break,
            }
        }
    }

    Ok(())
}

// ── Screen layout ────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn draw_screen(
    f: &mut Frame,
    step: Step,
    selected_idx: usize,
    selected_syntax_idx: usize,
    provider: Provider,
    key_buf: &str,
    cursor_blink: bool,
    bg: Color,
    fg: Color,
    accent: Color,
) {
    let area = f.area();
    let width = area.width as usize;

    // Only fill the bg on the theme selection step (live preview).
    // On all other steps the terminal's own background is used to avoid
    // banding artifacts from our bg color differing from the terminal default.
    if step == Step::ThemeSelect {
        f.render_widget(Block::default().style(Style::default().bg(bg)), area);
    }

    let artscape_h = ARTSCAPE.len() as u16;
    // header(1) + top_sep(1) + artscape + bot_sep(1) + content(min 8)
    let show_artscape = area.height >= 3 + artscape_h + 8;

    if show_artscape {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),          // header
                Constraint::Length(1),          // top separator
                Constraint::Length(artscape_h), // artscape
                Constraint::Length(1),          // bottom separator
                Constraint::Min(0),             // content
            ])
            .split(area);

        draw_header(f, chunks[0], accent);
        draw_separator(f, chunks[1], width, accent);
        draw_artscape(f, chunks[2], width);
        draw_separator(f, chunks[3], width, accent);
        draw_content(
            f,
            chunks[4],
            step,
            selected_idx,
            selected_syntax_idx,
            provider,
            key_buf,
            cursor_blink,
            fg,
        );
    } else {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // header
                Constraint::Length(1), // separator
                Constraint::Min(0),    // content
            ])
            .split(area);

        draw_header(f, chunks[0], accent);
        draw_separator(f, chunks[1], width, accent);
        draw_content(
            f,
            chunks[2],
            step,
            selected_idx,
            selected_syntax_idx,
            provider,
            key_buf,
            cursor_blink,
            fg,
        );
    }
}

fn draw_header(f: &mut Frame, area: Rect, accent: Color) {
    let version = env!("CARGO_PKG_VERSION");
    let header = Paragraph::new(Line::from(dspan_bold(
        format!("Welcome to One v{version}"),
        accent,
    )));
    f.render_widget(header, area);
}

fn draw_artscape(f: &mut Frame, area: Rect, term_width: usize) {
    let total = ARTSCAPE.len().max(1);
    let mut lines: Vec<Line<'static>> = Vec::new();

    for (i, art_line) in ARTSCAPE.iter().enumerate() {
        let frac = i as f32 / total as f32;
        let color = gradient_color(frac, 0.75);

        let text_len = art_line.chars().count();
        let left_pad = if term_width > text_len {
            (term_width - text_len) / 2
        } else {
            0
        };

        let mut spans = vec![];
        if left_pad > 0 {
            spans.push(Span::raw(" ".repeat(left_pad)));
        }
        spans.push(Span::styled(
            art_line.to_string(),
            Style::default().fg(color),
        ));
        lines.push(Line::from(spans));
    }

    f.render_widget(Paragraph::new(Text::from(lines)), area);
}

fn draw_separator(f: &mut Frame, area: Rect, width: usize, color: Color) {
    let sep = Paragraph::new(Line::from(dspan("\u{2026}".repeat(width), color)));
    f.render_widget(sep, area);
}

#[allow(clippy::too_many_arguments)]
fn draw_content(
    f: &mut Frame,
    area: Rect,
    step: Step,
    selected_idx: usize,
    selected_syntax_idx: usize,
    provider: Provider,
    key_buf: &str,
    cursor_blink: bool,
    fg: Color,
) {
    let lines = match step {
        Step::ThemeSelect => theme_select_lines(selected_idx, selected_syntax_idx, fg),
        Step::Login => login_lines(selected_idx),
        Step::ProviderSelect => provider_select_lines(selected_idx),
        Step::ApiKeyInput => api_key_input_lines(provider, key_buf, cursor_blink),
        Step::SecurityNote => security_note_lines(),
        Step::Done => done_lines(),
    };
    f.render_widget(Paragraph::new(Text::from(lines)), area);
}

// ── Content builders ─────────────────────────────────────────────

fn theme_select_lines(
    selected_theme_idx: usize,
    selected_syntax_idx: usize,
    fg: Color,
) -> Vec<Line<'static>> {
    let theme_opt = &THEME_OPTIONS[selected_theme_idx];
    let syntax_opt = &SYNTAX_THEMES[selected_syntax_idx];

    // ── Theme options ──────────────────────────────────────────────
    let mut lines = vec![
        Line::from(""),
        Line::from(dspan_bold(" Let's get started.", fg)),
        Line::from(""),
        Line::from(dspan(
            " Choose the text style that looks best with your terminal",
            fg,
        )),
        Line::from(dspan(" To change this later, run /theme", Color::DarkGray)),
        Line::from(""),
    ];

    for (i, opt) in THEME_OPTIONS.iter().enumerate() {
        if i == selected_theme_idx {
            // ✓ in the theme's own accent — signals both cursor and live color preview
            lines.push(Line::from(vec![
                dspan_bold(" \u{2713} ", opt.accent),
                dspan_bold(format!("{}. {}", i + 1, opt.label), opt.accent),
            ]));
        } else {
            lines.push(Line::from(vec![
                dspan("   ", Color::DarkGray),
                dspan(format!("{}. ", i + 1), Color::DarkGray),
                dspan(opt.label.to_string(), opt.accent),
            ]));
        }
    }

    // ── Diff color preview (the main visual diff between themes) ───
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        dspan_bold(" + ", theme_opt.diff_add),
        dspan("added line   ", theme_opt.diff_add),
        dspan_bold(" - ", theme_opt.diff_remove),
        dspan("removed line", theme_opt.diff_remove),
    ]));

    // ── Syntax-highlighted code snippet ───────────────────────────
    // Plain text uses `fg` (the theme's primary text color) rather than the
    // syntax theme's fixed plain color, so it stays readable on light backgrounds.
    let kw = |s: &'static str| Span::styled(s, Style::default().fg(syntax_opt.keyword));
    let str_s = |s: &'static str| Span::styled(s, Style::default().fg(syntax_opt.string));
    let cmt = |s: &'static str| Span::styled(s, Style::default().fg(syntax_opt.comment));
    let pl = |s: &'static str| Span::styled(s, Style::default().fg(fg));
    let border = Style::default().fg(Color::DarkGray);

    let code_lines: Vec<Line<'static>> = vec![
        Line::from(vec![pl("  "), kw("function"), pl(" greet() {")]),
        Line::from(vec![cmt("    // Say hello")]),
        Line::from(vec![
            pl("    "),
            kw("const"),
            pl(" msg = "),
            str_s("\"Hello, Claude!\""),
            pl(";"),
        ]),
        Line::from(vec![pl("    "), kw("return"), pl(" msg;")]),
        Line::from(vec![pl("  }")]),
    ];

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  \u{256D}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}",
        border,
    )));
    for code_line in code_lines {
        let mut spans = vec![Span::styled("  \u{2502} ", border)];
        spans.extend(code_line.spans);
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(Span::styled(
        "  \u{2570}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}",
        border,
    )));
    lines.push(Line::from(vec![
        dspan("  Syntax theme: ", Color::DarkGray),
        dspan_bold(syntax_opt.label.to_string(), syntax_opt.keyword),
        dspan("  (t to cycle)", Color::DarkGray),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        dspan_bold(" Enter ", Color::Green),
        dspan("confirm  ", Color::DarkGray),
        dspan_bold(" t ", theme_opt.accent),
        dspan("cycle syntax theme", Color::DarkGray),
    ]));

    lines
}

fn login_lines(selected_idx: usize) -> Vec<Line<'static>> {
    let options: &[(&str, &str)] = &[
        ("Sign in with Hugging Face", "Opens browser for OAuth"),
        ("Skip for now", "Sign in later with /login"),
    ];

    let mut lines = vec![
        Line::from(""),
        Line::from(dspan(
            " One can be used with your Hugging Face account for AI inference",
            Color::White,
        )),
        Line::from(dspan(" and model access.", Color::White)),
        Line::from(""),
        Line::from(dspan(" Select login method:", Color::White)),
        Line::from(""),
    ];

    for (i, (label, hint)) in options.iter().enumerate() {
        if i == selected_idx {
            lines.push(Line::from(vec![
                dspan_bold(" \u{276F} ", Color::Cyan),
                dspan_bold(format!("{}. {label}", i + 1), Color::Cyan),
                dspan(format!(" \u{00B7} {hint}"), Color::DarkGray),
            ]));
        } else {
            lines.push(Line::from(vec![
                dspan(format!("   {}. {label}", i + 1), Color::White),
                dspan(format!(" \u{00B7} {hint}"), Color::DarkGray),
            ]));
        }
        lines.push(Line::from(""));
    }

    lines
}

fn provider_select_lines(selected_idx: usize) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(""),
        Line::from(dspan(
            " Which AI provider would you like to use?",
            Color::White,
        )),
        Line::from(""),
    ];

    for (i, provider) in Provider::all().iter().enumerate() {
        let has_key = provider.has_env_key();
        let is_sel = i == selected_idx;

        if is_sel {
            let mut spans = vec![
                dspan_bold(" \u{276F} ", Color::Cyan),
                dspan_bold(format!("{}. {}", i + 1, provider.label()), Color::Cyan),
            ];
            if has_key {
                spans.push(dspan(
                    format!(" \u{00B7} \u{2713} found in {}", provider.env_var()),
                    Color::Green,
                ));
            }
            lines.push(Line::from(spans));
            // Description on the next line for selected item
            lines.push(Line::from(dspan(
                format!("      {}", provider.hint()),
                Color::DarkGray,
            )));
        } else {
            let mut spans = vec![dspan(
                format!("   {}. {}", i + 1, provider.label()),
                Color::White,
            )];
            if has_key {
                spans.push(dspan(
                    format!(" \u{00B7} \u{2713} {}", provider.env_var()),
                    Color::Green,
                ));
            }
            lines.push(Line::from(spans));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(dspan(
        " Add more providers anytime with /login",
        Color::DarkGray,
    )));

    lines
}

fn api_key_input_lines(
    provider: Provider,
    key_buf: &str,
    cursor_blink: bool,
) -> Vec<Line<'static>> {
    // Masked key display: show prefix chars, mask the rest, blinking block cursor
    let display = if key_buf.is_empty() {
        if cursor_blink { "\u{2588}" } else { " " }.to_string()
    } else {
        let visible = key_buf.len().min(7);
        let prefix = &key_buf[..visible];
        let rest = key_buf.len().saturating_sub(visible);
        let cursor = if cursor_blink { "\u{2588}" } else { "" };
        if rest > 0 {
            format!("{prefix}{}{cursor}", "\u{2022}".repeat(rest.min(24)))
        } else {
            format!("{prefix}{cursor}")
        }
    };

    let mut lines = vec![
        Line::from(""),
        Line::from(dspan_bold(
            format!(" Enter your {} API key", provider.label()),
            Color::White,
        )),
        Line::from(""),
        Line::from(dspan(
            format!(" Get your key at {}", provider.key_url()),
            Color::DarkGray,
        )),
        Line::from(""),
        Line::from(vec![
            dspan("  ", Color::White),
            Span::styled(
                format!(" {display:<40} "),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(""),
    ];

    let prefix = provider.key_prefix();
    if !prefix.is_empty() {
        lines.push(Line::from(dspan(
            format!(" Prefix: {prefix}     Stored in: OS keychain"),
            Color::DarkGray,
        )));
    } else {
        lines.push(Line::from(dspan(
            " Stored in: OS keychain",
            Color::DarkGray,
        )));
    }

    lines.push(Line::from(dspan(
        format!(" Or set {} in your shell", provider.env_var()),
        Color::DarkGray,
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        dspan_bold(" Enter ", Color::Green),
        dspan("confirm  ", Color::DarkGray),
        dspan_bold(" Esc ", Color::Yellow),
        dspan("skip", Color::DarkGray),
    ]));

    lines
}

fn security_note_lines() -> Vec<Line<'static>> {
    vec![
        Line::from(""),
        Line::from(dspan_bold(" Security notes:", Color::White)),
        Line::from(""),
        Line::from(dspan_bold(" 1. AI can make mistakes", Color::Yellow)),
        Line::from(dspan(
            "    You should always review AI responses, especially when",
            Color::DarkGray,
        )),
        Line::from(dspan("    running code.", Color::DarkGray)),
        Line::from(""),
        Line::from(dspan_bold(
            " 2. Due to prompt injection risks, only use it with code you trust",
            Color::Yellow,
        )),
        Line::from(dspan("    For more details see:", Color::DarkGray)),
        Line::from(dspan(
            "    https://github.com/one-artificial/cli",
            Color::Cyan,
        )),
        Line::from(""),
        Line::from(dspan_bold(" Press Enter to continue\u{2026}", Color::Green)),
    ]
}

fn done_lines() -> Vec<Line<'static>> {
    vec![
        Line::from(""),
        Line::from(dspan_bold(" Starting One\u{2026}", Color::Green)),
    ]
}

// ── Helpers ──────────────────────────────────────────────────────

fn dspan(text: impl Into<String>, fg: Color) -> Span<'static> {
    Span::styled(text.into(), Style::default().fg(fg))
}

fn dspan_bold(text: impl Into<String>, fg: Color) -> Span<'static> {
    Span::styled(
        text.into(),
        Style::default().fg(fg).add_modifier(Modifier::BOLD),
    )
}
