//! TUI rendering primitives.
//!
//! Each function renders exactly one visual concept and returns styled Lines.
//! `draw_messages()` in app.rs orchestrates these calls; it contains no
//! rendering logic itself.
//!
//! Visual grammar:
//!
//!   ⏺  assistant text …        ← static dot prefix on first line
//!   · Bash(cmd)                 ← animated dot while tool runs
//!   ⏺ Bash(cmd)                 ← static dot when complete
//!     ⎿  output line            ← indented result
//!     ⎿  … +N more lines
//!   ⠒ debug event              ← muted debug line
//!   ⠹ Thinking…                ← status: processing / active

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

// ── Constants ─────────────────────────────────────────────────────────────────

pub const STATIC_DOT: &str = "⏺ ";
pub const OUTPUT_PREFIX: &str = "  ⎿  ";
pub const DEBUG_PREFIX: &str = "  ⠒ ";
pub const MORE_LINES: &str = "⠒";
const MAX_OUTPUT_LINES: usize = 5;
const MAX_LINE_CHARS: usize = 120;

// ── Spinner frames ────────────────────────────────────────────────────────────

const GROWING: &[&str] = &["\u{00b7}", "\u{2022}", "\u{25cf}", "\u{2022}", "\u{00b7}"];
const FALLING_SAND: &[&str] = &[
    "⠁", "⠂", "⠄", "⡀", "⡈", "⡐", "⡠", "⣀", "⣁", "⣂", "⣄", "⣌", "⣔", "⣤", "⣥", "⣦", "⣮", "⣶", "⣷",
    "⣿", "⡿", "⠿", "⢟", "⠟", "⡛", "⠛", "⠫", "⢋", "⠋", "⠍", "⡉", "⠉", "⠑", "⠡", "⢁",
];
const FOLD: &[&str] = &["-", "≻", "›", "⟩", "|", "⟨", "‹", "≺"];
const BOX_BOUNCE: &[&str] = &["▖", "▘", "▝", "▗"];
const BRAILLE: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Return one spinner character for `style` at animation frame `tick`.
pub fn spinner_char(style: usize, tick: usize) -> &'static str {
    let frames: &[&str] = match style % 5 {
        0 => GROWING,
        1 => FALLING_SAND,
        2 => FOLD,
        3 => BOX_BOUNCE,
        _ => BRAILLE,
    };
    frames[tick % frames.len()]
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn truncate(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        s.to_string()
    } else {
        chars[..max].iter().collect::<String>() + "…"
    }
}

fn out_line(text: impl Into<String>) -> Line<'static> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    Line::from(vec![
        Span::styled(OUTPUT_PREFIX, dim),
        Span::styled(text.into(), dim),
    ])
}

fn more_line(hidden: usize) -> Line<'static> {
    out_line(format!("… +{hidden} more lines"))
}

// ── Turn renderers ────────────────────────────────────────────────────────────

/// `> User message` — bold white, blank lines before and after.
pub fn user_turn(content: &str) -> Vec<Line<'static>> {
    let style = Style::default()
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);
    content
        .lines()
        .map(|line| Line::from(Span::styled(format!("> {line}"), style)))
        .collect()
}

/// `⏺ Markdown content` — dot on first line, rendered markdown below.
pub fn assistant_text(content: &str, dot: Span<'static>) -> Vec<Line<'static>> {
    let md_lines = crate::markdown::render_markdown(content);
    md_lines
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            if i == 0 {
                let mut spans = vec![dot.clone()];
                spans.extend(line.spans);
                Line::from(spans)
            } else {
                line
            }
        })
        .collect()
}

/// System message — dim gray, no decoration.
pub fn system_turn(content: &str) -> Vec<Line<'static>> {
    content
        .lines()
        .map(|l| {
            Line::from(Span::styled(
                l.to_string(),
                Style::default().fg(Color::DarkGray),
            ))
        })
        .collect()
}

/// ToolResult turn — `⎿  content`, 5-line limit, 120-char truncation per line.
pub fn tool_result_turn(content: &str) -> Vec<Line<'static>> {
    if content.is_empty() {
        return Vec::new();
    }
    let total = content.lines().count();
    let mut lines: Vec<Line<'static>> = content
        .lines()
        .take(MAX_OUTPUT_LINES)
        .map(|l| out_line(truncate(l, MAX_LINE_CHARS)))
        .collect();
    if total > MAX_OUTPUT_LINES {
        lines.push(more_line(total - MAX_OUTPUT_LINES));
    }
    lines
}

/// Muted `⠒ message` debug line.
pub fn debug_event_line(message: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!("{DEBUG_PREFIX}{message}"),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    ))
}

// ── Tool call renderers ───────────────────────────────────────────────────────

/// Tool header line: animated dot while running, static `⏺` when done.
///
/// ```text
/// · Bash(cargo test)      ← running
/// ⏺ Bash(cargo test)      ← complete
/// ```
pub fn tool_header(
    tool_name: &str,
    input_summary: &str,
    is_running: bool,
    dot_style: usize,
    tick: usize,
    color: Color,
    static_dot: Span<'static>,
) -> Line<'static> {
    let face = match tool_name {
        "Edit" => "Update",
        "Glob" => "Search",
        other => other,
    };
    let label = if input_summary.is_empty() {
        face.to_string()
    } else {
        format!("{face}({input_summary})")
    };

    let dot = if is_running {
        Span::styled(
            format!("{} ", spinner_char(dot_style, tick)),
            Style::default().fg(color).add_modifier(Modifier::DIM),
        )
    } else {
        static_dot
    };

    Line::from(vec![
        dot,
        Span::styled(label, Style::default().add_modifier(Modifier::BOLD)),
    ])
}

