use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

/// A single suggestion entry (command or file).
#[derive(Clone, Debug)]
pub struct Suggestion {
    pub name: String,
    pub description: String,
    pub kind: SuggestionKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SuggestionKind {
    Command,
    File,
    Directory,
}

/// All slash commands: built-in + custom skills.
fn all_commands(project_dir: &str) -> Vec<Suggestion> {
    let mut cmds = vec![
        cs("/help", "Show available commands"),
        cs("/clear", "Clear the current conversation"),
        cs("/compact", "Compact conversation context"),
        cs("/model", "Switch model (opus, sonnet, haiku)"),
        cs("/cost", "Show token usage and cost"),
        cs("/config", "Show current configuration"),
        cs("/version", "Show version"),
        cs("/new", "Open a new project session"),
        cs("/close", "Close the current session"),
        cs("/switch", "Switch to a session by name"),
        cs("/session", "List active sessions"),
        cs("/login", "Login to a provider"),
        cs("/logout", "Remove stored credentials"),
        cs("/provider", "Show providers and auth status"),
        cs("/pet", "Show or manage your pet"),
        cs("/inbox", "Show notification count"),
        cs("/status", "Show connection and provider status"),
        cs("/plugin", "List installed plugins"),
        cs("/history", "Browse previous sessions"),
        cs("/effort", "Set reasoning effort (low/medium/high/max/auto)"),
        cs("/fast", "Toggle fast mode"),
        cs("/diff", "Show git diff summary"),
        cs("/git", "Run a git command"),
        cs("/doctor", "Check system health"),
        cs("/bug", "Report an issue"),
        cs("/plan", "Toggle plan mode"),
        cs("/debug", "Toggle debug mode (show background activity)"),
        cs("/permissions", "Show permission settings"),
        cs("/mcp", "Show MCP server connections"),
        cs("/memory", "List or search memories"),
        cs("/remember", "Save a quick memory"),
        cs("/tasks", "List or manage tasks"),
        cs("/tools", "List available tools"),
        cs("/pr", "Create a pull request (AI-guided)"),
        cs("/commit", "Create a git commit (AI-guided)"),
    ];

    // Add custom skills from project/user directories
    let skills = one_core::skills::load_skills(project_dir);
    for skill in skills {
        cmds.push(Suggestion {
            name: format!("/{}", skill.name),
            description: skill.description,
            kind: SuggestionKind::Command,
        });
    }

    cmds
}

fn cs(name: &str, desc: &str) -> Suggestion {
    Suggestion {
        name: name.into(),
        description: desc.into(),
        kind: SuggestionKind::Command,
    }
}

/// Manages the autocomplete popup for both slash commands and @ file mentions.
pub struct Autocomplete {
    pub suggestions: Vec<Suggestion>,
    pub selected: usize,
    pub visible: bool,
    /// The @ token being completed (position in input where @ starts).
    at_token_start: Option<usize>,
}

impl Autocomplete {
    pub fn new() -> Self {
        Self {
            suggestions: Vec::new(),
            selected: 0,
            visible: false,
            at_token_start: None,
        }
    }

    /// Update suggestions based on current input text.
    pub fn update(&mut self, input: &str) {
        self.update_with_context(input, ".", ".");
    }

    /// Update suggestions with project directory and cwd.
    pub fn update_with_context(&mut self, input: &str, project_dir: &str, cwd: &str) {
        let trimmed = input.trim();

        // Check for @ file mention anywhere in input
        if let Some(at_info) = find_at_token(input) {
            self.at_token_start = Some(at_info.start);
            let query = &at_info.query;
            self.suggestions = file_suggestions(query, cwd);
            self.visible = !self.suggestions.is_empty();
            if self.selected >= self.suggestions.len() {
                self.selected = 0;
            }
            return;
        }
        self.at_token_start = None;

        // Slash command completion
        if !trimmed.starts_with('/') || trimmed.contains(' ') {
            self.visible = false;
            self.suggestions.clear();
            self.selected = 0;
            return;
        }

        let prefix = trimmed.to_lowercase();
        self.suggestions = all_commands(project_dir)
            .into_iter()
            .filter(|cmd| cmd.name.starts_with(&prefix))
            .collect();

        self.visible = !self.suggestions.is_empty();
        if self.selected >= self.suggestions.len() {
            self.selected = self.suggestions.len().saturating_sub(1);
        }
    }

