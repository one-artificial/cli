//! NotebookEdit tool — edit cells in Jupyter notebooks (.ipynb).
//!
//! Modifies a specific cell by index, replacing its source content.
//! Preserves the notebook structure, metadata, and other cells.

use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use serde_json::Value;

use crate::{Tool, ToolContext, ToolResult};

pub struct NotebookEditTool;

impl Tool for NotebookEditTool {
    fn name(&self) -> &str {
        "notebook_edit"
    }

    fn description(&self) -> &str {
        "Edit a cell in a Jupyter notebook (.ipynb). Replace the source content \
         of a specific cell by index (1-based). Use file_read first to see the \
         notebook structure."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the .ipynb file"
                },
                "cell_index": {
                    "type": "integer",
                    "description": "1-based index of the cell to edit"
                },
                "new_source": {
                    "type": "string",
                    "description": "New source content for the cell"
                },
                "cell_type": {
                    "type": "string",
                    "enum": ["code", "markdown", "raw"],
                    "description": "Optionally change the cell type"
                }
            },
            "required": ["file_path", "cell_index", "new_source"]
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
            let file_path = input["file_path"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("file_path is required"))?;
            let cell_index = input["cell_index"]
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("cell_index is required"))?
                as usize;
            let new_source = input["new_source"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("new_source is required"))?;

            if cell_index == 0 {
                return Ok(ToolResult::error("cell_index is 1-based (minimum 1)"));
            }

            let path = if std::path::Path::new(file_path).is_absolute() {
                std::path::PathBuf::from(file_path)
            } else {
                std::path::PathBuf::from(&working_dir).join(file_path)
            };

            if !path.exists() {
                return Ok(ToolResult::error(format!(
                    "File not found: {}",
                    path.display()
                )));
            }

            let content = tokio::fs::read_to_string(&path).await?;
            let mut notebook: Value = serde_json::from_str(&content)
                .map_err(|e| anyhow::anyhow!("Failed to parse notebook: {e}"))?;

            let cells = notebook["cells"]
                .as_array_mut()
                .ok_or_else(|| anyhow::anyhow!("No cells array in notebook"))?;

            let idx = cell_index - 1; // Convert to 0-based
            if idx >= cells.len() {
                return Ok(ToolResult::error(format!(
                    "Cell index {} out of range (notebook has {} cells)",
                    cell_index,
                    cells.len()
                )));
            }

            // Update source — notebooks store source as array of lines
            let source_lines: Vec<Value> = new_source
                .lines()
                .enumerate()
                .map(|(i, line)| {
                    let total_lines = new_source.lines().count();
                    if i < total_lines - 1 {
                        Value::String(format!("{line}\n"))
                    } else {
                        Value::String(line.to_string())
                    }
                })
                .collect();

            cells[idx]["source"] = Value::Array(source_lines);

            // Optionally update cell type
            if let Some(new_type) = input["cell_type"].as_str() {
                cells[idx]["cell_type"] = Value::String(new_type.to_string());
            }

            // Clear outputs for code cells when source changes
            if cells[idx]["cell_type"].as_str() == Some("code") {
                cells[idx]["outputs"] = Value::Array(Vec::new());
                cells[idx]["execution_count"] = Value::Null;
            }

            // Get cell type before releasing the mutable borrow
            let cell_type = cells[idx]["cell_type"]
                .as_str()
                .unwrap_or("unknown")
                .to_string();

            // Write back
            let output = serde_json::to_string_pretty(&notebook)?;
            tokio::fs::write(&path, &output).await?;

            Ok(ToolResult::success(format!(
                "Updated cell {} ({cell_type}) in {}",
                cell_index,
                path.display()
            )))
        })
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> Option<&str> {
        Some("edit jupyter notebook cell ipynb modify")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_properties() {
        let tool = NotebookEditTool;
        assert_eq!(tool.name(), "notebook_edit");
        assert!(tool.should_defer());
        assert!(!tool.is_read_only());
    }
}
