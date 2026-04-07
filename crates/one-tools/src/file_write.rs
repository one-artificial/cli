use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use serde_json::Value;

use crate::{Tool, ToolContext, ToolResult};

/// Writes content to a file. Creates parent directories if needed.
pub struct FileWriteTool;

impl Tool for FileWriteTool {
    fn name(&self) -> &str {
        "Write"
    }

    fn description(&self) -> &str {
        "Write content to a file. Creates the file if it doesn't exist. \
         Creates parent directories as needed. Overwrites existing content."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                }
            },
            "required": ["file_path", "content"]
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

            let content = input["content"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("content is required"))?;

            let path = if std::path::Path::new(file_path).is_absolute() {
                std::path::PathBuf::from(file_path)
            } else {
                std::path::PathBuf::from(&working_dir).join(file_path)
            };

            // Safety check: if the file already exists, it must have been read first
            if path.exists()
                && let Ok(canonical) = std::fs::canonicalize(&path)
            {
                let was_read = read_files
                    .lock()
                    .map(|set| set.contains(&canonical.to_string_lossy().to_string()))
                    .unwrap_or(true);
                if !was_read {
                    return Ok(ToolResult::error(
                        "File has not been read yet. Read it first before writing to it.",
                    ));
                }
            }

            // Create parent directories
            if let Some(parent) = path.parent()
                && let Err(e) = tokio::fs::create_dir_all(parent).await
            {
                return Ok(ToolResult::error(format!(
                    "Failed to create directories: {e}"
                )));
            }

            match tokio::fs::write(&path, content).await {
                Ok(()) => {
                    let line_count = content.lines().count();
                    Ok(ToolResult::success(format!(
                        "{line_count} lines\nWrote {} bytes to {}",
                        content.len(),
                        path.display()
                    )))
                }
                Err(e) => Ok(ToolResult::error(format!(
                    "Failed to write {}: {e}",
                    path.display()
                ))),
            }
        })
    }
}