    pub fn select_next(&mut self) {
        if !self.suggestions.is_empty() {
            self.selected = (self.selected + 1) % self.suggestions.len();
        }
    }

    pub fn select_prev(&mut self) {
        if !self.suggestions.is_empty() {
            if self.selected == 0 {
                self.selected = self.suggestions.len() - 1;
            } else {
                self.selected -= 1;
            }
        }
    }

    /// Accept the currently highlighted suggestion.
    /// For commands: returns the command name (e.g. "/help").
    /// For files: returns the full input with the @ token replaced.
    pub fn accept_with_input(&mut self, current_input: &str) -> Option<String> {
        if !self.visible || self.suggestions.is_empty() {
            return None;
        }
        let suggestion = &self.suggestions[self.selected];

        let result = if let Some(at_start) = self.at_token_start {
            // Replace the @token in the input with the completed path
            let before = &current_input[..at_start];
            let suffix = if suggestion.kind == SuggestionKind::Directory {
                "/" // keep typing into subdirectory
            } else {
                " " // file complete, add space
            };
            Some(format!("{before}@{}{suffix}", suggestion.name))
        } else {
            // Slash command
            Some(suggestion.name.clone())
        };

        self.visible = false;
        self.suggestions.clear();
        self.selected = 0;
        self.at_token_start = None;
        result
    }

    /// Legacy accept (for slash commands only).
    pub fn accept(&mut self) -> Option<String> {
        if !self.visible || self.suggestions.is_empty() {
            return None;
        }
        let name = self.suggestions[self.selected].name.clone();
        self.visible = false;
        self.suggestions.clear();
        self.selected = 0;
        self.at_token_start = None;
        Some(name)
    }

