//! Shared base for OpenAI-compatible chat completion providers.
//!
//! Used by: OpenAI, HuggingFace, LM Studio, and any provider that
//! implements the OpenAI Chat Completions API format.
//!
//! Handles: request body construction, SSE streaming, tool call parsing,
//! and error extraction. Each provider only needs to specify its endpoint,
//! auth headers, and any provider-specific overrides.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use one_core::provider::{AiProvider, AiResponse, Message, ModelConfig, Role, ToolCall, Usage};

/// Configuration for an OpenAI-compatible provider.
pub struct OpenAiCompatConfig {
    /// Base URL for the API (e.g. "https://api.openai.com/v1")
    pub base_url: String,
    /// Provider name for error messages.
    pub name: String,
    /// Default model when none is specified.
    pub default_model: String,
    /// Whether to include tool schemas in API requests.
    pub send_tools: bool,
}

/// Shared OpenAI-compatible provider implementation.
pub struct OpenAiCompatProvider {
    pub config: OpenAiCompatConfig,
    api_key: String,
    client: reqwest::Client,
    tool_schemas: Vec<serde_json::Value>,
}

impl OpenAiCompatProvider {
    pub fn new(config: OpenAiCompatConfig, api_key: String) -> Self {
        Self {
            config,
            api_key,
            client: reqwest::Client::new(),
            tool_schemas: Vec::new(),
        }
    }

    /// Add tool schemas (in OpenAI function calling format).
    pub fn with_tools(mut self, schemas: Vec<serde_json::Value>) -> Self {
        self.tool_schemas = schemas;
        self
    }

    fn api_url(&self) -> String {
        format!("{}/chat/completions", self.config.base_url)
    }

