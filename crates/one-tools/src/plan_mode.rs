//! Plan mode tools — let the AI enter/exit plan mode programmatically.
//!
//! EnterPlanMode: AI starts describing what it would do without executing.
//! ExitPlanMode: AI presents its plan and exits plan mode.
//! Mirrors CC's EnterPlanModeTool and ExitPlanModeTool.

use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use serde_json::Value;

use crate::{Tool, ToolContext, ToolResult};

/// Tool that lets the AI enter plan mode.
/// In plan mode, tool calls are described but not executed.
pub struct EnterPlanModeTool;

impl Tool for EnterPlanModeTool {
    fn name(&self) -> &str {
        "enter_plan_mode"
    }

    fn description(&self) -> &str {
        "Enter plan mode. In plan mode, you describe what tools you would call and why, \
         without actually executing them. Use this for complex tasks that benefit from \
         planning before execution."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    fn execute(
        &self,
        _input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            // The query engine checks the tool name and toggles plan mode
            Ok(ToolResult::success(
                "Plan mode activated. Describe what you would do without executing tools. \
                 Call exit_plan_mode when the plan is ready for review.",
            ))
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> Option<&str> {
        Some("plan mode describe strategy before executing")
    }
}

/// Tool that lets the AI exit plan mode and present its plan.
pub struct ExitPlanModeTool;

impl Tool for ExitPlanModeTool {
    fn name(&self) -> &str {
        "exit_plan_mode"
    }

    fn description(&self) -> &str {
        "Exit plan mode and present the plan for user approval. \
         Only callable when plan mode is active."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "plan_summary": {
                    "type": "string",
                    "description": "A brief summary of the plan to present to the user for approval."
                }
            },
            "required": []
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let summary = input["plan_summary"].as_str().unwrap_or("Plan complete.");

            Ok(ToolResult::success(format!(
                "Plan mode deactivated. Plan summary:\n\n{summary}\n\n\
                 Ready to execute. Proceed with the plan."
            )))
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> Option<&str> {
        Some("exit plan mode approve execute proceed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ToolContext {
        ToolContext::new("/tmp", "test")
    }

    #[tokio::test]
    async fn test_enter_plan_mode() {
        let tool = EnterPlanModeTool;
        let result = tool.execute(serde_json::json!({}), &ctx()).await.unwrap();
        assert!(!result.is_error);
        assert!(result.output.contains("Plan mode activated"));
    }

    #[tokio::test]
    async fn test_exit_plan_mode() {
        let tool = ExitPlanModeTool;
        let result = tool
            .execute(
                serde_json::json!({"plan_summary": "Step 1: Read files. Step 2: Edit code."}),
                &ctx(),
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.output.contains("Plan mode deactivated"));
        assert!(result.output.contains("Step 1"));
    }

    #[test]
    fn test_both_deferred() {
        assert!(EnterPlanModeTool.should_defer());
        assert!(ExitPlanModeTool.should_defer());
        assert!(EnterPlanModeTool.is_read_only());
        assert!(ExitPlanModeTool.is_read_only());
    }
}
