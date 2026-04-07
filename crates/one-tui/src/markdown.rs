use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// Parse markdown text into styled ratatui Lines for TUI rendering.
/// Handles: headers, bold, italic, code spans, code blocks, lists, links, tables, rules.
pub fn render_markdown(text: &str) -> Vec<Line<'static>> {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    let parser = Parser::new_ext(text, opts);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::new();
    let mut style_stack: Vec<Style> = vec![Style::default().fg(Color::White)];
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut list_depth: usize = 0;
    // Table state
    let mut in_table = false;
    let mut table_row: Vec<String> = Vec::new();
    let mut table_col_widths: Vec<usize> = Vec::new();
    let mut table_rows: Vec<Vec<String>> = Vec::new();
    let mut _is_table_head = false;

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    flush_line(&mut lines, &mut current_spans);
                    let color = match level {
                        pulldown_cmark::HeadingLevel::H1 => Color::Cyan,
                        pulldown_cmark::HeadingLevel::H2 => Color::Green,
                        _ => Color::Yellow,
                    };
                    style_stack.push(Style::default().fg(color).add_modifier(Modifier::BOLD));
                }
                Tag::Strong => {
                    style_stack.push(current_style(&style_stack).add_modifier(Modifier::BOLD));
                }
                Tag::Emphasis => {
                    style_stack.push(current_style(&style_stack).add_modifier(Modifier::ITALIC));
                }
                Tag::Strikethrough => {
                    style_stack.push(
                        current_style(&style_stack)
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::CROSSED_OUT),
                    );
                }
                Tag::BlockQuote(_) => {
                    flush_line(&mut lines, &mut current_spans);
                    style_stack.push(
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    );
                }
                Tag::CodeBlock(kind) => {
                    flush_line(&mut lines, &mut current_spans);
                    in_code_block = true;
                    code_lang = match kind {
                        pulldown_cmark::CodeBlockKind::Fenced(lang) => lang.to_string(),
                        _ => String::new(),
                    };
                    style_stack.push(Style::default().fg(Color::Gray));
                    // Code block separator
                    lines.push(Line::from(Span::styled(
                        "  ┌─────────────────────────────────────",
                        Style::default().fg(Color::DarkGray),
                    )));
                }
                Tag::List(_) => {
                    list_depth += 1;
                }
                Tag::Item => {
                    flush_line(&mut lines, &mut current_spans);
                    let indent = "  ".repeat(list_depth);
                    current_spans.push(Span::styled(
                        format!("{indent}• "),
                        Style::default().fg(Color::Cyan),
                    ));
                }
                Tag::Link { dest_url, .. } => {
                    style_stack.push(
                        Style::default()
                            .fg(Color::Blue)
                            .add_modifier(Modifier::UNDERLINED),
                    );
                    // Store URL for after the link text
                    // We'll just style the text as a link
                    let _ = dest_url; // URL available but not rendered in TUI
                }
                Tag::Paragraph => {
                    flush_line(&mut lines, &mut current_spans);
                }
                Tag::Table(_alignments) => {
                    flush_line(&mut lines, &mut current_spans);
                    in_table = true;
                    table_rows.clear();
                    table_col_widths.clear();
                }
                Tag::TableHead => {
                    _is_table_head = true;
                    table_row = Vec::new();
                }
                Tag::TableRow => {
                    table_row = Vec::new();
                }
                Tag::TableCell => {
                    // Will collect text in the Text handler
                }
                _ => {}
            },

            Event::End(tag_end) => match tag_end {
                TagEnd::Heading(_) => {
                    style_stack.pop();
                    flush_line(&mut lines, &mut current_spans);
                    lines.push(Line::from(""));
                }
                TagEnd::Strong | TagEnd::Emphasis | TagEnd::Link | TagEnd::Strikethrough => {
                    style_stack.pop();
                }
                TagEnd::BlockQuote(_) => {
                    style_stack.pop();
                    flush_line(&mut lines, &mut current_spans);
                }
                TagEnd::CodeBlock => {
                    in_code_block = false;
                    style_stack.pop();
                    lines.push(Line::from(Span::styled(
                        "  └─────────────────────────────────────",
                        Style::default().fg(Color::DarkGray),
                    )));
                }
                TagEnd::List(_) => {
                    list_depth = list_depth.saturating_sub(1);
                }
                TagEnd::Item => {
                    flush_line(&mut lines, &mut current_spans);
                }
                TagEnd::Paragraph => {
                    flush_line(&mut lines, &mut current_spans);
                    lines.push(Line::from(""));
                }
                TagEnd::TableCell => {
                    // Collect the current spans as cell text
                    let cell_text: String = current_spans
                        .drain(..)
                        .map(|s| s.content.to_string())
                        .collect::<String>()
                        .trim()
                        .replace("  ", "")
                        .to_string();
                    table_row.push(cell_text);
                }
                TagEnd::TableHead => {
                    // Track column widths from header
                    for (i, cell) in table_row.iter().enumerate() {
                        if i >= table_col_widths.len() {
                            table_col_widths.push(cell.len().max(3));
                        } else {
                            table_col_widths[i] = table_col_widths[i].max(cell.len());
                        }
                    }
                    table_rows.push(table_row.clone());
                    _is_table_head = false;
                }
                TagEnd::TableRow => {
                    // Update column widths
                    for (i, cell) in table_row.iter().enumerate() {
                        if i >= table_col_widths.len() {
                            table_col_widths.push(cell.len().max(3));
                        } else {
                            table_col_widths[i] = table_col_widths[i].max(cell.len());
                        }
                    }
                    table_rows.push(table_row.clone());
                }
                TagEnd::Table => {
                    in_table = false;
                    // Render the collected table
                    render_table(&table_rows, &table_col_widths, &mut lines);
                    table_rows.clear();
                    table_col_widths.clear();
                }
                _ => {}
            },

            Event::Text(text) => {
                // In table cells, just accumulate text as a plain span
                if in_table {
                    current_spans.push(Span::raw(text.to_string()));
                    continue;
                }

                let style = current_style(&style_stack);
                let prefix = if in_code_block { "  │ " } else { "  " };

                for (i, line) in text.lines().enumerate() {
                    if i > 0 {
                        flush_line(&mut lines, &mut current_spans);
                    }
                    if current_spans.is_empty() {
                        current_spans.push(Span::styled(
                            prefix.to_string(),
                            Style::default().fg(Color::DarkGray),
                        ));
                    }
                    if in_code_block && !code_lang.is_empty() {
                        // Syntax-highlighted code
                        highlight_code_line(line, &code_lang, &mut current_spans);
                    } else {
                        current_spans.push(Span::styled(line.to_string(), style));
                    }
                }
            }

            Event::Code(code) => {
                current_spans.push(Span::styled(
                    format!("`{code}`"),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::DIM),
                ));
            }

            Event::SoftBreak | Event::HardBreak => {
                flush_line(&mut lines, &mut current_spans);
            }

            Event::Rule => {
                flush_line(&mut lines, &mut current_spans);
                lines.push(Line::from(Span::styled(
                    "  ────────────────────────────────────────",
                    Style::default().fg(Color::DarkGray),
                )));
            }

            _ => {}
        }
    }

    flush_line(&mut lines, &mut current_spans);
    lines
}

