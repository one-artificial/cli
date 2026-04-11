//! RecallDetail tool — retrieve verbatim content from an Evergreen-compressed message.
//!
//! When the Evergreen compressor summarizes a span of conversation turns, the original
//! messages are flagged `is_evergreen_compressed = 1` and hidden from the normal context
//! window.  Their full text is still stored in `messages.content`; this tool fetches it
//! by primary-key ID so the AI can drill into details that the summary left out.
//!
//! Typical usage flow:
//! 1. AI receives an Evergreen summary that mentions "msg_id 42–58 compressed".
//! 2. AI wants to recall exact content from turn 47 (e.g., a specific error message).
//! 3. AI calls `recall_detail { "id": 47 }`.
//! 4. Tool returns the verbatim `role: content` for that message.

use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use serde_json::Value;

use crate::{Tool, ToolContext, ToolResult};

pub struct RecallDetailTool;

impl Tool for RecallDetailTool {
    fn name(&self) -> &str {
        "recall_detail"
    }

    fn description(&self) -> &str {
        "Retrieve the verbatim content of a specific conversation message that has been \
         Evergreen-compressed. Use this when an Evergreen summary references a message ID \
         and you need the exact text (error message, file path, specific value, etc.)."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "integer",
                    "description": "The messages.id of the compressed turn to retrieve."
                }
            },
            "required": ["id"]
        })
    }

    fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        let db_path = ctx.db_path.clone();

        Box::pin(async move {
            let msg_id = input["id"]
                .as_i64()
                .ok_or_else(|| anyhow::anyhow!("Missing or invalid required parameter: id"))?;

            let Some(path) = db_path else {
                return Ok(ToolResult::error(
                    "recall_detail: no session database available for this session",
                ));
            };

            let row = tokio::task::spawn_blocking(move || {
                let db = one_db::SessionDb::open(&path)?;
                db.get_message(msg_id)
            })
            .await??;

            match row {
                None => Ok(ToolResult::error(format!(
                    "No message found with id {msg_id}"
                ))),
                Some(msg) => {
                    let compressed_note = if msg.is_evergreen_compressed {
                        " [evergreen-compressed]"
                    } else {
                        ""
                    };
                    Ok(ToolResult::success(format!(
                        "message_id: {}\nrole: {}{}\ncreated_at: {}\n\n{}",
                        msg.id, msg.role, compressed_note, msg.created_at, msg.content
                    )))
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn should_defer(&self) -> bool {
        // Load on demand — only relevant once compression has occurred
        true
    }

    fn search_hint(&self) -> Option<&str> {
        Some("recall verbatim compressed message detail evergreen")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_no_db() -> ToolContext {
        ToolContext::new("/tmp", "test")
    }

    #[tokio::test]
    async fn test_missing_id() {
        let tool = RecallDetailTool;
        let result = tool.execute(serde_json::json!({}), &ctx_no_db()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_no_db_path() {
        let tool = RecallDetailTool;
        let result = tool
            .execute(serde_json::json!({ "id": 1 }), &ctx_no_db())
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.output.contains("no session database"));
    }

    fn temp_db_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("one_recall_test_{name}.db"))
    }

    #[tokio::test]
    async fn test_missing_message() {
        let db_path = temp_db_path("missing_msg");
        let db = one_db::SessionDb::open(&db_path).unwrap();
        drop(db);

        let mut ctx = ToolContext::new("/tmp", "test");
        ctx.db_path = Some(db_path.clone());

        let tool = RecallDetailTool;
        let result = tool
            .execute(serde_json::json!({ "id": 9999 }), &ctx)
            .await
            .unwrap();
        let _ = std::fs::remove_file(&db_path);
        assert!(result.is_error);
        assert!(result.output.contains("No message found"));
    }

    #[tokio::test]
    async fn test_recall_compressed_message() {
        let db_path = temp_db_path("compressed_msg");
        let db = one_db::SessionDb::open(&db_path).unwrap();
        let msg_id = db
            .save_message("assistant", "The answer is 42.", "2026-04-11T10:00:00Z", None)
            .unwrap();
        db.mark_messages_compressed(msg_id, msg_id).unwrap();
        drop(db);

        let mut ctx = ToolContext::new("/tmp", "test");
        ctx.db_path = Some(db_path.clone());

        let tool = RecallDetailTool;
        let result = tool
            .execute(serde_json::json!({ "id": msg_id }), &ctx)
            .await
            .unwrap();
        let _ = std::fs::remove_file(&db_path);
        assert!(!result.is_error);
        assert!(result.output.contains("The answer is 42."));
        assert!(result.output.contains("evergreen-compressed"));
        assert!(result.output.contains("assistant"));
    }

    #[test]
    fn test_tool_properties() {
        let tool = RecallDetailTool;
        assert_eq!(tool.name(), "recall_detail");
        assert!(tool.is_read_only());
        assert!(tool.should_defer());
    }
}
