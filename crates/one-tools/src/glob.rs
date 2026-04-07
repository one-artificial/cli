use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use serde_json::Value;

use crate::{Tool, ToolContext, ToolResult};

/// Finds files matching glob patterns. Uses ripgrep for fast, gitignore-aware
/// searching with results sorted by modification time (oldest first).
pub struct GlobTool;

const MAX_RESULTS: usize = 100;

impl Tool for GlobTool {
    fn name(&self) -> &str {
        "Glob"
    }

    fn description(&self) -> &str {
        "Find files matching a glob pattern. Returns matching file paths sorted by \
         modification time. Use this when you need to find files by name patterns."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern (e.g. \"**/*.rs\", \"src/**/*.ts\")"
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in. Defaults to working directory."
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

            // Try ripgrep first: --files --glob --sort=modified (like CC)
            let output = tokio::process::Command::new("rg")
                .arg("--files")
                .arg("--glob")
                .arg(pattern)
                .arg("--sort=modified")
                .arg("--hidden") // include hidden files
                .arg(&search_path)
                .output()
                .await;

            let result = match output {
                Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).to_string(),
                _ => {
                    // Fallback: try fd
                    let fd_out = tokio::process::Command::new("fd")
                        .arg("--glob")
                        .arg(pattern)
                        .arg(&search_path)
                        .arg("--type")
                        .arg("f")
                        .output()
                        .await;

                    match fd_out {
                        Ok(out) if out.status.success() => {
                            String::from_utf8_lossy(&out.stdout).to_string()
                        }
                        _ => {
                            // Final fallback: find
                            let name_pattern = pattern.strip_prefix("**/").unwrap_or(pattern);
                            let find_out = tokio::process::Command::new("find")
                                .arg(&search_path)
                                .arg("-name")
                                .arg(name_pattern)
                                .arg("-type")
                                .arg("f")
                                .output()
                                .await;

                            match find_out {
                                Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
                                Err(e) => {
                                    return Ok(ToolResult::error(format!(
                                        "No search tool available (tried rg, fd, find): {e}"
                                    )));
                                }
                            }
                        }
                    }
                }
            };

            if result.trim().is_empty() {
                return Ok(ToolResult::success("No files matched."));
            }

            let lines: Vec<&str> = result.lines().collect();
            let total = lines.len();
            let truncated = total > MAX_RESULTS;

            let display: String = lines
                .into_iter()
                .take(MAX_RESULTS)
                .collect::<Vec<_>>()
                .join("\n");

            let mut output = display;
            if truncated {
                output.push_str(&format!(
                    "\n\n({total} total files, showing first {MAX_RESULTS})"
                ));
            }

            Ok(ToolResult::success(output))
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn search_hint(&self) -> Option<&str> {
        Some("find files pattern name glob directory search")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ToolContext {
        ToolContext::new("/tmp", "test")
    }

    #[tokio::test]
    async fn test_no_matches() {
        let tool = GlobTool;
        let result = tool
            .execute(
                serde_json::json!({"pattern": "*.xyzzy_nonexistent_ext"}),
                &ctx(),
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.output.contains("No files matched"));
    }

    #[test]
    fn test_tool_properties() {
        let tool = GlobTool;
        assert_eq!(tool.name(), "Glob");
        assert!(tool.is_read_only());
        assert!(!tool.should_defer());
    }
}
