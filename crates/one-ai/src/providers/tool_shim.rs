//! Tool-use shim for providers without native tool support.
//!
//! Wraps any AiProvider and adds prompt-based tool calling:
//! 1. Injects tool schemas into the system prompt
//! 2. Instructs the model to use <tool_call> XML blocks
//! 3. Parses tool calls from the response text
//!
//! This makes Ollama and other providers functional for tool-based workflows.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;
use one_core::provider::{AiProvider, AiResponse, Message, ModelConfig, Role, ToolCall};

/// Wraps a provider that lacks native tool use, injecting tool schemas
/// into the system prompt and parsing tool calls from text output.
pub struct ToolShimProvider {
    inner: Arc<dyn AiProvider>,
    tool_schemas: Vec<serde_json::Value>,
}

impl ToolShimProvider {
    pub fn new(inner: Arc<dyn AiProvider>, tool_schemas: Vec<serde_json::Value>) -> Self {
        Self {
            inner,
            tool_schemas,
        }
    }

    /// Build the tool description text to inject into the system prompt.
    fn tool_prompt(&self) -> String {
        if self.tool_schemas.is_empty() {
            return String::new();
        }

        let mut prompt = String::from(
            "\n\n# Available Tools\n\n\
             You have access to the following tools. To use a tool, output a tool_call block \
             in this exact format:\n\n\
             <tool_call>\n\
             {\"name\": \"tool_name\", \"input\": {\"param\": \"value\"}}\n\
             </tool_call>\n\n\
             You can make multiple tool calls in a single response. Each must be in its own \
             <tool_call> block. Wait for tool results before proceeding.\n\n\
             ## Tools\n\n",
        );

        for schema in &self.tool_schemas {
            let name = schema["name"].as_str().unwrap_or("unknown");
            let desc = schema["description"].as_str().unwrap_or("");
            prompt.push_str(&format!("### {name}\n{desc}\n\n"));

            if let Some(props) = schema["input_schema"]["properties"].as_object() {
                prompt.push_str("Parameters:\n");
                let required: Vec<&str> = schema["input_schema"]["required"]
                    .as_array()
                    .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();

                for (param, spec) in props {
                    let param_type = spec["type"].as_str().unwrap_or("string");
                    let param_desc = spec["description"].as_str().unwrap_or("");
                    let req = if required.contains(&param.as_str()) {
                        " (required)"
                    } else {
                        ""
                    };
                    prompt.push_str(&format!("- `{param}` ({param_type}{req}): {param_desc}\n"));
                }
                prompt.push('\n');
            }
        }

        prompt
    }

    /// Inject tool schemas into messages by appending to the system prompt.
    fn inject_tools(&self, messages: &[Message]) -> Vec<Message> {
        let tool_prompt = self.tool_prompt();
        if tool_prompt.is_empty() {
            return messages.to_vec();
        }

        let mut result = Vec::with_capacity(messages.len());
        let mut system_found = false;

        for msg in messages {
            if msg.role == Role::System && !system_found {
                // Append tool descriptions to the system prompt
                result.push(Message {
                    role: Role::System,
                    content: format!("{}{}", msg.content, tool_prompt),
                });
                system_found = true;
            } else {
                result.push(msg.clone());
            }
        }

        // If no system message existed, add one with just the tools
        if !system_found && !tool_prompt.is_empty() {
            result.insert(
                0,
                Message {
                    role: Role::System,
                    content: tool_prompt,
                },
            );
        }

        result
    }
}

/// Parse <tool_call> blocks from model text output.
/// Returns the cleaned text (with tool_call blocks removed) and parsed tool calls.
fn parse_tool_calls(text: &str) -> (String, Vec<ToolCall>) {
    let mut calls = Vec::new();
    let mut clean_text = String::new();
    let mut remaining = text;
    let mut call_idx = 0;

    while let Some(start) = remaining.find("<tool_call>") {
        // Add text before the tool call
        clean_text.push_str(&remaining[..start]);

        let after_tag = &remaining[start + "<tool_call>".len()..];
        if let Some(end) = after_tag.find("</tool_call>") {
            let json_str = after_tag[..end].trim();

            // Parse the JSON tool call
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json_str) {
                let name = parsed["name"].as_str().unwrap_or("unknown").to_string();
                let input = parsed["input"].clone();

                calls.push(ToolCall {
                    id: format!("shim_{call_idx}"),
                    name,
                    input,
                });
                call_idx += 1;
            }

            remaining = &after_tag[end + "</tool_call>".len()..];
        } else {
            // Unclosed tag — include it as text
            clean_text.push_str(&remaining[..start + "<tool_call>".len()]);
            remaining = after_tag;
        }
    }

    clean_text.push_str(remaining);

    (clean_text.trim().to_string(), calls)
}

