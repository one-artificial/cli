//! AskUserQuestion tool — lets the AI ask the user structured questions.
//!
//! The model calls this tool to present choices or ask for clarification.
//! The tool blocks until the user responds.
//!
//! In TUI mode: emits a UserQuestion event and waits for the answer via oneshot.
//! In one-shot mode: prints the question to stderr and reads from stdin.

use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use serde_json::Value;

use crate::{Tool, ToolContext, ToolResult};

pub struct AskUserQuestionTool;

impl Tool for AskUserQuestionTool {
    fn name(&self) -> &str {
        "ask_user"
    }

    fn description(&self) -> &str {
        "Ask the user a question to gather information, clarify ambiguity, or get a decision. \
         Provide 2-4 options for multiple choice, or omit options for free-text input. \
         The user can always provide a custom answer."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to ask the user. Should be clear and specific."
                },
                "options": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "label": {
                                "type": "string",
                                "description": "Short label for this option (1-5 words)"
                            },
                            "description": {
                                "type": "string",
                                "description": "Explanation of what this option means"
                            }
                        },
                        "required": ["label"]
                    },
                    "description": "2-4 options for multiple choice. Omit for free-text input."
                }
            },
            "required": ["question"]
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let question = input["question"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing required parameter: question"))?;

            // Format options for display
            let options: Vec<String> = input["options"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|opt| {
                            let label = opt["label"].as_str()?;
                            let desc = opt["description"].as_str().unwrap_or("");
                            if desc.is_empty() {
                                Some(label.to_string())
                            } else {
                                Some(format!("{label}: {desc}"))
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();

            // Format the question for the tool result.
            // The query engine will intercept this tool and handle user interaction.
            // For now, return the formatted question — the query engine pauses.
            let mut formatted = format!("Question: {question}");
            if !options.is_empty() {
                formatted.push_str("\n\nOptions:");
                for (i, opt) in options.iter().enumerate() {
                    formatted.push_str(&format!("\n  {}. {opt}", i + 1));
                }
                formatted.push_str("\n  (or type a custom answer)");
            }

            // Mark as needing user input — the query engine checks tool name
            Ok(ToolResult {
                output: formatted,
                is_error: false,
                new_cwd: None,
            })
        })
    }

    fn is_read_only(&self) -> bool {
        true // asking a question doesn't modify anything
    }

    fn search_hint(&self) -> Option<&str> {
        Some("ask question user input clarify choice")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_context() -> ToolContext {
        ToolContext::new("/tmp", "test")
    }

    #[tokio::test]
    async fn test_simple_question() {
        let tool = AskUserQuestionTool;
        let ctx = test_context();

        let result = tool
            .execute(
                serde_json::json!({
                    "question": "Which database should we use?"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.output.contains("Which database should we use?"));
    }

    #[tokio::test]
    async fn test_multiple_choice() {
        let tool = AskUserQuestionTool;
        let ctx = test_context();

        let result = tool
            .execute(
                serde_json::json!({
                    "question": "Which approach?",
                    "options": [
                        {"label": "Option A", "description": "Fast but complex"},
                        {"label": "Option B", "description": "Simple but slower"}
                    ]
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.output.contains("Option A"));
        assert!(result.output.contains("Option B"));
        assert!(result.output.contains("Fast but complex"));
    }

    #[tokio::test]
    async fn test_missing_question() {
        let tool = AskUserQuestionTool;
        let ctx = test_context();

        let result = tool.execute(serde_json::json!({}), &ctx).await;

        assert!(result.is_err());
    }

    #[test]
    fn test_tool_properties() {
        let tool = AskUserQuestionTool;
        assert_eq!(tool.name(), "ask_user");
        assert!(tool.is_read_only());
        assert!(!tool.should_defer());
    }
}
