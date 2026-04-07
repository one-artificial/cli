use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use serde_json::Value;

use crate::{Tool, ToolContext, ToolResult};

/// Searches file contents using ripgrep (rg) if available, falling back to grep.
/// Respects .gitignore by default.
pub struct GrepTool;

const DEFAULT_HEAD_LIMIT: usize = 250;

impl Tool for GrepTool {
    fn name(&self) -> &str {
        "Grep"
    }

    fn description(&self) -> &str {
        "Search for a pattern in file contents using ripgrep. Supports regex, \
         multiple output modes, context lines, and file type filtering."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regular expression pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "File or directory to search in. Defaults to working directory."
                },
                "glob": {
                    "type": "string",
                    "description": "Glob pattern to filter files (e.g. \"*.rs\", \"*.{ts,tsx}\")"
                },
                "type": {
                    "type": "string",
                    "description": "File type to search (e.g. \"js\", \"py\", \"rust\", \"go\"). More efficient than glob for standard file types."
                },
                "output_mode": {
                    "type": "string",
                    "enum": ["content", "files_with_matches", "count"],
                    "description": "Output mode: \"content\" shows matching lines, \"files_with_matches\" shows only file paths (default), \"count\" shows match counts per file."
                },
                "-i": {
                    "type": "boolean",
                    "description": "Case insensitive search"
                },
                "-n": {
                    "type": "boolean",
                    "description": "Show line numbers in output. Default true for content mode."
                },
                "-A": {
                    "type": "integer",
                    "description": "Number of lines to show after each match. Requires output_mode: \"content\"."
                },
                "-B": {
                    "type": "integer",
                    "description": "Number of lines to show before each match. Requires output_mode: \"content\"."
                },
                "-C": {
                    "type": "integer",
                    "description": "Number of lines to show before and after each match (context). Requires output_mode: \"content\"."
                },
                "context": {
                    "type": "integer",
                    "description": "Alias for -C (context lines)."
                },
                "multiline": {
                    "type": "boolean",
                    "description": "Enable multiline matching where . matches newlines. Default: false."
                },
                "head_limit": {
                    "type": "integer",
                    "description": "Limit output to first N lines/entries. Default: 250. Pass 0 for unlimited."
                },
                "offset": {
                    "type": "integer",
                    "description": "Skip first N lines/entries before applying head_limit. Default: 0."
                }
            },
            "required": ["pattern"]
        })
    }

    fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        let input = input.clone();
        let working_dir = ctx.working_dir.clone();

        Box::pin(async move {
            let pattern = input["pattern"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("pattern is required"))?;

            let search_path = input["path"]
                .as_str()
                .map(String::from)
                .unwrap_or(working_dir);

            let output_mode = input["output_mode"]
                .as_str()
                .unwrap_or("files_with_matches");

            let case_insensitive = input["-i"].as_bool().unwrap_or(false);
            let show_line_numbers = input["-n"].as_bool().unwrap_or(output_mode == "content");
            let multiline = input["multiline"].as_bool().unwrap_or(false);
            let head_limit = input["head_limit"]
                .as_u64()
                .map(|v| v as usize)
                .unwrap_or(DEFAULT_HEAD_LIMIT);
            let offset = input["offset"].as_u64().unwrap_or(0) as usize;

            // Context lines: -C > context > -A/-B
            let context_c = input["-C"].as_u64().or_else(|| input["context"].as_u64());
            let context_a = input["-A"].as_u64();
            let context_b = input["-B"].as_u64();

            // Build rg command
            let mut cmd = tokio::process::Command::new("rg");
            cmd.arg("--no-heading");

            match output_mode {
                "content" => {
                    if show_line_numbers {
                        cmd.arg("-n");
                    }
                    // Context lines
                    if let Some(c) = context_c {
                        cmd.arg("-C").arg(c.to_string());
                    } else {
                        if let Some(a) = context_a {
                            cmd.arg("-A").arg(a.to_string());
                        }
                        if let Some(b) = context_b {
                            cmd.arg("-B").arg(b.to_string());
                        }
                    }
                }
                "count" => {
                    cmd.arg("-c");
                }
                _ => {
                    // files_with_matches (default)
                    cmd.arg("-l");
                }
            }

            if case_insensitive {
                cmd.arg("-i");
            }

            if multiline {
                cmd.arg("-U"); // multiline mode
                cmd.arg("--multiline-dotall");
            }

            // File type filtering
            if let Some(file_type) = input["type"].as_str() {
                cmd.arg("--type").arg(file_type);
            }

            if let Some(glob) = input["glob"].as_str() {
                cmd.arg("--glob").arg(glob);
            }

            cmd.arg(pattern);
            cmd.arg(&search_path);

            let output = cmd.output().await;

            match output {
                Ok(out) => {
                    let stdout = String::from_utf8_lossy(&out.stdout);

                    if stdout.is_empty() {
                        return Ok(ToolResult::success("No matches found"));
                    }

                    let lines: Vec<&str> = stdout.lines().collect();
                    let total = lines.len();

                    // Apply offset and head_limit
                    let effective_limit = if head_limit == 0 { total } else { head_limit };
                    let display: String = lines
                        .into_iter()
                        .skip(offset)
                        .take(effective_limit)
                        .collect::<Vec<_>>()
                        .join("\n");

                    let mut result = display;
                    let shown = total.saturating_sub(offset).min(effective_limit);
                    let remaining = total.saturating_sub(offset + shown);

                    if remaining > 0 {
                        result.push_str(&format!(
                            "\n\n[Showing results with pagination = limit: {effective_limit}]"
                        ));
                    }

                    Ok(ToolResult::success(result))
                }
                Err(_) => {
                    // rg not found, try grep fallback
                    let mut cmd = tokio::process::Command::new("grep");
                    cmd.arg("-r");
                    match output_mode {
                        "content" => {
                            cmd.arg("-n");
                        }
                        "count" => {
                            cmd.arg("-c");
                        }
                        _ => {
                            cmd.arg("-l");
                        }
                    }
                    if case_insensitive {
                        cmd.arg("-i");
                    }
                    cmd.arg(pattern);
                    cmd.arg(&search_path);

                    match cmd.output().await {
                        Ok(out) => {
                            let stdout = String::from_utf8_lossy(&out.stdout);
                            if stdout.is_empty() {
                                Ok(ToolResult::success("No matches found"))
                            } else {
                                Ok(ToolResult::success(stdout.to_string()))
                            }
                        }
                        Err(e) => Ok(ToolResult::error(format!(
                            "Neither rg nor grep available: {e}"
                        ))),
                    }
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn search_hint(&self) -> Option<&str> {
        Some("search file contents regex pattern ripgrep")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_context() -> ToolContext {
        ToolContext::new("/tmp", "test")
    }

    #[tokio::test]
    async fn test_basic_search() {
        let tool = GrepTool;
        let ctx = test_context();

        // Search for something that definitely exists
        let result = tool
            .execute(
                serde_json::json!({
                    "pattern": "PATH",
                    "path": "/etc/profile",
                    "output_mode": "content"
                }),
                &ctx,
            )
            .await
            .unwrap();

        // /etc/profile should contain PATH
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_no_matches() {
        let tool = GrepTool;
        let ctx = test_context();

        let result = tool
            .execute(
                serde_json::json!({
                    "pattern": "xyzzy_impossible_pattern_99999",
                    "path": "/etc/profile"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.output.contains("No matches"));
    }

    #[test]
    fn test_tool_properties() {
        let tool = GrepTool;
        assert_eq!(tool.name(), "Grep");
        assert!(tool.is_read_only());
        assert!(!tool.should_defer());
    }

    #[test]
    fn test_schema_has_output_mode() {
        let tool = GrepTool;
        let schema = tool.input_schema();
        let props = &schema["properties"];

        assert!(props["output_mode"].is_object());
        assert!(props["-i"].is_object());
        assert!(props["-A"].is_object());
        assert!(props["-B"].is_object());
        assert!(props["-C"].is_object());
        assert!(props["multiline"].is_object());
        assert!(props["head_limit"].is_object());
        assert!(props["type"].is_object());
    }
}