/// Process an AiResponse to extract shimmed tool calls from the text.
fn process_response(mut response: AiResponse) -> AiResponse {
    if response.content.contains("<tool_call>") {
        let (clean_text, calls) = parse_tool_calls(&response.content);
        response.content = clean_text;
        response.tool_calls = calls;
    }
    response
}

impl AiProvider for ToolShimProvider {
    fn send_message(
        &self,
        messages: &[Message],
        config: &ModelConfig,
    ) -> Pin<Box<dyn Future<Output = Result<AiResponse>> + Send + '_>> {
        let injected = self.inject_tools(messages);
        let config = config.clone();
        Box::pin(async move {
            let response = self.inner.send_message(&injected, &config).await?;
            Ok(process_response(response))
        })
    }

    fn stream_message(
        &self,
        messages: &[Message],
        config: &ModelConfig,
        on_chunk: Box<dyn Fn(String) + Send + Sync>,
    ) -> Pin<Box<dyn Future<Output = Result<AiResponse>> + Send + '_>> {
        let injected = self.inject_tools(messages);
        let config = config.clone();
        // For shimmed providers, we collect the full stream first then parse.
        // Tool calls in text can't be detected mid-stream.
        Box::pin(async move {
            let response = self
                .inner
                .stream_message(&injected, &config, on_chunk)
                .await?;
            Ok(process_response(response))
        })
    }

    fn is_configured(&self) -> bool {
        self.inner.is_configured()
    }

    fn provider_name(&self) -> &str {
        self.inner.provider_name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_single_tool_call() {
        let text = "I'll read the file for you.\n\n<tool_call>\n{\"name\": \"file_read\", \"input\": {\"file_path\": \"/tmp/test.rs\"}}\n</tool_call>";

        let (clean, calls) = parse_tool_calls(text);

        assert_eq!(clean, "I'll read the file for you.");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "file_read");
        assert_eq!(calls[0].input["file_path"], "/tmp/test.rs");
        assert_eq!(calls[0].id, "shim_0");
    }

    #[test]
    fn test_parse_multiple_tool_calls() {
        let text = "Let me check.\n\n<tool_call>\n{\"name\": \"file_read\", \"input\": {\"file_path\": \"a.rs\"}}\n</tool_call>\n\nAnd also:\n\n<tool_call>\n{\"name\": \"grep\", \"input\": {\"pattern\": \"fn main\"}}\n</tool_call>";

        let (clean, calls) = parse_tool_calls(text);

        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "file_read");
        assert_eq!(calls[1].name, "grep");
        assert!(clean.contains("Let me check."));
        assert!(clean.contains("And also:"));
    }

    #[test]
    fn test_parse_no_tool_calls() {
        let text = "Just a normal response with no tools.";
        let (clean, calls) = parse_tool_calls(text);

        assert_eq!(clean, text);
        assert!(calls.is_empty());
    }

    #[test]
    fn test_parse_malformed_json() {
        let text = "<tool_call>\nnot valid json\n</tool_call>";
        let (clean, calls) = parse_tool_calls(text);

        assert_eq!(clean, ""); // tag removed even if JSON fails
        assert!(calls.is_empty());
    }

    #[test]
    fn test_parse_unclosed_tag() {
        let text = "text before <tool_call> unclosed content";
        let (clean, calls) = parse_tool_calls(text);

        assert!(clean.contains("<tool_call>"));
        assert!(calls.is_empty());
    }

    #[test]
    fn test_tool_prompt_generation() {
        let schemas = vec![serde_json::json!({
            "name": "file_read",
            "description": "Read a file",
            "input_schema": {
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Path to the file"
                    }
                },
                "required": ["file_path"]
            }
        })];

        let shim = ToolShimProvider::new(Arc::new(crate::mock::MockProvider::new("test")), schemas);

        let prompt = shim.tool_prompt();
        assert!(prompt.contains("file_read"));
        assert!(prompt.contains("<tool_call>"));
        assert!(prompt.contains("file_path"));
        assert!(prompt.contains("(required)"));
    }

    #[test]
    fn test_inject_into_system_message() {
        let schemas = vec![serde_json::json!({
            "name": "bash",
            "description": "Run a command",
            "input_schema": {"type": "object", "properties": {}}
        })];

        let shim = ToolShimProvider::new(Arc::new(crate::mock::MockProvider::new("test")), schemas);

        let messages = vec![
            Message {
                role: Role::System,
                content: "You are helpful.".to_string(),
            },
            Message {
                role: Role::User,
                content: "Hi".to_string(),
            },
        ];

        let injected = shim.inject_tools(&messages);
        assert_eq!(injected.len(), 2);
        assert!(injected[0].content.contains("You are helpful."));
        assert!(injected[0].content.contains("bash"));
        assert!(injected[0].content.contains("<tool_call>"));
    }
}
