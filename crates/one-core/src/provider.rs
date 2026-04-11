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
    /// Extended thinking budget for this specific request (0 or None = disabled).
    /// Set per-request by the query engine from ResolvedEffort — not a session default.
    #[serde(default)]
    pub budget_tokens: Option<u32>,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            provider: Provider::Ollama, // No default provider — resolved at runtime
            model: String::new(),
            max_tokens: 8000,
            temperature: None,
            budget_tokens: None,
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

// ── Effort / capability system ────────────────────────────────────────────────

/// Hard limits for a specific model — context window, output ceiling, thinking support.
#[derive(Debug, Clone)]
pub struct ModelCapabilities {
    /// Total input + output context window in tokens.
    pub context_window: u32,
    /// Maximum output tokens the model will produce.
    pub max_output_tokens: u32,
    /// Whether extended thinking / chain-of-thought is supported.
    pub supports_thinking: bool,
    /// Maximum budget_tokens allowed (0 if unsupported).
    pub max_thinking_budget: u32,
}

/// Concrete per-request parameters derived from effort level + model + context state.
#[derive(Debug, Clone)]
pub struct ResolvedEffort {
    /// Extended thinking token budget (0 = thinking disabled).
    pub budget_tokens: u32,
    /// Output token ceiling for this request.
    pub max_tokens: u32,
    /// Evergreen compression target (% of context window). NaN = no target (max effort).
    pub evergreen_target_pct: f32,
    /// Human-readable label for TUI display, e.g. "medium" or "auto→high".
    pub label: String,
}

/// Complexity hint derived cheaply from the user message — used by auto effort mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageComplexity {
    Low,
    Medium,
    High,
}

/// Return the known capability limits for a model name.
/// Falls back to conservative defaults for unrecognised models.
pub fn model_capabilities(model: &str) -> ModelCapabilities {
    let lower = model.to_lowercase();

    if lower.contains("opus-4") {
        return ModelCapabilities {
            context_window: 200_000,
            max_output_tokens: 32_000,
            supports_thinking: true,
            max_thinking_budget: 32_000,
        };
    }
    // Sonnet 4.x and 3.7 — thinking supported
    if lower.contains("sonnet-4") || lower.contains("sonnet-3-7") {
        return ModelCapabilities {
            context_window: 200_000,
            max_output_tokens: 16_000,
            supports_thinking: true,
            max_thinking_budget: 10_000,
        };
    }
    // Haiku and older Claude — no extended thinking
    if lower.contains("claude") {
        return ModelCapabilities {
            context_window: 200_000,
            max_output_tokens: 8_000,
            supports_thinking: false,
            max_thinking_budget: 0,
        };
    }
    if lower.contains("gpt-4o") || lower.contains("gpt-4-turbo") {
        return ModelCapabilities {
            context_window: 128_000,
            max_output_tokens: 16_384,
            supports_thinking: false,
            max_thinking_budget: 0,
        };
    }
    // Gemini 2.x — very large context
    if lower.contains("gemini-2") {
        return ModelCapabilities {
            context_window: 1_000_000,
            max_output_tokens: 8_192,
            supports_thinking: false,
            max_thinking_budget: 0,
        };
    }
    // Conservative fallback
    ModelCapabilities {
        context_window: 32_000,
        max_output_tokens: 4_096,
        supports_thinking: false,
        max_thinking_budget: 0,
    }
}

/// Cheap local heuristic — classifies the user message without any API call.
pub fn estimate_message_complexity(message: &str) -> MessageComplexity {
    let lower = message.to_lowercase();
    let words = message.split_whitespace().count();

    if words < 8 {
        return MessageComplexity::Low;
    }

    let has_code_fence = message.contains("```");
    let has_error_keywords = lower.contains("error")
        || lower.contains("panic")
        || lower.contains("failed")
        || lower.contains("exception")
        || lower.contains("crash");
    let has_arch_keywords = lower.contains("architect")
        || lower.contains("design")
        || lower.contains("refactor")
        || lower.contains("strategy")
        || lower.contains("system");
    let is_simple_lookup = (lower.starts_with("what ")
        || lower.starts_with("where ")
        || lower.starts_with("show ")
        || lower.starts_with("list "))
        && !has_code_fence;

    if is_simple_lookup && words < 15 {
        return MessageComplexity::Low;
    }
    if has_arch_keywords || (has_code_fence && words > 60) || (has_error_keywords && has_code_fence)
    {
        return MessageComplexity::High;
    }

    MessageComplexity::Medium
}