/// Render a markdown table with aligned columns and box-drawing borders.
fn render_table(rows: &[Vec<String>], col_widths: &[usize], lines: &mut Vec<Line<'static>>) {
    if rows.is_empty() || col_widths.is_empty() {
        return;
    }

    let border_style = Style::default().fg(Color::DarkGray);
    let header_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let cell_style = Style::default().fg(Color::White);

    // Top border: ┌───┬───┐
    let top = format!(
        "  ┌{}┐",
        col_widths
            .iter()
            .map(|w| "─".repeat(w + 2))
            .collect::<Vec<_>>()
            .join("┬")
    );
    lines.push(Line::from(Span::styled(top, border_style)));

    for (row_idx, row) in rows.iter().enumerate() {
        let style = if row_idx == 0 {
            header_style
        } else {
            cell_style
        };

        // Build cell spans: │ cell │ cell │
        let mut spans = vec![Span::styled("  │", border_style)];
        for (col_idx, cell) in row.iter().enumerate() {
            let width = col_widths.get(col_idx).copied().unwrap_or(3);
            let padded = format!(" {:<width$} ", cell, width = width);
            spans.push(Span::styled(padded, style));
            spans.push(Span::styled("│", border_style));
        }
        // Fill missing columns
        for width in col_widths.iter().skip(row.len()) {
            spans.push(Span::styled(" ".repeat(width + 2), cell_style));
            spans.push(Span::styled("│", border_style));
        }
        lines.push(Line::from(spans));

        // After header row, add separator: ├───┼───┤
        if row_idx == 0 {
            let sep = format!(
                "  ├{}┤",
                col_widths
                    .iter()
                    .map(|w| "─".repeat(w + 2))
                    .collect::<Vec<_>>()
                    .join("┼")
            );
            lines.push(Line::from(Span::styled(sep, border_style)));
        }
    }

    // Bottom border: └───┴───┘
    let bottom = format!(
        "  └{}┘",
        col_widths
            .iter()
            .map(|w| "─".repeat(w + 2))
            .collect::<Vec<_>>()
            .join("┴")
    );
    lines.push(Line::from(Span::styled(bottom, border_style)));
}

