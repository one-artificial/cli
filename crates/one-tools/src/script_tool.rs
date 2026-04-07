//! Script-based tool — executes a shell script with JSON input on stdin.
//!
//! Used by the plugin system to turn shell scripts into AI-callable tools.
//! The script receives tool input as JSON on stdin and returns output on stdout.

use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use serde_json::Value;

use crate::{Tool, ToolContext, ToolResult};

/// A tool backed by a shell script.
pub struct ScriptTool {
    tool_name: String,
    tool_description: String,
    entrypoint: String,
    plugin_dir: String,
}

impl ScriptTool {
    pub fn new(
        tool_name: String,
        tool_description: String,
        entrypoint: String,
        plugin_dir: String,
    ) -> Self {
        Self {
            tool_name,
            tool_description,
            entrypoint,
            plugin_dir,
        }
    }
}

impl Tool for ScriptTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        &self.tool_description
    }

    fn input_schema(&self) -> Value {
        // Generic input schema — accepts any JSON object
        serde_json::json!({
            "type": "object",
            "properties": {
                "input": {
                    "type": "string",
                    "description": "Input data for the tool"
                }
            }
        })
    }

    fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        let entrypoint = self.entrypoint.clone();
        let plugin_dir = self.plugin_dir.clone();
        let working_dir = ctx.working_dir.clone();
        let tool_name = self.tool_name.clone();
        let input_json = serde_json::to_string(&input).unwrap_or_default();

        Box::pin(async move {
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                tokio::process::Command::new("bash")
                    .arg("-c")
                    .arg(&entrypoint)
                    .current_dir(&plugin_dir)
                    .env("TOOL_NAME", &tool_name)
                    .env("TOOL_INPUT", &input_json)
                    .env("WORKING_DIR", &working_dir)
                    .output(),
            )
            .await;

            match result {
                Ok(Ok(output)) => {
                    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    if output.status.success() {
                        Ok(ToolResult::success(if stdout.is_empty() {
                            "(no output)".to_string()
                        } else {
                            stdout
                        }))
                    } else {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        Ok(ToolResult::error(format!("{stdout}\n{stderr}")))
                    }
                }
                Ok(Err(e)) => Ok(ToolResult::error(format!("Script failed: {e}"))),
                Err(_) => Ok(ToolResult::error("Script timed out after 30s")),
            }
        })
    }

    fn should_defer(&self) -> bool {
        true // Plugin tools are deferred by default
    }

    fn search_hint(&self) -> Option<&str> {
        None // Plugin tools don't need search hints — they're found by name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_script_tool_properties() {
        let tool = ScriptTool::new(
            "my_tool".to_string(),
            "A custom tool".to_string(),
            "echo hello".to_string(),
            "/tmp".to_string(),
        );
        assert_eq!(tool.name(), "my_tool");
        assert!(tool.should_defer());
    }

    #[tokio::test]
    async fn test_script_execution() {
        let tool = ScriptTool::new(
            "echo_tool".to_string(),
            "Echoes input".to_string(),
            "echo $TOOL_INPUT".to_string(),
            "/tmp".to_string(),
        );
        let ctx = ToolContext::new("/tmp", "test");
        let result = tool
            .execute(serde_json::json!({"input": "hello"}), &ctx)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.output.contains("hello"));
    }
}
