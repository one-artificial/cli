use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use one_core::provider::{AiProvider, AiResponse, Message, ModelConfig, Role, ToolCall, Usage};
use reqwest::header::{HeaderMap, HeaderValue};

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const MAX_RETRIES: u32 = 3;
const ANTHROPIC_VERSION: &str = "2023-06-01";
const BETA_FLAGS: &str = "interleaved-thinking-2025-05-14,prompt-caching-2024-11-20";

pub struct AnthropicProvider {
    api_key: String,
    client: reqwest::Client,
    base_url: String,
    tool_schemas: Vec<serde_json::Value>,
}

impl AnthropicProvider {
    pub fn new(api_key: String) -> Self {
        let base_url = std::env::var("ANTHROPIC_BASE_URL")
            .unwrap_or_else(|_| DEFAULT_BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string();

        let client = reqwest::Client::builder()
            .pool_max_idle_per_host(4)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            api_key,
            client,
            base_url,
            tool_schemas: Vec::new(),
        }
    }

    pub fn with_tools(mut self, schemas: Vec<serde_json::Value>) -> Self {
        self.tool_schemas = schemas;
        self
    }

    fn api_url(&self) -> String {
        format!("{}/v1/messages", self.base_url)
    }

    fn headers(&self) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-api-key", HeaderValue::from_str(&self.api_key).unwrap());
        h.insert(
            "anthropic-version",
            HeaderValue::from_static(ANTHROPIC_VERSION),
        );
        h.insert("content-type", HeaderValue::from_static("application/json"));
        h.insert("anthropic-beta", HeaderValue::from_static(BETA_FLAGS));
        h
    }

    fn build_request_body(&self, messages: &[Message], config: &ModelConfig) -> serde_json::Value {
        let api_messages: Vec<serde_json::Value> = messages
            .iter()
            .filter(|m| m.role != Role::System)
            .map(|m| {
                let role = match m.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                    Role::System => unreachable!(),
                };
                let content = if m.content.starts_with('[') {
                    serde_json::from_str::<serde_json::Value>(&m.content)
                        .unwrap_or_else(|_| serde_json::json!(m.content))
                } else {
                    serde_json::json!(m.content)
                };
                serde_json::json!({ "role": role, "content": content })
            })
            .collect();

        let system = messages
            .iter()
            .find(|m| m.role == Role::System)
            .map(|m| m.content.as_str());

        let mut body = serde_json::json!({
            "model": config.model,
            "max_tokens": config.max_tokens,
            "messages": api_messages,
            "stream": true,
        });

        // Thinking: enabled with budget for models that support it
        let budget = config.max_tokens.saturating_sub(1);
        if budget > 0 {
            body["thinking"] = serde_json::json!({
                "type": "enabled",
                "budget_tokens": budget
            });
        }

        // System prompt
        if let Some(sys) = system {
            body["system"] = serde_json::json!([
                {
                    "type": "text",
                    "text": sys,
                    "cache_control": { "type": "ephemeral" }
                }
            ]);
        }

        if let Some(temp) = config.temperature {
            body["temperature"] = serde_json::json!(temp);
        }

        if !self.tool_schemas.is_empty() {
            body["tools"] = serde_json::json!(self.tool_schemas);
        }

        body
    }

    async fn do_streaming_request(
        &self,
        body: &serde_json::Value,
        on_chunk: &(dyn Fn(String) + Send + Sync),
    ) -> Result<AiResponse> {
        let mut last_error = String::new();

        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                let delay_ms = if attempt <= 3 {
                    100 * attempt as u64 + fastrand::u64(0..50)
                } else {
                    let base_ms = (500u64 * (1u64 << (attempt - 1))).min(32000);
                    base_ms + fastrand::u64(0..=(base_ms / 4)).max(1)
                };
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            }

            let resp = self
                .client
                .post(self.api_url())
                .headers(self.headers())
                .json(body)
                .send()
                .await?;

            if resp.status().is_success() {
                let mut full_content = String::new();
                let mut usage = Usage::default();
                let mut tool_calls: Vec<ToolCall> = Vec::new();
                let mut current_tool_id = String::new();
                let mut current_tool_name = String::new();
                let mut current_tool_input = String::new();
                let mut in_tool_use = false;
                let mut in_thinking = false;

                let mut stream = resp.bytes_stream();
                let mut buffer = String::new();

                use futures_util::StreamExt;

                while let Some(chunk_result) = stream.next().await {
                    let chunk = chunk_result?;
                    buffer.push_str(&String::from_utf8_lossy(&chunk));

                    while let Some(boundary) = buffer.find("\n\n") {
                        let event_text = buffer[..boundary].to_string();
                        buffer = buffer[boundary + 2..].to_string();

                        let mut event_type = String::new();
                        let mut event_data = String::new();

                        for line in event_text.lines() {
                            if let Some(t) = line.strip_prefix("event: ") {
                                event_type = t.to_string();
                            } else if let Some(d) = line.strip_prefix("data: ") {
                                event_data = d.to_string();
                            }
                        }

                        if event_data.is_empty() {
                            continue;
                        }

                        let data: serde_json::Value = match serde_json::from_str(&event_data) {
                            Ok(d) => d,
                            Err(_) => continue,
                        };

                        match event_type.as_str() {
                            "content_block_start" => match data["content_block"]["type"].as_str() {
                                Some("tool_use") => {
                                    in_tool_use = true;
                                    current_tool_id = data["content_block"]["id"]
                                        .as_str()
                                        .unwrap_or("")
                                        .to_string();
                                    current_tool_name = data["content_block"]["name"]
                                        .as_str()
                                        .unwrap_or("")
                                        .to_string();
                                    current_tool_input.clear();
                                }
                                Some("thinking") => {
                                    in_thinking = true;
                                }
                                _ => {}
                            },
                            "content_block_delta" => {
                                if in_tool_use {
                                    if let Some(partial) = data["delta"]["partial_json"].as_str() {
                                        current_tool_input.push_str(partial);
                                    }
                                } else if in_thinking {
                                    // Thinking blocks — silently consume for now.
                                    // TODO: surface thinking duration in TUI status.
                                } else if let Some(text) = data["delta"]["text"].as_str() {
                                    full_content.push_str(text);
                                    on_chunk(text.to_string());
                                }
                            }
                            "content_block_stop" => {
                                if in_tool_use {
                                    let input: serde_json::Value =
                                        serde_json::from_str(&current_tool_input)
                                            .unwrap_or(serde_json::Value::Null);
                                    tool_calls.push(ToolCall {
                                        id: current_tool_id.clone(),
                                        name: current_tool_name.clone(),
                                        input,
                                    });
                                    in_tool_use = false;
                                } else if in_thinking {
                                    in_thinking = false;
                                }
                            }
                            "message_delta" => {
                                if let Some(u) = data["usage"]["output_tokens"].as_u64() {
                                    usage.output_tokens = u as u32;
                                }
                            }
                            "message_start" => {
                                if let Some(u) = data["message"]["usage"]["input_tokens"].as_u64() {
                                    usage.input_tokens = u as u32;
                                }
                            }
                            _ => {}
                        }
                    }
                }

                return Ok(AiResponse {
                    content: full_content,
                    usage,
                    tool_calls,
                });
            }

            // Error handling
            let status = resp.status();
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .map(String::from);
            let body_text = resp.text().await.unwrap_or_default();
            let (error, retryable) = Self::parse_error(status, &body_text, retry_after.as_deref());
            last_error = error.clone();

            if !retryable {
                break;
            }

            if attempt < MAX_RETRIES {
                tracing::warn!(
                    "Retryable error (attempt {}/{}), retrying...",
                    attempt + 1,
                    MAX_RETRIES + 1
                );
            }
        }

        anyhow::bail!("{last_error}");
    }

    fn parse_error(
        status: reqwest::StatusCode,
        body: &str,
        retry_after: Option<&str>,
    ) -> (String, bool) {
        let data: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
        let err_msg = data["error"]["message"]
            .as_str()
            .unwrap_or("Unknown API error");
        let err_type = data["error"]["type"].as_str().unwrap_or("");

        let is_retryable = matches!(err_type, "rate_limit_error" | "overloaded_error")
            || status == reqwest::StatusCode::TOO_MANY_REQUESTS
            || status == reqwest::StatusCode::SERVICE_UNAVAILABLE;

        let hint = match err_type {
            "rate_limit_error" => {
                let wait = retry_after.unwrap_or("a moment");
                format!("\n\nRate limited. Wait {wait} and try again.")
            }
            "authentication_error" | "permission_error" => {
                "\n\nCheck your API key with /login anthropic <key>".to_string()
            }
            "invalid_request_error" if err_msg.contains("credit balance") => {
                "\n\nYour account needs API credits. Visit: console.anthropic.com/settings/billing"
                    .to_string()
            }
            "overloaded_error" => "\n\nAPI overloaded. Retrying...".to_string(),
            _ => format!(" ({status})"),
        };

        (format!("{err_msg}{hint}"), is_retryable)
    }
}

impl AiProvider for AnthropicProvider {
    fn is_configured(&self) -> bool {
        !self.api_key.is_empty()
    }

    fn provider_name(&self) -> &str {
        "anthropic"
    }

    fn send_message(
        &self,
        messages: &[Message],
        config: &ModelConfig,
    ) -> Pin<Box<dyn Future<Output = Result<AiResponse>> + Send + '_>> {
        let body = self.build_request_body(messages, config);
        Box::pin(async move { self.do_streaming_request(&body, &|_| {}).await })
    }

    fn stream_message(
        &self,
        messages: &[Message],
        config: &ModelConfig,
        on_chunk: Box<dyn Fn(String) + Send + Sync>,
    ) -> Pin<Box<dyn Future<Output = Result<AiResponse>> + Send + '_>> {
        let body = self.build_request_body(messages, config);
        Box::pin(async move { self.do_streaming_request(&body, &on_chunk).await })
    }
}
