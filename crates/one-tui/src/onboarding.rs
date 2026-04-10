use std::io;

use anyhow::Result;
use crossterm::{
    event::{self, Event as CrosstermEvent, KeyCode},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use one_core::config::AppConfig;

// ── ASCII logo background ─────────────────────────────────────────

/// The One logo as ASCII art (from Pro.txt).
const LOGO_ART: &str = include_str!("logo.txt");

/// Color stops for the logo gradient (matches Pro.png).
/// Format: (row_fraction, r, g, b)
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

    // Find the two stops we're between
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

/// Render the ASCII logo as a dimmed colored background filling the terminal.
fn draw_logo_background(f: &mut Frame) {
    let area = f.area();
    let lines: Vec<&str> = LOGO_ART.lines().collect();
    let total_rows = lines.len().max(1);

    // Center the logo vertically and horizontally
    let y_offset = if area.height as usize > total_rows {
        (area.height as usize - total_rows) / 2
    } else {
        0
    };

    let mut styled_lines: Vec<Line<'static>> = Vec::new();

    for row in 0..area.height as usize {
        let logo_row = row.checked_sub(y_offset);
        let w = area.width as usize;
        if let Some(lr) = logo_row
            && lr < total_rows
        {
            let line_text = lines[lr];
            let frac = lr as f32 / total_rows as f32;
            let color = gradient_color(frac, 0.35);

            // Center horizontally
            let text_len = line_text.len();
            let left_pad = if w > text_len { (w - text_len) / 2 } else { 0 };

            let mut spans = vec![];
            if left_pad > 0 {
                spans.push(Span::raw(" ".repeat(left_pad)));
            }
            spans.push(Span::styled(
                line_text.to_string(),
                Style::default().fg(color),
            ));
            styled_lines.push(Line::from(spans));
            continue;
        }
        // Empty row (above or below the logo)
        styled_lines.push(Line::from(""));
    }

    let paragraph = Paragraph::new(Text::from(styled_lines));
    f.render_widget(paragraph, area);
}

// ── Provider definitions ──────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provider {
    Anthropic,
    OpenAi,
    Ollama,
    Skip,
}

impl Provider {
    fn all() -> &'static [Provider] {
        &[
            Provider::Anthropic,
            Provider::OpenAi,
            Provider::Ollama,
            Provider::Skip,
        ]
    }

    fn label(&self) -> &'static str {
        match self {
            Provider::Anthropic => "Anthropic (Claude)",
            Provider::OpenAi => "OpenAI (GPT)",
            Provider::Ollama => "Ollama (local, no auth)",
            Provider::Skip => "Skip for now",
        }
    }

    fn hint(&self) -> &'static str {
        match self {
            Provider::Anthropic => "Best experience: thinking, prompt caching, extended context",
            Provider::OpenAi => "GPT-4o and o-series models",
            Provider::Ollama => "Free, private, runs on your machine",
            Provider::Skip => "Auto-detect later, or use /login",
        }
    }

    fn config_name(&self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::OpenAi => "openai",
            Provider::Ollama => "ollama",
            Provider::Skip => "",
        }
    }

    fn default_model(&self) -> &'static str {
        match self {
            Provider::Anthropic => "claude-sonnet-4-20250514",
            Provider::OpenAi => "gpt-4o",
            Provider::Ollama => "llama3",
            Provider::Skip => "",
        }
    }

    fn needs_api_key(&self) -> bool {
        matches!(self, Provider::Anthropic | Provider::OpenAi)
    }

    fn env_var_name(&self) -> &'static str {
        match self {
            Provider::Anthropic => "ANTHROPIC_API_KEY",
            Provider::OpenAi => "OPENAI_API_KEY",
            _ => "",
        }
    }

    fn key_url(&self) -> &'static str {
        match self {
            Provider::Anthropic => "console.anthropic.com/settings/keys",
            Provider::OpenAi => "platform.openai.com/api-keys",
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

    fn keychain_name(&self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::OpenAi => "openai",
            _ => "",
        }
    }
}