    fn build_request_body(
        &self,
        messages: &[Message],
        config: &ModelConfig,
        stream: bool,
    ) -> serde_json::Value {
        let api_messages: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                    Role::System => "system",
                };
                // Content may be a JSON array (tool_call results) or plain string
                let content = if m.content.starts_with('[') {
                    serde_json::from_str::<serde_json::Value>(&m.content)
                        .unwrap_or_else(|_| serde_json::json!(m.content))
                } else {
                    serde_json::json!(m.content)
                };
                serde_json::json!({ "role": role, "content": content })
            })
            .collect();

        let model = if config.model.is_empty() {
            &self.config.default_model
        } else {
            &config.model
        };

        let mut body = serde_json::json!({
            "model": model,
            "max_tokens": config.max_tokens,
            "messages": api_messages,
        });

        if stream {
            body["stream"] = serde_json::json!(true);
        }

        if let Some(temp) = config.temperature {
            body["temperature"] = serde_json::json!(temp);
        }

        // Include tool schemas in OpenAI function calling format
        if self.config.send_tools && !self.tool_schemas.is_empty() {
            let tools: Vec<serde_json::Value> = self
                .tool_schemas
                .iter()
                .map(|schema| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": schema["name"],
                            "description": schema["description"],
                            "parameters": schema["input_schema"],
                        }
                    })
                })
                .collect();
            body["tools"] = serde_json::json!(tools);
        }

        body
    }

    async fn do_send(&self, body: &serde_json::Value) -> Result<AiResponse> {
        let resp = self
            .client
            .post(self.api_url())
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(body)
            .send()
            .await?;

        let status = resp.status();
        let data: serde_json::Value = resp.json().await?;

        if !status.is_success() {
            let err_msg = data["error"]["message"]
                .as_str()
                .or_else(|| data["error"].as_str())
                .or_else(|| data["message"].as_str())
                .unwrap_or("Unknown API error");
            anyhow::bail!("{} API error ({}): {}", self.config.name, status, err_msg);
        }

        let content = data["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();

        let tool_calls = extract_tool_calls(&data);

        let usage = Usage {
            input_tokens: data["usage"]["prompt_tokens"].as_u64().unwrap_or(0) as u32,
            output_tokens: data["usage"]["completion_tokens"].as_u64().unwrap_or(0) as u32,
        };

        Ok(AiResponse {
            content,
            usage,
            tool_calls,
        })
    }

    async fn do_stream(
        &self,
        body: &serde_json::Value,
        on_chunk: &(dyn Fn(String) + Send + Sync),
    ) -> Result<AiResponse> {
        let resp = self
            .client
            .post(self.api_url())
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            anyhow::bail!("{} API error ({}): {}", self.config.name, status, body_text);
        }

        let mut full_content = String::new();
        let mut usage = Usage::default();
        let mut stream = resp.bytes_stream();
        let mut buffer = String::new();

        // Accumulate streaming tool calls
        let mut pending_tools: HashMap<usize, (String, String, String)> = HashMap::new();

        use futures_util::StreamExt;

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(boundary) = buffer.find("\n\n") {
                let event_text = buffer[..boundary].to_string();
                buffer = buffer[boundary + 2..].to_string();

                for line in event_text.lines() {
                    if let Some(data_str) = line.strip_prefix("data: ") {
                        if data_str == "[DONE]" {
                            continue;
                        }

                        if let Ok(data) = serde_json::from_str::<serde_json::Value>(data_str) {
                            // Text content
                            if let Some(delta) = data["choices"][0]["delta"]["content"].as_str() {
                                full_content.push_str(delta);
                                on_chunk(delta.to_string());
                            }

                            // Streaming tool calls (OpenAI format)
                            if let Some(tc_deltas) =
                                data["choices"][0]["delta"]["tool_calls"].as_array()
                            {
                                for tc in tc_deltas {
                                    let idx = tc["index"].as_u64().unwrap_or(0) as usize;
                                    let entry = pending_tools.entry(idx).or_insert_with(|| {
                                        (String::new(), String::new(), String::new())
                                    });

                                    if let Some(id) = tc["id"].as_str() {
                                        entry.0 = id.to_string();
                                    }
                                    if let Some(name) = tc["function"]["name"].as_str() {
                                        entry.1 = name.to_string();
                                    }
                                    if let Some(args) = tc["function"]["arguments"].as_str() {
                                        entry.2.push_str(args);
                                    }
                                }
                            }

                            // Usage (sometimes in final chunk)
                            if let Some(u) = data["usage"]["prompt_tokens"].as_u64() {
                                usage.input_tokens = u as u32;
                            }
                            if let Some(u) = data["usage"]["completion_tokens"].as_u64() {
                                usage.output_tokens = u as u32;
                            }
                        }
                    }
                }
            }
        }

        // Finalize accumulated tool calls
        let mut tool_calls = Vec::new();
        let mut indices: Vec<usize> = pending_tools.keys().copied().collect();
        indices.sort();
        for idx in indices {
            if let Some((id, name, args)) = pending_tools.remove(&idx)
                && !name.is_empty()
            {
                let input: serde_json::Value =
                    serde_json::from_str(&args).unwrap_or(serde_json::Value::Null);
                tool_calls.push(ToolCall { id, name, input });
            }
        }

        Ok(AiResponse {
            content: full_content,
            usage,
            tool_calls,
        })
    }
}

impl AiProvider for OpenAiCompatProvider {
    fn is_configured(&self) -> bool {
        !self.api_key.is_empty()
    }

    fn provider_name(&self) -> &str {
        &self.config.name
    }

    fn send_message(
        &self,
        messages: &[Message],
        config: &ModelConfig,
    ) -> Pin<Box<dyn Future<Output = Result<AiResponse>> + Send + '_>> {
        let body = self.build_request_body(messages, config, false);
        Box::pin(async move { self.do_send(&body).await })
    }

    fn stream_message(
        &self,
        messages: &[Message],
        config: &ModelConfig,
        on_chunk: Box<dyn Fn(String) + Send + Sync>,
    ) -> Pin<Box<dyn Future<Output = Result<AiResponse>> + Send + '_>> {
        let body = self.build_request_body(messages, config, true);
        Box::pin(async move { self.do_stream(&body, &on_chunk).await })
    }
}

/// Extract tool calls from a non-streaming OpenAI response.
fn extract_tool_calls(data: &serde_json::Value) -> Vec<ToolCall> {
    let Some(calls) = data["choices"][0]["message"]["tool_calls"].as_array() else {
        return Vec::new();
    };

    calls
        .iter()
        .filter_map(|call| {
            let id = call["id"].as_str()?.to_string();
            let name = call["function"]["name"].as_str()?.to_string();
            let args_str = call["function"]["arguments"].as_str()?;
            let input: serde_json::Value = serde_json::from_str(args_str).ok()?;
            Some(ToolCall { id, name, input })
        })
        .collect()
}
