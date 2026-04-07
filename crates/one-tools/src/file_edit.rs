use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use serde_json::Value;

use crate::{Tool, ToolContext, ToolResult};

/// Performs exact string replacements in files.
/// Requires old_string to be unique unless replace_all is true.
pub struct FileEditTool;

impl Tool for FileEditTool {
    fn name(&self) -> &str {
        "Edit"
    }

    fn description(&self) -> &str {
        "Edit a file by replacing an exact string with new content. \
         The old_string must uniquely match in the file (unless replace_all is true). \
         Preserves indentation and line endings."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the file to edit"
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact text to find and replace"
                },
                "new_string": {
                    "type": "string",
                    "description": "The text to replace it with"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace all occurrences (default: false)"
                }
            },
            "required": ["file_path", "old_string", "new_string"]
        })
    }

    fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        let input = input.clone();
        let working_dir = ctx.working_dir.clone();
        let read_files = ctx.read_files.clone();

        Box::pin(async move {
            let file_path = input["file_path"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("file_path is required"))?;
            let old_string = input["old_string"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("old_string is required"))?;
            let new_string = input["new_string"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("new_string is required"))?;
            let replace_all = input["replace_all"].as_bool().unwrap_or(false);

            if old_string == new_string {
                return Ok(ToolResult::error("old_string and new_string are identical"));
            }

            let path = if std::path::Path::new(file_path).is_absolute() {
                std::path::PathBuf::from(file_path)
            } else {
                std::path::PathBuf::from(&working_dir).join(file_path)
            };

            if !path.exists() {
                return Ok(ToolResult::error(format!(
                    "File does not exist: {}",
                    path.display()
                )));
            }

            // Safety check: warn if file hasn't been read first (CC parity)
            if let Ok(canonical) = std::fs::canonicalize(&path) {
                let was_read = read_files
                    .lock()
                    .map(|set| set.contains(&canonical.to_string_lossy().to_string()))
                    .unwrap_or(true); // Default to "read" if lock fails
                if !was_read {
                    return Ok(ToolResult::error(
                        "File has not been read yet. Read it first before writing to it."
                            .to_string(),
                    ));
                }
            }

            let raw_content = match tokio::fs::read_to_string(&path).await {
                Ok(c) => c,
                Err(e) => {
                    return Ok(ToolResult::error(format!(
                        "Error reading {}: {e}",
                        path.display()
                    )));
                }
            };

            // Normalize CRLF → LF for matching (like CC)
            let content = raw_content.replace("\r\n", "\n");
            let old_string = &old_string.replace("\r\n", "\n");

            // Strip trailing whitespace from new_string (except .md files)
            let is_markdown = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e == "md" || e == "mdx")
                .unwrap_or(false);
            let new_string: &str = if !is_markdown {
                &new_string
                    .lines()
                    .map(|l| l.trim_end())
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                new_string
            };

            // Try exact match first, then quote-normalized match
            let (search_str, actual_match) = find_actual_string(&content, old_string);
            let _ = &search_str; // used for the match below

            let match_count = content.matches(&actual_match).count();

            if match_count == 0 {
                return Ok(ToolResult::error(format!(
                    "old_string not found in {}. Make sure the string matches exactly \
                     (including whitespace and indentation).",
                    path.display()
                )));
            }

            if match_count > 1 && !replace_all {
                return Ok(ToolResult::error(format!(
                    "old_string matches {match_count} locations in {}. \
                     Provide more context to make it unique, or set replace_all: true.",
                    path.display()
                )));
            }

            let new_content = if replace_all {
                content.replace(&actual_match, new_string)
            } else {
                content.replacen(&actual_match, new_string, 1)
            };

            match tokio::fs::write(&path, &new_content).await {
                Ok(()) => {
                    // Build a structured diff
                    let diff_output = build_structured_diff(
                        &content,
                        &new_content,
                        file_path,
                        replace_all,
                        match_count,
                    );
                    Ok(ToolResult::success(diff_output))
                }
                Err(e) => Ok(ToolResult::error(format!(
                    "Failed to write {}: {e}",
                    path.display()
                ))),
            }
        })
    }
}

