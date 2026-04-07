//! Skill tool — lets the AI invoke user-installed slash commands.
//!
//! This tool's schema is registered normally, but execution is intercepted
//! by the QueryEngine which loads and prepares the skill prompt. The tool
//! itself returns a fallback message — real execution happens in the engine.

use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use serde_json::Value;

use crate::{Tool, ToolContext, ToolResult};

/// Skill tool — invoke a user-installed slash command by name.
/// Execution is intercepted by the QueryEngine.
pub struct SkillTool;

impl Tool for SkillTool {
    fn name(&self) -> &str {
        "Skill"
    }

    fn description(&self) -> &str {
        "Execute a skill within the main conversation. \
         When users ask you to perform tasks, check if any available skills match. \
         Skills provide specialized capabilities and domain knowledge. \
         When users reference a slash command (e.g., /commit, /review-pr), \
         use this tool to invoke it."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "skill": {
                    "type": "string",
                    "description": "The skill name. E.g., \"commit\", \"review-pr\", or \"pdf\""
                },
                "args": {
                    "type": "string",
                    "description": "Optional arguments for the skill"
                }
            },
            "required": ["skill"],
            "additionalProperties": false
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        let skill_name = input["skill"].as_str().unwrap_or("").to_string();

        Box::pin(async move {
            // Fallback — real execution is intercepted by QueryEngine
            Ok(ToolResult::error(format!(
                "Skill '{skill_name}' not found or not intercepted by the engine."
            )))
        })
    }

    fn is_read_only(&self) -> bool {
        false // Skills can modify files
    }
}