    /// Render the autocomplete popup below the input area (CC style).
    pub fn render(&self, f: &mut Frame, input_area: Rect) {
        if !self.visible || self.suggestions.is_empty() {
            return;
        }

        let max_items = 8;
        let show_count = self.suggestions.len().min(max_items);
        let height = show_count as u16 + 2; // +2 for border
        let width = input_area.width;

        let is_file_mode = self.at_token_start.is_some();

        // Place popup below the input area (CC style)
        let y = input_area.y + input_area.height;
        let available_below = f.area().height.saturating_sub(y);

        // If not enough space below, place above
        let (popup_y, popup_height) = if available_below >= height {
            (y, height)
        } else if input_area.y >= height {
            (input_area.y.saturating_sub(height), height)
        } else {
            // Constrain to available space
            let h = available_below.max(3);
            (y, h)
        };

        let popup_area = Rect::new(input_area.x, popup_y, width, popup_height);
        f.render_widget(Clear, popup_area);

        let lines: Vec<Line> = self
            .suggestions
            .iter()
            .take(max_items)
            .enumerate()
            .map(|(i, s)| {
                let is_selected = i == self.selected;
                let (fg, bg, modifier) = if is_selected {
                    (Color::Black, Color::Cyan, Modifier::BOLD)
                } else {
                    (Color::White, Color::Reset, Modifier::empty())
                };

                let icon = match s.kind {
                    // Commands already start with '/' — no separate icon needed
                    SuggestionKind::Command => "",
                    SuggestionKind::File => "+",
                    SuggestionKind::Directory => "+",
                };

                let name_display = if s.name.len() > 40 {
                    format!("{}...", &s.name[..37])
                } else {
                    s.name.clone()
                };

                let suffix = if s.kind == SuggestionKind::Directory {
                    "/"
                } else {
                    ""
                };

                let entry = if icon.is_empty() {
                    format!(" {name_display}{suffix}")
                } else {
                    format!(" {icon} {name_display}{suffix}")
                };
                Line::from(vec![
                    Span::styled(entry, Style::default().fg(fg).bg(bg).add_modifier(modifier)),
                    Span::styled(
                        if s.description.is_empty() {
                            String::new()
                        } else {
                            format!("  {}", s.description)
                        },
                        Style::default()
                            .fg(if is_selected {
                                Color::Black
                            } else {
                                Color::DarkGray
                            })
                            .bg(bg),
                    ),
                ])
            })
            .collect();

        let title = if is_file_mode {
            " files "
        } else {
            " commands "
        };
        let more = if self.suggestions.len() > max_items {
            format!(" +{} more ", self.suggestions.len() - max_items)
        } else {
            String::new()
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(title)
            .title_bottom(more);

        let paragraph = Paragraph::new(lines).block(block);
        f.render_widget(paragraph, popup_area);
    }
}

impl Default for Autocomplete {
    fn default() -> Self {
        Self::new()
    }
}

// ── @ token detection ────────────────────────────────────────────

struct AtToken {
    /// Byte offset in input where @ starts
    start: usize,
    /// The query text after @ (may be empty)
    query: String,
}

/// Find the last @ token in the input that's eligible for completion.
/// Returns None if no @ is found or if it's inside a word.
fn find_at_token(input: &str) -> Option<AtToken> {
    // Find the last @ that's either at the start or preceded by whitespace
    let bytes = input.as_bytes();
    for (i, &b) in bytes.iter().enumerate().rev() {
        if b == b'@' {
            // Check it's at start or after whitespace
            if i == 0 || bytes[i - 1] == b' ' || bytes[i - 1] == b'\t' {
                let after = &input[i + 1..];
                // Stop at whitespace (the query is what's between @ and cursor)
                let query = after
                    .split(|c: char| c.is_whitespace())
                    .next()
                    .unwrap_or("");
                return Some(AtToken {
                    start: i,
                    query: query.to_string(),
                });
            }
        }
    }
    None
}

// ── File suggestion generation ───────────────────────────────────

/// Generate file suggestions for a query. Uses git ls-files if available,
/// falls back to directory listing.
fn file_suggestions(query: &str, cwd: &str) -> Vec<Suggestion> {
    let max_results = 20;

    // Try git ls-files first (fast, respects .gitignore)
    let files = git_files(cwd).unwrap_or_else(|| dir_files(cwd));

    let lower_query = query.to_lowercase();

    let mut results: Vec<Suggestion> = files
        .into_iter()
        .filter(|(name, _is_dir)| {
            if query.is_empty() {
                true
            } else {
                // Prefix match on path or filename
                let lower = name.to_lowercase();
                lower.starts_with(&lower_query)
                    || {
                        // Also match on filename component
                        let fname = std::path::Path::new(&lower)
                            .file_name()
                            .and_then(|f| f.to_str())
                            .unwrap_or("");
                        fname.starts_with(&lower_query)
                    }
                    || {
                        // Fuzzy: query chars appear in order
                        fuzzy_match(&lower, &lower_query)
                    }
            }
        })
        .take(max_results)
        .map(|(name, is_dir)| Suggestion {
            name,
            description: String::new(),
            kind: if is_dir {
                SuggestionKind::Directory
            } else {
                SuggestionKind::File
            },
        })
        .collect();

    // Sort: directories first, then alphabetical
    results.sort_by(|a, b| {
        let dir_ord =
            (b.kind == SuggestionKind::Directory).cmp(&(a.kind == SuggestionKind::Directory));
        dir_ord.then(a.name.cmp(&b.name))
    });

    results
}

/// Simple fuzzy match: all query chars appear in order in the target.
fn fuzzy_match(target: &str, query: &str) -> bool {
    let mut target_chars = target.chars();
    for qc in query.chars() {
        if !target_chars.any(|tc| tc == qc) {
            return false;
        }
    }
    true
}

/// Get tracked files via `git ls-files`.
fn git_files(cwd: &str) -> Option<Vec<(String, bool)>> {
    let output = std::process::Command::new("git")
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .current_dir(cwd)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results: Vec<(String, bool)> = Vec::new();
    let mut seen_dirs = std::collections::HashSet::new();

    for line in stdout.lines() {
        if line.is_empty() {
            continue;
        }
        // Add the file
        results.push((line.to_string(), false));
        // Add parent directories we haven't seen yet
        let path = std::path::Path::new(line);
        if let Some(parent) = path.parent() {
            let parent_str = parent.to_string_lossy().to_string();
            if !parent_str.is_empty() && seen_dirs.insert(parent_str.clone()) {
                results.push((parent_str, true));
            }
        }
    }

    Some(results)
}

/// Fallback: list files in cwd directory.
fn dir_files(cwd: &str) -> Vec<(String, bool)> {
    let mut results = Vec::new();
    if let Ok(entries) = std::fs::read_dir(cwd) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue; // skip dotfiles
            }
            let is_dir = entry.path().is_dir();
            results.push((name, is_dir));
        }
    }
    results.sort();
    results
}
