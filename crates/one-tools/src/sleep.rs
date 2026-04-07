//! SleepTool — pause execution for a specified duration.
//!
//! Used by agents to wait for async operations, rate limiting, or
//! coordinating between parallel tasks. Max 300 seconds.

use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use serde_json::Value;

use crate::{Tool, ToolContext, ToolResult};

pub struct SleepTool;

const MAX_SECONDS: u64 = 300;

impl Tool for SleepTool {
    fn name(&self) -> &str {
        "sleep"
    }

    fn description(&self) -> &str {
        "Pause execution for a specified number of seconds (max 300). \
         Use sparingly — prefer checking status directly over sleeping."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "seconds": {
                    "type": "integer",
                    "description": "Number of seconds to sleep (1-300)"
                }
            },
            "required": ["seconds"]
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let seconds = input["seconds"]
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("seconds is required"))?;

            let clamped = seconds.clamp(1, MAX_SECONDS);

            tokio::time::sleep(std::time::Duration::from_secs(clamped)).await;

            Ok(ToolResult::success(format!("Slept for {clamped} seconds.")))
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn should_defer(&self) -> bool {
        true // rarely needed — load via ToolSearch
    }

    fn search_hint(&self) -> Option<&str> {
        Some("wait pause delay seconds timer")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_properties() {
        let tool = SleepTool;
        assert_eq!(tool.name(), "sleep");
        assert!(tool.is_read_only());
        assert!(tool.should_defer());
    }

    #[tokio::test]
    async fn test_sleep_short() {
        let tool = SleepTool;
        let ctx = ToolContext::new("/tmp", "test");

        let start = std::time::Instant::now();
        let result = tool
            .execute(serde_json::json!({"seconds": 1}), &ctx)
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.output.contains("1 seconds"));
        assert!(start.elapsed().as_secs() >= 1);
    }
}