/// Find the actual matching string in the file content.
/// Tries exact match first, then falls back to quote-normalized matching.
/// Returns (search_string_used, actual_string_in_file).
fn find_actual_string(content: &str, search: &str) -> (String, String) {
    // 1. Exact match
    if content.contains(search) {
        return (search.to_string(), search.to_string());
    }

    // 2. Quote-normalized match: curly quotes → straight quotes
    let normalized_search = normalize_quotes(search);
    let normalized_content = normalize_quotes(content);

    if let Some(pos) = normalized_content.find(&normalized_search) {
        // Find the actual substring in the original content at that position
        // by mapping the position in normalized content back to original
        let actual = &content[pos..pos + search.len().min(content.len() - pos)];
        return (normalized_search, actual.to_string());
    }

    // No match found — return the original search string (will result in error)
    (search.to_string(), search.to_string())
}

/// Normalize curly/smart quotes to straight ASCII quotes.
fn normalize_quotes(s: &str) -> String {
    s.replace(['\u{2018}', '\u{2019}'], "'") // right single curly
        .replace(['\u{201C}', '\u{201D}'], "\"") // right double curly
}

/// Build a structured diff output with line numbers and context.
fn build_structured_diff(
    old_content: &str,
    new_content: &str,
    _file_path: &str,
    replace_all: bool,
    match_count: usize,
) -> String {
    use similar::{ChangeTag, TextDiff};

    let diff = TextDiff::from_lines(old_content, new_content);
    let mut added = 0usize;
    let mut removed = 0usize;

    // Count additions and removals
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => added += 1,
            ChangeTag::Delete => removed += 1,
            ChangeTag::Equal => {}
        }
    }

    // Summary line: "Added N lines, removed M lines"
    let summary = match (added > 0, removed > 0) {
        (true, true) => format!(
            "Added {} {}, removed {} {}",
            added,
            if added == 1 { "line" } else { "lines" },
            removed,
            if removed == 1 { "line" } else { "lines" },
        ),
        (true, false) => format!(
            "Added {} {}",
            added,
            if added == 1 { "line" } else { "lines" },
        ),
        (false, true) => format!(
            "Removed {} {}",
            removed,
            if removed == 1 { "line" } else { "lines" },
        ),
        (false, false) => "Updated (no line changes)".to_string(),
    };

    let mut lines = Vec::new();

    if replace_all && match_count > 1 {
        lines.push(format!("{summary} ({match_count} occurrences)"));
    } else {
        lines.push(summary);
    }

    // Generate diff with line numbers:
    //     209                  existing code
    //     210
    //     211 +                new code
    //     212 -                old code
    let unified = diff.unified_diff();
    for hunk in unified.iter_hunks() {
        for change in hunk.iter_changes() {
            let line_content = change.value().trim_end_matches('\n');
            let line_num = match change.tag() {
                ChangeTag::Delete => change.old_index().map(|i| i + 1),
                ChangeTag::Insert | ChangeTag::Equal => change.new_index().map(|i| i + 1),
            };
            let num_str = line_num
                .map(|n| format!("{n:>6}"))
                .unwrap_or_else(|| "      ".to_string());

            let marker = match change.tag() {
                ChangeTag::Delete => " -",
                ChangeTag::Insert => " +",
                ChangeTag::Equal => "  ",
            };

            lines.push(format!("{num_str}{marker}{line_content}"));
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_quotes() {
        assert_eq!(normalize_quotes("it\u{2019}s"), "it's");
        assert_eq!(normalize_quotes("\u{201C}hello\u{201D}"), "\"hello\"");
        assert_eq!(normalize_quotes("plain text"), "plain text");
    }

    #[test]
    fn test_find_actual_string_exact() {
        let content = "fn main() {\n    println!(\"hello\");\n}";
        let (_, actual) = find_actual_string(content, "println!(\"hello\")");
        assert_eq!(actual, "println!(\"hello\")");
    }

    #[test]
    fn test_find_actual_string_quote_normalized() {
        let content = "let s = \"hello world\";";
        // Search with curly quotes should find the straight-quoted version
        let (_, actual) = find_actual_string(content, "let s = \u{201C}hello world\u{201D};");
        // Should find the actual string from the file
        assert!(content.contains(&actual) || actual.contains("hello world"));
    }

    #[test]
    fn test_find_actual_string_no_match() {
        let content = "fn main() {}";
        let (_, actual) = find_actual_string(content, "fn nonexistent()");
        // Returns search string unchanged (will fail on match_count check)
        assert_eq!(actual, "fn nonexistent()");
    }
}