/// Compute concrete request parameters from effort + model capabilities + context state.
///
/// `effort`       – None or "auto" = auto mode; otherwise "low" | "medium" | "high" | "max"
/// `caps`         – capability limits for the active model
/// `min_ctx_pct`  – current minimum context % (0.0–100.0). Pass 0.0 until Evergreen is live.
/// `complexity`   – cheaply estimated from the user message; only used in auto mode
pub fn resolve_effort(
    effort: Option<&str>,
    caps: &ModelCapabilities,
    min_ctx_pct: f32,
    complexity: MessageComplexity,
) -> ResolvedEffort {
    // How many input tokens are already committed on every request (from evergreen context).
    let committed = (caps.context_window as f32 * min_ctx_pct / 100.0) as u32;
    let free_input = caps.context_window.saturating_sub(committed);

    match effort.unwrap_or("auto") {
        "low" => ResolvedEffort {
            budget_tokens: 0,
            max_tokens: 2_048.min(caps.max_output_tokens),
            evergreen_target_pct: 5.0,
            label: "low".into(),
        },
        "medium" => ResolvedEffort {
            budget_tokens: if caps.supports_thinking {
                (caps.max_thinking_budget / 5).clamp(1_024, caps.max_thinking_budget)
            } else {
                0
            },
            max_tokens: 8_000.min(caps.max_output_tokens),
            evergreen_target_pct: 15.0,
            label: "medium".into(),
        },
        "high" => ResolvedEffort {
            budget_tokens: if caps.supports_thinking {
                (caps.max_thinking_budget / 2).clamp(2_048, caps.max_thinking_budget)
            } else {
                0
            },
            max_tokens: 16_000.min(caps.max_output_tokens),
            evergreen_target_pct: 30.0,
            label: "high".into(),
        },
        "max" => ResolvedEffort {
            budget_tokens: if caps.supports_thinking {
                (caps.max_thinking_budget * 4 / 5).min(caps.max_thinking_budget)
            } else {
                0
            },
            max_tokens: caps.max_output_tokens,
            evergreen_target_pct: f32::NAN, // no compression target
            label: "max".into(),
        },
        // "auto" — dynamic: pick a base level from message complexity, then
        // back it off one notch when context headroom is tight (>60% committed).
        _ => {
            let base = match complexity {
                MessageComplexity::Low => 0u8,
                MessageComplexity::Medium => 1,
                MessageComplexity::High => 2,
            };
            let level = if min_ctx_pct > 60.0 {
                base.saturating_sub(1)
            } else {
                base
            };

            match level {
                0 => ResolvedEffort {
                    budget_tokens: 0,
                    max_tokens: 4_096.min(caps.max_output_tokens),
                    evergreen_target_pct: 10.0,
                    label: "auto→low".into(),
                },
                1 => {
                    let budget = if caps.supports_thinking {
                        (free_input / 4).min(caps.max_thinking_budget)
                    } else {
                        0
                    };
                    ResolvedEffort {
                        budget_tokens: budget,
                        max_tokens: 8_000.min(caps.max_output_tokens),
                        evergreen_target_pct: 15.0,
                        label: "auto→medium".into(),
                    }
                }
                _ => {
                    let budget = if caps.supports_thinking {
                        (free_input * 2 / 5)
                            .clamp(2_048, caps.max_thinking_budget)
                            .min(caps.max_thinking_budget)
                    } else {
                        0
                    };
                    ResolvedEffort {
                        budget_tokens: budget,
                        max_tokens: 16_000.min(caps.max_output_tokens),
                        evergreen_target_pct: 30.0,
                        label: "auto→high".into(),
                    }
                }
            }
        }
    }
}