// ── Onboarding steps ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Welcome,
    Identity,       // HF login — your account (optional)
    ProviderSelect, // model auth — which AI to use
    ApiKeyInput,    // API key for selected provider
    SecurityNote,
    Done,
}

// ── Public entry point ────────────────────────────────────────────

/// Run the onboarding wizard in a standalone TUI.
/// This completes fully before the chat UI is ever created.
pub async fn run_onboarding(config: &mut AppConfig) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = onboarding_loop(&mut terminal, config).await;

    // Clean exit: leave alternate screen so the chat UI starts completely fresh
    disable_raw_mode()?;
    crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

// ── Main loop ─────────────────────────────────────────────────────

async fn onboarding_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    config: &mut AppConfig,
) -> Result<()> {
    let mut step = Step::Welcome;
    let mut selected_idx: usize = 0;
    let mut selected_provider = Provider::Skip;
    let mut api_key_buf = String::new();
    let mut cursor_blink = false;
    let mut tick: u16 = 0;

    loop {
        let step_snap = step;
        let idx_snap = selected_idx;
        let prov_snap = selected_provider;
        let key_snap = api_key_buf.clone();
        let blink = cursor_blink;

        terminal.draw(|f| {
            // Layer 1: gradient-colored ASCII logo background
            draw_logo_background(f);

            // Layer 2: dialog card on top
            match step_snap {
                Step::Welcome => draw_welcome(f),
                Step::Identity => draw_identity(f, idx_snap),
                Step::ProviderSelect => draw_provider_select(f, idx_snap),
                Step::ApiKeyInput => draw_api_key_dialog(f, prov_snap, &key_snap, blink),
                Step::SecurityNote => draw_security_note(f),
                Step::Done => draw_done(f),
            }
        })?;

        tick = tick.wrapping_add(1);
        if tick.is_multiple_of(10) {
            cursor_blink = !cursor_blink;
        }

        if step == Step::Done {
            if event::poll(std::time::Duration::from_millis(1200))? {
                let _ = event::read()?;
            }
            break;
        }

        if !event::poll(std::time::Duration::from_millis(50))? {
            continue;
        }

        if let CrosstermEvent::Key(key) = event::read()? {
            match step {
                Step::Welcome => {
                    if key.code == KeyCode::Enter {
                        step = Step::Identity;
                        selected_idx = 0;
                    }
                }
                Step::Identity => match key.code {
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
                        // Either way, proceed to provider selection
                        step = Step::ProviderSelect;
                        selected_idx = 0;
                    }
                    _ => {}
                },
                Step::ProviderSelect => match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        selected_idx = selected_idx.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        let max = Provider::all().len().saturating_sub(1);
                        if selected_idx < max {
                            selected_idx += 1;
                        }
                    }
                    KeyCode::Enter => {
                        selected_provider = Provider::all()[selected_idx];
                        config.provider.default_provider =
                            selected_provider.config_name().to_string();
                        config.provider.default_model =
                            selected_provider.default_model().to_string();

                        if selected_provider.needs_api_key() {
                            step = Step::ApiKeyInput;
                            api_key_buf.clear();
                        } else {
                            step = Step::SecurityNote;
                        }
                    }
                    KeyCode::Esc => {
                        one_core::onboarding::mark_onboarding_complete(config)?;
                        step = Step::Done;
                    }
                    _ => {}
                },
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
                            let name = selected_provider.keychain_name();
                            let _ = one_core::credentials::CredentialStore::store(name, &trimmed);
                            match selected_provider {
                                Provider::Anthropic => {
                                    config.provider.anthropic.api_key = Some(trimmed);
                                }
                                Provider::OpenAi => {
                                    config.provider.openai.api_key = Some(trimmed);
                                }
                                _ => {}
                            }
                        }
                        step = Step::SecurityNote;
                    }
                    KeyCode::Esc => {
                        step = Step::SecurityNote;
                    }
                    _ => {}
                },
                Step::SecurityNote => {
                    if key.code == KeyCode::Enter {
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

// ── Dialog drawing functions ──────────────────────────────────────

fn draw_welcome(f: &mut Frame) {
    let dialog = dialog_rect(46, 14, f.area());
    let block = dialog_block(" one ");
    let inner = block.inner(dialog);
    f.render_widget(Clear, dialog);
    f.render_widget(block, dialog);

    let lines = vec![
        Line::from(""),
        Line::from(dspan_bold("   ___  _  _ ___", Color::Cyan)),
        Line::from(dspan_bold("  / _ \\| \\| | __|", Color::Cyan)),
        Line::from(dspan_bold(" | (_) | .` | _|", Color::Cyan)),
        Line::from(dspan_bold("  \\___/|_|\\_|___|", Color::Cyan)),
        Line::from(""),
        Line::from(dspan(" Your AI coding terminal.", Color::White)),
        Line::from(dspan(" Multi-provider. Extensible.", Color::DarkGray)),
        Line::from(""),
        Line::from(""),
        Line::from(dspan_bold(" Press Enter to get started", Color::Green)),
    ];

    f.render_widget(paragraph(lines), inner);
}

fn draw_identity(f: &mut Frame, selected_idx: usize) {
    let dialog = dialog_rect(52, 14, f.area());
    let block = dialog_block(" sign in ");
    let inner = block.inner(dialog);
    f.render_widget(Clear, dialog);
    f.render_widget(block, dialog);

    let options = [
        (
            "Sign in with Hugging Face",
            "Opens browser — unlocks integrations",
        ),
        ("Skip for now", "Sign in later with /login"),
    ];

    let mut lines = vec![
        Line::from(""),
        Line::from(dspan(" Your identity unlocks integrations.", Color::White)),
        Line::from(dspan(
            " Model auth is configured in the next step.",
            Color::DarkGray,
        )),
        Line::from(""),
    ];

    for (i, (label, hint)) in options.iter().enumerate() {
        let is_sel = i == selected_idx;
        if is_sel {
            lines.push(Line::from(dspan_bold(format!(" > {label}"), Color::Cyan)));
            lines.push(Line::from(dspan(format!("     {hint}"), Color::DarkGray)));
        } else {
            lines.push(Line::from(dspan(format!("   {label}"), Color::White)));
        }
    }

    f.render_widget(paragraph(lines), inner);
}

fn draw_provider_select(f: &mut Frame, selected_idx: usize) {
    let dialog = dialog_rect(60, 18, f.area());
    let block = dialog_block(" choose a provider ");
    let inner = block.inner(dialog);
    f.render_widget(Clear, dialog);
    f.render_widget(block, dialog);

    let mut lines = vec![
        Line::from(""),
        Line::from(dspan(
            " Which AI provider would you like to use?",
            Color::White,
        )),
        Line::from(""),
    ];

    for (i, provider) in Provider::all().iter().enumerate() {
        let is_sel = i == selected_idx;
        if is_sel {
            lines.push(Line::from(dspan_bold(
                format!(" > {}", provider.label()),
                Color::Cyan,
            )));
            lines.push(Line::from(dspan(
                format!("     {}", provider.hint()),
                Color::DarkGray,
            )));
        } else {
            lines.push(Line::from(dspan(
                format!("   {}", provider.label()),
                Color::White,
            )));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(dspan(
        " Add more anytime with /login",
        Color::DarkGray,
    )));

    f.render_widget(paragraph(lines), inner);
}

fn draw_api_key_dialog(f: &mut Frame, provider: Provider, key_buf: &str, cursor_blink: bool) {
    let dialog = dialog_rect(56, 14, f.area());
    let title = match provider {
        Provider::Anthropic => " anthropic api key ",
        Provider::OpenAi => " openai api key ",
        _ => " api key ",
    };
    let block = dialog_block(title);
    let inner = block.inner(dialog);
    f.render_widget(Clear, dialog);
    f.render_widget(block, dialog);

    // Build masked key display: show prefix, mask rest, blinking cursor
    let display = if key_buf.is_empty() {
        if cursor_blink { "\u{2588}" } else { " " }.to_string() // block cursor
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

    let lines = vec![
        Line::from(""),
        Line::from(dspan(
            format!(" Get your key at {}", provider.key_url()),
            Color::DarkGray,
        )),
        Line::from(""),
        // The text field — uses a slightly lighter bg to look like an input
        Line::from(vec![
            dspan("  ", Color::White),
            Span::styled(
                format!(" {display:<40} "),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(""),
        Line::from(dspan(
            format!(
                " Prefix: {}     Stored in: OS keychain",
                provider.key_prefix()
            ),
            Color::DarkGray,
        )),
        Line::from(dspan(
            format!(" Or set {} in your shell", provider.env_var_name()),
            Color::DarkGray,
        )),
        Line::from(""),
        Line::from(""),
        Line::from(vec![
            dspan_bold(" Enter ", Color::Green),
            dspan("confirm  ", Color::DarkGray),
            dspan_bold(" Esc ", Color::Yellow),
            dspan("skip", Color::DarkGray),
        ]),
    ];

    f.render_widget(paragraph(lines), inner);
}

fn draw_security_note(f: &mut Frame) {
    let dialog = dialog_rect(54, 14, f.area());
    let block = dialog_block(" before you start ");
    let inner = block.inner(dialog);
    f.render_widget(Clear, dialog);
    f.render_widget(block, dialog);

    let lines = vec![
        Line::from(""),
        Line::from(dspan(
            " One can read, write, and execute code.",
            Color::Yellow,
        )),
        Line::from(""),
        Line::from(dspan(
            " \u{2022} Tool calls require your approval",
            Color::DarkGray,
        )),
        Line::from(dspan(
            " \u{2022} Shell commands run in your terminal",
            Color::DarkGray,
        )),
        Line::from(dspan(
            " \u{2022} Files must be read before editing",
            Color::DarkGray,
        )),
        Line::from(dspan(
            " \u{2022} Credentials stay in your OS keychain",
            Color::DarkGray,
        )),
        Line::from(""),
        Line::from(dspan(
            " /permissions to configure  |  /help for commands",
            Color::DarkGray,
        )),
        Line::from(""),
        Line::from(""),
        Line::from(dspan_bold(" Press Enter to start", Color::Green)),
    ];

    f.render_widget(paragraph(lines), inner);
}

fn draw_done(f: &mut Frame) {
    let dialog = dialog_rect(36, 6, f.area());
    let block = dialog_block(" ready ");
    let inner = block.inner(dialog);
    f.render_widget(Clear, dialog);
    f.render_widget(block, dialog);

    let lines = vec![
        Line::from(""),
        Line::from(dspan_bold(" Starting One...", Color::Green)),
    ];

    f.render_widget(paragraph(lines), inner);
}

// ── Helpers ───────────────────────────────────────────────────────

/// Build a styled span.
fn dspan(text: impl Into<String>, fg: Color) -> Span<'static> {
    Span::styled(text.into(), Style::default().fg(fg))
}

/// Bold variant.
fn dspan_bold(text: impl Into<String>, fg: Color) -> Span<'static> {
    Span::styled(
        text.into(),
        Style::default().fg(fg).add_modifier(Modifier::BOLD),
    )
}

fn paragraph(lines: Vec<Line<'static>>) -> Paragraph<'static> {
    Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false })
}

fn dialog_block(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .title(title)
        .title_alignment(Alignment::Center)
        .border_style(Style::default().fg(Color::Rgb(0x33, 0xCC, 0xFF)))
}

/// Create a centered dialog rectangle with fixed character dimensions.
fn dialog_rect(width: u16, height: u16, area: Rect) -> Rect {
    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(height.min(area.height)),
            Constraint::Min(0),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(width.min(area.width)),
            Constraint::Min(0),
        ])
        .split(vert[1])[1]
}
