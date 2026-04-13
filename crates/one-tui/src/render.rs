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

/// Tool header — up to 2 wrapped lines, front-truncated if needed.
///
/// ```text
/// · Bash(cd /long/path && cargo test)    ← fits on one line
/// ⏺ Bash(cd /very/long/path/that/wraps
///         && cargo test --workspace)     ← two lines
/// ```
pub fn tool_header(
    tool_name: &str,
    input_summary: &str,
    is_running: bool,
    dot_style: usize,
    tick: usize,
    color: Color,
    static_dot: Span<'static>,
) -> Vec<Line<'static>> {
    let face = match tool_name {
        "Edit" => "Update",
        "Glob" => "Search",
        other => other,
    };

    let dot = if is_running {
        Span::styled(
            format!("{} ", spinner_char(dot_style, tick)),
            Style::default().fg(color).add_modifier(Modifier::DIM),
        )
    } else {
        static_dot
    };

    let bold = Style::default().add_modifier(Modifier::BOLD);
    const MAX_COLS: usize = 78;

    if input_summary.is_empty() {
        return vec![Line::from(vec![dot, Span::styled(face.to_string(), bold)])];
    }

    // Prefix: "ToolName(" — its char length determines space left for the arg
    let prefix = format!("{face}(");
    let suffix = ")";
    let prefix_len = prefix.chars().count() + 2; // 2 = dot + space
    let available = MAX_COLS.saturating_sub(prefix_len);

    let chars: Vec<char> = input_summary.chars().collect();

    if chars.len() <= available + MAX_COLS {
        // Fits in 1 or 2 lines — split at `available` chars
        if chars.len() <= available {
            vec![Line::from(vec![
                dot,
                Span::styled(format!("{prefix}{input_summary}{suffix}"), bold),
            ])]
        } else {
            // Line 1: prefix + first chunk
            let line1_arg: String = chars[..available].iter().collect();
            // Line 2: indented continuation — front-truncate if still too long
            let rest: String = chars[available..].iter().collect();
            let rest_max = MAX_COLS.saturating_sub(4); // 4 = "    " indent
            let rest_display = if rest.chars().count() > rest_max {
                let start = rest.chars().count() - rest_max;
                format!("…{}", rest.chars().skip(start).collect::<String>())
            } else {
                rest
            };
            vec![
                Line::from(vec![
                    dot,
                    Span::styled(format!("{prefix}{line1_arg}"), bold),
                ]),
                Line::from(Span::styled(format!("    {rest_display}{suffix}"), bold)),
            ]
        }
    } else {
        // Too long even for 2 lines — keep the tail (most recent part)
        let keep = available + MAX_COLS.saturating_sub(4);
        let start = chars.len().saturating_sub(keep);
        let truncated: String = chars[start..].iter().collect();
        let mid = available.min(truncated.chars().count());
        let line1_arg: String = truncated.chars().take(mid).collect();
        let rest: String = truncated.chars().skip(mid).collect();
        vec![
            Line::from(vec![
                dot,
                Span::styled(format!("{prefix}…{line1_arg}"), bold),
            ]),
            Line::from(Span::styled(format!("    {rest}{suffix}"), bold)),
        ]
    }
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

// ── Agent tree ────────────────────────────────────────────────────────────────

/// Render the parallel-agent tree shown while sub-agents are running.
///
/// ```text
/// ⏺ Running 3 agents…  (Ctrl+\ to collapse)
///    ├─ Explore codebase · 4 tool uses · 12.3k tokens
///    │  ⎿  Read: src/lib.rs
///    ├─ Find tests · 2 tool uses · 8.1k tokens
///    │  ⎿  Glob: **/*.test.rs
///    └─ Check CI · 1 tool use · 6.2k tokens
///       ⎿  Initializing…
/// ```
pub fn agent_tree(
    agents: &[one_core::session::AgentStatus],
    collapsed: bool,
) -> Vec<Line<'static>> {
    if agents.is_empty() {
        return Vec::new();
    }

    let running = agents.iter().filter(|a| !a.done).count();
    let total = agents.len();

    let header_text = if running == 0 {
        let total_tools: usize = agents.iter().map(|a| a.tool_uses).sum();
        let total_tokens: u64 = agents.iter().map(|a| a.tokens).sum();
        format!(
            "{} agent{} complete \u{00b7} {} tool use{} \u{00b7} {}",
            total,
            if total == 1 { "" } else { "s" },
            total_tools,
            if total_tools == 1 { "" } else { "s" },
            format_k(total_tokens),
        )
    } else if collapsed {
        format!(
            "Running {} agent{}\u{2026}  (Ctrl+\\ to expand)",
            running,
            if running == 1 { "" } else { "s" }
        )
    } else {
        format!(
            "Running {} agent{}\u{2026}  (Ctrl+\\ to collapse)",
            running,
            if running == 1 { "" } else { "s" }
        )
    };

    let mut lines: Vec<Line<'static>> = Vec::new();

    lines.push(Line::from(vec![
        Span::styled(
            STATIC_DOT.to_string(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            header_text,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    // Collapsed: only show the header line
    if collapsed {
        return lines;
    }

    for (i, agent) in agents.iter().enumerate() {
        let is_last = i == agents.len() - 1;
        let branch = if is_last {
            "   \u{2514}\u{2500} "
        } else {
            "   \u{251c}\u{2500} "
        };
        let pipe = if is_last { "      " } else { "   \u{2502}  " };

        // Metric suffix
        let token_str = format_k(agent.tokens);
        let uses_str = if agent.tool_uses == 1 {
            "1 tool use".to_string()
        } else {
            format!("{} tool uses", agent.tool_uses)
        };
        let metrics = format!(" \u{00b7} {} \u{00b7} {}", uses_str, token_str);

        let name_color = if agent.done {
            Color::DarkGray
        } else {
            Color::White
        };

        lines.push(Line::from(vec![
            Span::styled(branch.to_string(), Style::default().fg(Color::DarkGray)),
            Span::styled(agent.description.clone(), Style::default().fg(name_color)),
            Span::styled(metrics, Style::default().fg(Color::DarkGray)),
        ]));

        let action = agent
            .last_action
            .as_deref()
            .unwrap_or("Initializing\u{2026}");
        let action_text: String = action.chars().take(80).collect();

        lines.push(Line::from(vec![
            Span::styled(
                format!("{pipe}{OUTPUT_PREFIX}"),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ),
            Span::styled(
                action_text,
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ),
        ]));
    }

    lines
}

fn format_k(n: u64) -> String {
    if n == 0 {
        "0 tokens".to_string()
    } else if n < 1_000 {
        format!("{n} tokens")
    } else {
        format!("{:.1}k tokens", n as f64 / 1_000.0)
    }
}
