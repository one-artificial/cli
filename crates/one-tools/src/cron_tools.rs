//! Cron scheduling tools — CronCreate, CronDelete, CronList.
//!
//! These are schema-only tools. Execution is intercepted by the query engine
//! which has access to the CronScheduler in shared state.

use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use serde_json::Value;

use crate::{Tool, ToolContext, ToolResult};

pub struct CronCreateTool;

impl Tool for CronCreateTool {
    fn name(&self) -> &str {
        "cron_create"
    }

    fn description(&self) -> &str {
        "Schedule a prompt to run on a cron schedule. Returns a job ID for management."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "cron": {
                    "type": "string",
                    "description": "5-field cron expression (minute hour day-of-month month day-of-week). Example: \"*/5 * * * *\" = every 5 minutes."
                },
                "prompt": {
                    "type": "string",
                    "description": "The prompt to execute at each fire time."
                },
                "recurring": {
                    "type": "boolean",
                    "description": "true (default) = fire repeatedly. false = fire once then delete."
                }
            },
            "required": ["cron", "prompt"]
        })
    }

    fn execute(
        &self,
        _input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async {
            Ok(ToolResult::error(
                "cron_create not intercepted by query engine",
            ))
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }
    fn should_defer(&self) -> bool {
        true
    }
    fn search_hint(&self) -> Option<&str> {
        Some("schedule cron recurring timer interval")
    }
}

pub struct CronDeleteTool;

impl Tool for CronDeleteTool {
    fn name(&self) -> &str {
        "cron_delete"
    }

    fn description(&self) -> &str {
        "Delete a scheduled cron job by its ID."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "job_id": {
                    "type": "string",
                    "description": "The ID of the job to delete (from cron_create)."
                }
            },
            "required": ["job_id"]
        })
    }

    fn execute(
        &self,
        _input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async {
            Ok(ToolResult::error(
                "cron_delete not intercepted by query engine",
            ))
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }
    fn should_defer(&self) -> bool {
        true
    }
    fn search_hint(&self) -> Option<&str> {
        Some("cancel remove stop cron job schedule")
    }
}

pub struct CronListTool;

impl Tool for CronListTool {
    fn name(&self) -> &str {
        "cron_list"
    }

    fn description(&self) -> &str {
        "List all active cron jobs."
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
        Box::pin(async {
            Ok(ToolResult::error(
                "cron_list not intercepted by query engine",
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
        Some("show active cron jobs scheduled tasks")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_names() {
        assert_eq!(CronCreateTool.name(), "cron_create");
        assert_eq!(CronDeleteTool.name(), "cron_delete");
        assert_eq!(CronListTool.name(), "cron_list");
    }

    #[test]
    fn test_deferred() {
        assert!(CronCreateTool.should_defer());
        assert!(CronDeleteTool.should_defer());
        assert!(CronListTool.should_defer());
    }
}
