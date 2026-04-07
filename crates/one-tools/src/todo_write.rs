//! TodoWrite tool — manages a structured task list.
//!
//! CC uses TodoWrite to create and update task lists during coding sessions.
//! This replaces the entire todo list with the provided array of items.
//! Execution is intercepted by the QueryEngine which stores the todos in state.

use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use serde_json::Value;

use crate::{Tool, ToolContext, ToolResult};

/// TodoWrite tool — write/replace the task list.
/// Execution intercepted by QueryEngine.
pub struct TodoWriteTool;

impl Tool for TodoWriteTool {
    fn name(&self) -> &str {
        "TodoWrite"
    }

    fn description(&self) -> &str {
        "Use this tool to create and manage a structured task list for your current \
         coding session. This helps you track progress, organize complex tasks, and \
         demonstrate thoroughness to the user."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "todos": {
                    "description": "The updated todo list",
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": {
                                "type": "string",
                                "minLength": 1
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed"]
                            },
                            "activeForm": {
                                "type": "string",
                                "minLength": 1
                            }
                        },
                        "required": ["content", "status", "activeForm"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["todos"],
            "additionalProperties": false
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        let todos = input["todos"].clone();
        Box::pin(async move {
            let count = todos.as_array().map(|a| a.len()).unwrap_or(0);
            Ok(ToolResult::success(format!("Updated {count} todos.")))
        })
    }
}
