use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use serde_json::Value;

use crate::{Tool, ToolContext, ToolResult};

/// Executes a bash command and returns stdout/stderr.
/// Commands run in the session's working directory.
/// Supports timeout, description (for activity display), and background execution.
pub struct BashTool;

const MAX_OUTPUT_BYTES: usize = 50_000; // Match CC's 50K char truncation limit
const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000; // 10 minutes

impl Tool for BashTool {
    fn name(&self) -> &str {
        "Bash"
    }

    fn description(&self) -> &str {
        "Execute a bash command in the project directory. Returns stdout and stderr. \
         Commands time out after 120 seconds by default (max 600s)."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash command to execute"
                },
                "description": {
                    "type": "string",
                    "description": "Brief description of what this command does (for activity display)"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (max 600000). Default: 120000"
                },
                "run_in_background": {
                    "type": "boolean",
                    "description": "Run command in background and return immediately. Output available later."
                },
                "dangerouslyDisableSandbox": {
                    "type": "boolean",
                    "description": "Set to true to run commands without sandboxing."
                }
            },
            "required": ["command"]
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
            let command = input["command"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("command is required"))?;

            let timeout_ms = input["timeout"]
                .as_u64()
                .unwrap_or(DEFAULT_TIMEOUT_MS)
                .min(MAX_TIMEOUT_MS);

            let run_in_background = input["run_in_background"].as_bool().unwrap_or(false);
            let description = input["description"].as_str().unwrap_or("");

            if run_in_background {
                // Spawn the command and return immediately
                let cmd_str = command.to_string();
                let wd = working_dir.clone();
                tokio::spawn(async move {
                    let _ = tokio::process::Command::new("bash")
                        .arg("-c")
                        .arg(&cmd_str)
                        .current_dir(&wd)
                        .output()
                        .await;
                });

                let msg = if description.is_empty() {
                    format!("Command launched in background: {command}")
                } else {
                    format!("Background: {description}")
                };
                return Ok(ToolResult::success(msg));
            }

            // Wrap with a sentinel so we can detect directory changes.
            // The exit code of the original command is preserved.
            // $PWD is updated by bash when `cd` runs, so it reflects the
            // post-command directory even if the command never printed anything.
            const CWD_MARKER: &str = "__ONE_EXIT_CWD__:";
            let wrapped = format!(
                "{{ {command}; }}; __ec=$?; printf '\\n{CWD_MARKER}%s\\n' \"$PWD\"; exit $__ec"
            );

            let result = tokio::time::timeout(
                std::time::Duration::from_millis(timeout_ms),
                tokio::process::Command::new("bash")
                    .arg("-c")
                    .arg(&wrapped)
                    .current_dir(&working_dir)
                    .output(),
            )
            .await;

            match result {
                Ok(Ok(output)) => {
                    let raw_stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                    // Parse and strip the cwd sentinel from stdout.
                    // The sentinel is always on its own line: "\n__ONE_EXIT_CWD__:<path>\n"
                    let (mut stdout, new_cwd) =
                        if let Some(marker_pos) = raw_stdout.find(CWD_MARKER) {
                            let path_start = marker_pos + CWD_MARKER.len();
                            let path_end = raw_stdout[path_start..]
                                .find('\n')
                                .map(|n| path_start + n)
                                .unwrap_or(raw_stdout.len());
                            let captured = raw_stdout[path_start..path_end].to_string();
                            // Strip the sentinel line (and the preceding \n)
                            let strip_from = marker_pos.saturating_sub(1);
                            let clean =
                                format!("{}{}", &raw_stdout[..strip_from], &raw_stdout[path_end..]);
                            let cwd = if captured.is_empty() || captured == working_dir {
                                None
                            } else {
                                Some(captured)
                            };
                            (clean, cwd)
                        } else {
                            (raw_stdout, None)
                        };

                    let mut combined = String::new();

                    if !stdout.is_empty() {
                        if stdout.len() > MAX_OUTPUT_BYTES {
                            stdout.truncate(MAX_OUTPUT_BYTES);
                            stdout.push_str("\n... (output truncated)");
                        }
                        combined.push_str(&stdout);
                    }

                    if !stderr.is_empty() {
                        if !combined.is_empty() {
                            combined.push('\n');
                        }
                        combined.push_str("stderr:\n");
                        combined.push_str(&stderr);
                    }

                    if !output.status.success() {
                        combined.push_str(&format!(
                            "\nExit code: {}",
                            output.status.code().unwrap_or(-1)
                        ));
                        Ok(ToolResult {
                            output: combined,
                            is_error: true,
                            new_cwd,
                        })
                    } else {
                        if combined.is_empty() {
                            combined = "(no output)".to_string();
                        }
                        Ok(ToolResult {
                            output: combined,
                            is_error: false,
                            new_cwd,
                        })
                    }
                }
                Ok(Err(e)) => Ok(ToolResult::error(format!("Failed to execute: {e}"))),
                Err(_) => Ok(ToolResult::error(format!(
                    "Command timed out after {timeout_ms}ms"
                ))),
            }
        })
    }

    fn is_destructive(&self) -> bool {
        true // bash can run anything — permission system handles specifics
    }

    fn search_hint(&self) -> Option<&str> {
        Some("execute shell command terminal process")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ToolContext {
        ToolContext::new("/tmp", "test")
    }

    #[tokio::test]
    async fn test_simple_command() {
        let tool = BashTool;
        let result = tool
            .execute(serde_json::json!({"command": "echo hello"}), &ctx())
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.output.contains("hello"));
    }

    #[tokio::test]
    async fn test_failing_command() {
        let tool = BashTool;
        let result = tool
            .execute(serde_json::json!({"command": "false"}), &ctx())
            .await
            .unwrap();

        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_background_command() {
        let tool = BashTool;
        let result = tool
            .execute(
                serde_json::json!({
                    "command": "sleep 0.1",
                    "run_in_background": true,
                    "description": "Short sleep"
                }),
                &ctx(),
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.output.contains("Background: Short sleep"));
    }

    #[tokio::test]
    async fn test_with_description() {
        let tool = BashTool;
        let result = tool
            .execute(
                serde_json::json!({
                    "command": "echo test",
                    "description": "Print test"
                }),
                &ctx(),
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.output.contains("test"));
    }

    #[test]
    fn test_tool_properties() {
        let tool = BashTool;
        assert_eq!(tool.name(), "Bash");
        assert!(tool.is_destructive());
        assert!(!tool.is_read_only());
        assert!(!tool.should_defer());
    }
}
