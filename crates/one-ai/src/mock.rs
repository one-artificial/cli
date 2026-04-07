use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use one_core::provider::{AiProvider, AiResponse, Message, ModelConfig, Usage};

/// A mock AI provider for testing. Returns canned responses
/// without making any network calls.
pub struct MockProvider {
    response: String,
}

impl MockProvider {
    pub fn new(response: impl Into<String>) -> Self {
        Self {
            response: response.into(),
        }
    }

    /// Create a mock that echoes the last user message.
    pub fn echo() -> Self {
        Self {
            response: "__ECHO__".to_string(),
        }
    }
}

impl AiProvider for MockProvider {
    fn provider_name(&self) -> &str {
        "mock"
    }

    fn send_message(
        &self,
        messages: &[Message],
        _config: &ModelConfig,
    ) -> Pin<Box<dyn Future<Output = Result<AiResponse>> + Send + '_>> {
        let content = if self.response == "__ECHO__" {
            messages
                .iter()
                .rev()
                .find(|m| m.role == one_core::provider::Role::User)
                .map(|m| format!("Echo: {}", m.content))
                .unwrap_or_else(|| "No user message".to_string())
        } else {
            self.response.clone()
        };

        Box::pin(async move {
            Ok(AiResponse {
                content,
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 20,
                },
                tool_calls: Vec::new(),
            })
        })
    }

    fn stream_message(
        &self,
        messages: &[Message],
        _config: &ModelConfig,
        on_chunk: Box<dyn Fn(String) + Send + Sync>,
    ) -> Pin<Box<dyn Future<Output = Result<AiResponse>> + Send + '_>> {
        let content = if self.response == "__ECHO__" {
            messages
                .iter()
                .rev()
                .find(|m| m.role == one_core::provider::Role::User)
                .map(|m| format!("Echo: {}", m.content))
                .unwrap_or_else(|| "No user message".to_string())
        } else {
            self.response.clone()
        };

        // Simulate streaming by sending the content in chunks
        let chunks: Vec<String> = content
            .split_whitespace()
            .map(|w| format!("{w} "))
            .collect();

        Box::pin(async move {
            for chunk in &chunks {
                on_chunk(chunk.clone());
            }

            Ok(AiResponse {
                content: chunks.join(""),
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 20,
                },
                tool_calls: Vec::new(),
            })
        })
    }
}