/// Tool output lines below the header.
///
/// Returns the appropriate summary or truncated content depending on tool type.
pub fn tool_output(tool_name: &str, output: &str, is_error: bool) -> Vec<Line<'static>> {
    if output.trim().is_empty() {
        return vec![out_line("(No output)")];
    }

    if is_error {
        let first = output.lines().next().unwrap_or("Error");
        return vec![Line::from(vec![
            Span::styled(OUTPUT_PREFIX, Style::default().add_modifier(Modifier::DIM)),
            Span::styled(
                truncate(first, MAX_LINE_CHARS),
                Style::default().fg(Color::Red),
            ),
        ])];
    }

    match tool_name {
        "Read" => {
            let count = output.lines().count();
            vec![out_line(format!("{count} lines"))]
        }
        "Write" => {
            let first = output.lines().next().unwrap_or("Done");
            vec![out_line(first)]
        }
        "Bash" => {
            let lines: Vec<&str> = output.lines().collect();
            if lines.is_empty() {
                return vec![out_line("(No output)")];
            }
            let mut result: Vec<Line<'static>> = lines
                .iter()
                .take(MAX_OUTPUT_LINES)
                .map(|l| out_line(truncate(l, MAX_LINE_CHARS)))
                .collect();
            if lines.len() > MAX_OUTPUT_LINES {
                result.push(more_line(lines.len() - MAX_OUTPUT_LINES));
            }
            result
        }
        "Grep" | "Glob" => {
            let count = output.lines().count();
            let unit = if tool_name == "Grep" {
                "results"
            } else {
                "files"
            };
            vec![out_line(format!("Found {count} {unit}"))]
        }
        "Edit" | "Update" => {
            let mut iter = output.lines();
            let mut result = Vec::new();
            if let Some(summary) = iter.next() {
                result.push(out_line(summary));
            }
            let diff: Vec<&str> = iter.collect();
            for line in diff.iter().take(MAX_OUTPUT_LINES) {
                let marker = line.get(6..8).unwrap_or("  ");
                let style = match marker {
                    " +" => Style::default().fg(Color::Green),
                    " -" => Style::default().fg(Color::Red),
                    _ => Style::default().add_modifier(Modifier::DIM),
                };
                result.push(Line::from(Span::styled(format!("      {line}"), style)));
            }
            if diff.len() > MAX_OUTPUT_LINES {
                result.push(more_line(diff.len() - MAX_OUTPUT_LINES));
            }
            result
        }
        "web_fetch" => {
            let chars = output.chars().count();
            let display = if chars < 10_000 {
                format!("{chars}")
            } else if chars < 1_000_000 {
                format!("{:.1}k", chars as f64 / 1000.0)
            } else {
                format!("{:.1}M", chars as f64 / 1_000_000.0)
            };
            vec![out_line(format!("Fetched {display} chars"))]
        }
        "web_search" => {
            let count = output.lines().filter(|l| l.starts_with("URL:")).count();
            if count > 0 {
                vec![out_line(format!("Found {count} result(s)"))]
            } else {
                vec![out_line(output.lines().next().unwrap_or("No results"))]
            }
        }
        _ => {
            let first = output.lines().next().unwrap_or("Done");
            vec![out_line(truncate(first, MAX_LINE_CHARS))]
        }
    }
}

/// Tool still running — yellow `⎿  running…`.
pub fn tool_running_line() -> Line<'static> {
    Line::from(Span::styled(
        format!("{OUTPUT_PREFIX}running\u{2026}"),
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::DIM),
    ))
}

// ── Status line renderers ─────────────────────────────────────────────────────

/// Minimal processing status — just spinner + verb.
///
/// ```text
/// ⠹ Thinking…
/// ```
pub fn status_processing(spinner: &str, verb: &str, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{spinner} "),
            Style::default().fg(color).add_modifier(Modifier::DIM),
        ),
        Span::styled(
            format!("{verb}\u{2026}"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
    ])
}

/// Full active status — spinner + verb + elapsed + tokens + tip.
///
/// ```text
/// ⠹ Wrangling… (12s · ↓ 4.1k tokens · high effort)
///   ⎿  Tip: Use /compact to save context
/// ```
pub fn status_active(
    spinner: &str,
    verb: &str,
    elapsed: &str,
    tokens: &str,
    effort_suffix: &str,
    tip: &str,
    color: Color,
) -> Vec<Line<'static>> {
    vec![
        Line::from(vec![
            Span::styled(
                format!("{spinner} "),
                Style::default().fg(color).add_modifier(Modifier::DIM),
            ),
            Span::styled(
                format!("{verb}\u{2026}"),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" ({elapsed} \u{00b7} {tokens}{effort_suffix})"),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                OUTPUT_PREFIX.to_string(),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(format!("Tip: {tip}"), Style::default().fg(Color::DarkGray)),
        ]),
    ]
}