/// Lightweight syntax highlighting for code blocks.
/// Colors keywords, strings, comments, and numbers by language.
fn highlight_code_line(line: &str, lang: &str, spans: &mut Vec<Span<'static>>) {
    let keywords = match lang.to_lowercase().as_str() {
        "rust" | "rs" => &[
            "fn", "let", "mut", "const", "pub", "use", "mod", "struct", "enum", "impl", "trait",
            "type", "where", "for", "while", "loop", "if", "else", "match", "return", "break",
            "continue", "self", "Self", "super", "crate", "async", "await", "move", "ref", "true",
            "false", "Some", "None", "Ok", "Err",
        ][..],
        "javascript" | "js" | "typescript" | "ts" | "tsx" | "jsx" => &[
            "function",
            "const",
            "let",
            "var",
            "return",
            "if",
            "else",
            "for",
            "while",
            "class",
            "extends",
            "import",
            "export",
            "from",
            "default",
            "async",
            "await",
            "try",
            "catch",
            "throw",
            "new",
            "this",
            "true",
            "false",
            "null",
            "undefined",
            "typeof",
            "instanceof",
            "yield",
            "interface",
            "type",
            "enum",
        ][..],
        "python" | "py" => &[
            "def", "class", "return", "if", "elif", "else", "for", "while", "import", "from", "as",
            "with", "try", "except", "raise", "True", "False", "None", "self", "lambda", "yield",
            "async", "await", "pass", "break", "continue",
        ][..],
        "go" => &[
            "func",
            "var",
            "const",
            "type",
            "struct",
            "interface",
            "return",
            "if",
            "else",
            "for",
            "range",
            "switch",
            "case",
            "default",
            "package",
            "import",
            "defer",
            "go",
            "chan",
            "select",
            "true",
            "false",
            "nil",
            "map",
            "make",
            "append",
        ][..],
        _ => &[][..],
    };

    if keywords.is_empty() {
        // Unknown language — render as plain gray
        spans.push(Span::styled(
            line.to_string(),
            Style::default().fg(Color::Gray),
        ));
        return;
    }

    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];

    // Check for comments first (whole-line)
    if trimmed.starts_with("//") || trimmed.starts_with('#') && !lang.contains("rust") {
        spans.push(Span::styled(
            line.to_string(),
            Style::default().fg(Color::DarkGray),
        ));
        return;
    }

    // Tokenize and color
    spans.push(Span::styled(indent.to_string(), Style::default()));

    let mut remaining = trimmed;
    while !remaining.is_empty() {
        // String literals
        if remaining.starts_with('"') || remaining.starts_with('\'') {
            let quote = remaining.chars().next().unwrap();
            let end = remaining[1..]
                .find(quote)
                .map(|i| i + 2)
                .unwrap_or(remaining.len());
            spans.push(Span::styled(
                remaining[..end].to_string(),
                Style::default().fg(Color::Green),
            ));
            remaining = &remaining[end..];
            continue;
        }

        // Inline comments
        if remaining.starts_with("//") {
            spans.push(Span::styled(
                remaining.to_string(),
                Style::default().fg(Color::DarkGray),
            ));
            break;
        }

        // Numbers
        if remaining
            .chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
        {
            let end = remaining
                .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '_')
                .unwrap_or(remaining.len());
            spans.push(Span::styled(
                remaining[..end].to_string(),
                Style::default().fg(Color::Yellow),
            ));
            remaining = &remaining[end..];
            continue;
        }

        // Words (potential keywords)
        if remaining
            .chars()
            .next()
            .map(|c| c.is_alphanumeric() || c == '_')
            .unwrap_or(false)
        {
            let end = remaining
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(remaining.len());
            let word = &remaining[..end];

            let color = if keywords.contains(&word) {
                Color::Magenta
            } else {
                Color::Gray
            };

            spans.push(Span::styled(word.to_string(), Style::default().fg(color)));
            remaining = &remaining[end..];
            continue;
        }

        // Punctuation / operators
        let end = remaining
            .find(|c: char| c.is_alphanumeric() || c == '_' || c == '"' || c == '\'' || c == '/')
            .unwrap_or(remaining.len())
            .max(1);
        spans.push(Span::styled(
            remaining[..end].to_string(),
            Style::default().fg(Color::Gray),
        ));
        remaining = &remaining[end..];
    }
}

fn current_style(stack: &[Style]) -> Style {
    stack.last().copied().unwrap_or_default()
}

fn flush_line(lines: &mut Vec<Line<'static>>, spans: &mut Vec<Span<'static>>) {
    if !spans.is_empty() {
        lines.push(Line::from(std::mem::take(spans)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_heading() {
        let lines = render_markdown("# Hello");
        assert!(!lines.is_empty());
        let text: String = lines[0]
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert!(text.contains("Hello"));
    }

    #[test]
    fn test_render_code_block() {
        let lines = render_markdown("```rust\nfn main() {}\n```");
        // Should have top border, code line, bottom border
        assert!(lines.len() >= 3);
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.to_string())
            .collect();
        assert!(all_text.contains("main"));
    }

    #[test]
    fn test_render_table() {
        let md = "| Name | Value |\n|---|---|\n| foo | 42 |\n| bar | 99 |";
        let lines = render_markdown(md);
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.to_string())
            .collect();
        // Should contain table borders and cell content
        assert!(all_text.contains("foo"));
        assert!(all_text.contains("42"));
        assert!(all_text.contains("┌")); // top border
        assert!(all_text.contains("┘")); // bottom border
    }

    #[test]
    fn test_render_horizontal_rule() {
        let lines = render_markdown("above\n\n---\n\nbelow");
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.to_string())
            .collect();
        assert!(all_text.contains("───"));
    }

    #[test]
    fn test_render_list() {
        let lines = render_markdown("- item one\n- item two");
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.to_string())
            .collect();
        assert!(all_text.contains("•"));
        assert!(all_text.contains("item one"));
    }
}
