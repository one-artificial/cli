use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Identifies which AI provider/model a session uses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub provider: Provider,
    pub model: String,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            provider: Provider::Ollama, // No default provider — resolved at runtime
            model: String::new(),
            max_tokens: 8000,
            temperature: None,
        }
    }
}

/// Infer the provider from a model name.
/// Returns None if the model name doesn't match any known pattern.
pub fn infer_provider(model: &str) -> Option<Provider> {
    let lower = model.to_lowercase();

    // Anthropic: claude-*
    if lower.starts_with("claude-") || lower.starts_with("claude_") {
        return Some(Provider::Anthropic);
    }

    // Shortcuts
    if matches!(lower.as_str(), "opus" | "sonnet" | "haiku") {
        return Some(Provider::Anthropic);
    }

    // OpenAI: gpt-*, o1-*, o3-*, chatgpt-*
    if lower.starts_with("gpt-")
        || lower.starts_with("o1-")
        || lower.starts_with("o3-")
        || lower.starts_with("chatgpt-")
    {
        return Some(Provider::OpenAI);
    }

    // Google: gemini-*
    if lower.starts_with("gemini-") {
        return Some(Provider::Google);
    }

    // HuggingFace: org/model format (contains '/')
    if model.contains('/') {
        return Some(Provider::HuggingFace);
    }

    None
}

/// Resolve a model shortcut to its full model ID.
pub fn resolve_model_shortcut(model: &str) -> &str {
    match model {
        "opus" => "claude-opus-4-6",
        "sonnet" => "claude-sonnet-4-20250514",
        "haiku" => "claude-haiku-4-5-20251001",
        other => other,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provider {
    Anthropic,
    OpenAI,
    Ollama,
    Google,
    HuggingFace,
    LmStudio,
}

impl std::fmt::Display for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Provider::Anthropic => write!(f, "Anthropic"),
            Provider::OpenAI => write!(f, "OpenAI"),
            Provider::Ollama => write!(f, "Ollama"),
            Provider::Google => write!(f, "Google"),
            Provider::HuggingFace => write!(f, "HuggingFace"),
            Provider::LmStudio => write!(f, "LM Studio"),
        }
    }
}

/// Describes what a provider supports — used to adapt tool schemas,
/// system prompts, and request formatting per provider.
pub struct ProviderCapabilities {
    /// Whether this provider supports native tool use in its API.
    pub supports_tool_use: bool,
    /// Tool schema format: "anthropic" (content blocks) or "openai" (function calling).
    pub tool_format: ToolFormat,
    /// Whether the provider supports streaming responses.
    pub supports_streaming: bool,
    /// Whether the provider supports system messages.
    pub supports_system_message: bool,
    /// Whether the provider supports extended thinking / chain-of-thought.
    pub supports_thinking: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolFormat {
    /// Anthropic: tool_use content blocks in assistant messages.
    Anthropic,
    /// OpenAI: function calling with tool_calls array.
    OpenAI,
    /// No native tool support — tool use must be shimmed via prompting.
    None,
}

impl Provider {
    pub fn capabilities(&self) -> ProviderCapabilities {
        match self {
            Provider::Anthropic => ProviderCapabilities {
                supports_tool_use: true,
                tool_format: ToolFormat::Anthropic,
                supports_streaming: true,
                supports_system_message: true,
                supports_thinking: true,
            },
            Provider::OpenAI => ProviderCapabilities {
                supports_tool_use: true,
                tool_format: ToolFormat::OpenAI,
                supports_streaming: true,
                supports_system_message: true,
                supports_thinking: false,
            },
            Provider::HuggingFace => ProviderCapabilities {
                supports_tool_use: true,
                tool_format: ToolFormat::OpenAI, // HF uses OpenAI-compatible format
                supports_streaming: true,
                supports_system_message: true,
                supports_thinking: false,
            },
            Provider::Ollama => ProviderCapabilities {
                supports_tool_use: false,
                tool_format: ToolFormat::None,
                supports_streaming: true,
                supports_system_message: true,
                supports_thinking: false,
            },
            Provider::Google => ProviderCapabilities {
                supports_tool_use: true,
                tool_format: ToolFormat::OpenAI,
                supports_streaming: true,
                supports_system_message: true,
                supports_thinking: false,
            },
            Provider::LmStudio => ProviderCapabilities {
                supports_tool_use: true,
                tool_format: ToolFormat::OpenAI,
                supports_streaming: true,
                supports_system_message: true,
                supports_thinking: false,
            },
        }
    }
}

/// Trait that AI provider implementations must satisfy.
/// Defined here in core so that one-ai and one-tui both
/// depend on the trait without depending on each other.
///
/// Uses boxed futures for dyn-compatibility — we store providers as
/// `Box<dyn AiProvider>` to support runtime provider switching.
pub trait AiProvider: Send + Sync {
    fn send_message(
        &self,
        messages: &[Message],
        config: &ModelConfig,
    ) -> Pin<Box<dyn Future<Output = Result<AiResponse>> + Send + '_>>;

    fn stream_message(
        &self,
        messages: &[Message],
        config: &ModelConfig,
        on_chunk: Box<dyn Fn(String) + Send + Sync>,
    ) -> Pin<Box<dyn Future<Output = Result<AiResponse>> + Send + '_>>;

    /// Returns true if the provider has valid credentials configured.
    /// Providers that require an API key should return false when the key is empty.
    fn is_configured(&self) -> bool {
        true
    }

    /// Human-readable provider name for error messages (e.g. "anthropic", "openai").
    fn provider_name(&self) -> &str {
        "unknown"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone)]
pub struct AiResponse {
    pub content: String,
    pub usage: Usage,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}
