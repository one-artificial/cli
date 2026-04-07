use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::provider::Provider;

// ── Effort levels ────────────────────────────────────────────────

/// Normalised effort level (0–4). Each level maps to a set of resolved
/// parameters that the provider adapter translates into real API params.
pub type Effort = u8;

pub const EFFORT_MINIMAL: Effort = 0;
pub const EFFORT_LOW: Effort = 1;
pub const EFFORT_MEDIUM: Effort = 2;
pub const EFFORT_HIGH: Effort = 3;
pub const EFFORT_MAXIMUM: Effort = 4;

/// Parse a user-facing effort string into the internal 0–4 level.
pub fn parse_effort(s: &str) -> Option<Effort> {
    match s.to_lowercase().as_str() {
        "minimal" | "min" | "0" => Some(EFFORT_MINIMAL),
        "low" | "1" => Some(EFFORT_LOW),
        "medium" | "med" | "2" => Some(EFFORT_MEDIUM),
        "high" | "3" => Some(EFFORT_HIGH),
        "maximum" | "max" | "4" => Some(EFFORT_MAXIMUM),
        _ => None,
    }
}

/// Display name for an effort level.
pub fn effort_label(effort: Effort) -> &'static str {
    match effort {
        0 => "minimal",
        1 => "low",
        2 => "medium",
        3 => "high",
        4 => "maximum",
        _ => "unknown",
    }
}

/// Unicode symbol for effort display.
pub fn effort_symbol(effort: Effort) -> &'static str {
    match effort {
        0 => "·", // dot
        1 => "○", // U+25CB empty circle
        2 => "◐", // U+25D0 half circle
        3 => "●", // U+25CF filled circle
        4 => "◉", // U+25C9 fisheye
        _ => "?",
    }
}

/// Description for an effort level.
pub fn effort_description(effort: Effort) -> &'static str {
    match effort {
        0 => "Fastest response, minimal reasoning",
        1 => "Quick, straightforward implementation",
        2 => "Balanced approach with standard testing",
        3 => "Comprehensive implementation with extensive testing",
        4 => "Maximum capability with deepest reasoning",
        _ => "",
    }
}

// ── Model descriptor ─────────────────────────────────────────────

/// How the model controls thinking/reasoning depth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThinkingBudgetType {
    /// Anthropic: type=enabled, budget_tokens=N.
    Tokens,
    /// OpenAI o-series: reasoning_effort="low"|"medium"|"high".
    Enum,
    /// Gemini 2.5+: thinkingBudget (integer tokens).
    Dynamic,
    /// Model reasons internally — no API toggle, no budget param.
    /// Qwen3, DeepSeek R1 distills, etc.
    Internal,
    /// Model has no thinking/reasoning capability.
    None,
}

/// Model tier hint for effort-based model suggestions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelTier {
    Fast,
    Balanced,
    Powerful,
}

/// The single struct the resolver needs to know about any model.
/// Register once, everything else derives from it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDescriptor {
    pub provider: Provider,
    pub model_id: String,
    /// Short slug for /model command (e.g. "opus", "o3", "devstral").
    pub slug: String,
    /// Human-readable display name.
    pub nice_name: String,
    pub tier: ModelTier,
    pub max_output_tokens: u32,
    pub supports_thinking: bool,
    pub thinking_budget_type: ThinkingBudgetType,
    pub supports_temperature: bool,
    pub supports_tools: bool,
}

// ── Resolved parameters ──────────────────────────────────────────

/// How context should be managed at this effort level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextStrategy {
    LastTurn,
    Recent,
    Full,
    Rag,
    RagSummary,
}

/// Retry behaviour at this effort level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RetryStrategy {
    None,
    Retry,
    RetryFallback,
    RetryFallbackAlert,
}

/// Tool selection mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolChoice {
    None,
    Auto,
    Required,
}

/// The fully resolved parameters for a given effort level + model.
/// Provider adapters translate these into real API params.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedParams {
    pub effort: Effort,
    pub model_tier: ModelTier,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    pub thinking: bool,
    pub thinking_budget: Option<u32>,
    pub tools_enabled: bool,
    pub tool_choice: ToolChoice,
    pub stream: bool,
    pub context_strategy: ContextStrategy,
    pub retry_strategy: RetryStrategy,
    /// Optional system prompt prefix for models without native effort controls.
    pub system_prefix: Option<&'static str>,
}

// ── Resolver ─────────────────────────────────────────────────────

/// Result of resolving an effort level against a model's capabilities.
#[derive(Debug, Clone)]
pub enum ResolveResult {
    Resolved(ResolvedParams),
    Degraded {
        params: ResolvedParams,
        requested: Effort,
        actual: Effort,
        reason: String,
    },
    Unsupported {
        reason: String,
    },
}

impl ResolveResult {
    pub fn params(&self) -> Option<&ResolvedParams> {
        match self {
            ResolveResult::Resolved(p) | ResolveResult::Degraded { params: p, .. } => Some(p),
            ResolveResult::Unsupported { .. } => None,
        }
    }
}

/// The normalised effort table.
fn effort_defaults(effort: Effort) -> ResolvedParams {
    match effort {
        0 => ResolvedParams {
            effort: 0,
            model_tier: ModelTier::Fast,
            max_tokens: 256,
            temperature: Some(0.0),
            thinking: false,
            thinking_budget: None,
            tools_enabled: false,
            tool_choice: ToolChoice::None,
            stream: false,
            context_strategy: ContextStrategy::LastTurn,
            retry_strategy: RetryStrategy::None,
            system_prefix: Some("Be concise. One-shot answers. No elaboration."),
        },
        1 => ResolvedParams {
            effort: 1,
            model_tier: ModelTier::Fast,
            max_tokens: 1024,
            temperature: Some(0.2),
            thinking: false,
            thinking_budget: None,
            tools_enabled: false,
            tool_choice: ToolChoice::None,
            stream: false,
            context_strategy: ContextStrategy::Recent,
            retry_strategy: RetryStrategy::Retry,
            system_prefix: None,
        },
        2 => ResolvedParams {
            effort: 2,
            model_tier: ModelTier::Balanced,
            max_tokens: 4096,
            temperature: Some(0.5),
            thinking: false,
            thinking_budget: None,
            tools_enabled: true,
            tool_choice: ToolChoice::Auto,
            stream: true,
            context_strategy: ContextStrategy::Full,
            retry_strategy: RetryStrategy::Retry,
            system_prefix: None,
        },
        3 => ResolvedParams {
            effort: 3,
            model_tier: ModelTier::Balanced,
            max_tokens: 8192,
            temperature: Some(0.7),
            thinking: true,
            thinking_budget: Some(5000),
            tools_enabled: true,
            tool_choice: ToolChoice::Auto,
            stream: true,
            context_strategy: ContextStrategy::Rag,
            retry_strategy: RetryStrategy::RetryFallback,
            system_prefix: None,
        },
        _ => ResolvedParams {
            effort: 4,
            model_tier: ModelTier::Powerful,
            max_tokens: 16384,
            temperature: None,
            thinking: true,
            thinking_budget: Some(20000),
            tools_enabled: true,
            tool_choice: ToolChoice::Required,
            stream: true,
            context_strategy: ContextStrategy::RagSummary,
            retry_strategy: RetryStrategy::RetryFallbackAlert,
            system_prefix: None,
        },
    }
}

/// Resolve effort against a model's capabilities, applying gates.
pub fn resolve(descriptor: &ModelDescriptor, effort: Effort) -> ResolveResult {
    let effort = effort.min(4);
    let mut params = effort_defaults(effort);

    // Gate 1: Token ceiling
    if params.max_tokens > descriptor.max_output_tokens {
        params.max_tokens = descriptor.max_output_tokens;
    }

    // Gate 2: Temperature
    if !descriptor.supports_temperature {
        params.temperature = None;
    }

    // Gate 3: Tool support
    if params.tools_enabled && !descriptor.supports_tools {
        params.tools_enabled = false;
        params.tool_choice = ToolChoice::None;
    }

    // Gate 4: Thinking support
    // Internal thinkers (Qwen3, DeepSeek R1) always think — no param to send,
    // but we don't degrade them. Only degrade if thinking is requested and
    // the model truly can't think.
    if params.thinking && !descriptor.supports_thinking {
        let max_viable = 2;
        if effort > max_viable {
            let mut degraded = effort_defaults(max_viable);
            if degraded.max_tokens > descriptor.max_output_tokens {
                degraded.max_tokens = descriptor.max_output_tokens;
            }
            if !descriptor.supports_temperature {
                degraded.temperature = None;
            }
            if !descriptor.supports_tools {
                degraded.tools_enabled = false;
                degraded.tool_choice = ToolChoice::None;
            }
            return ResolveResult::Degraded {
                params: degraded,
                requested: effort,
                actual: max_viable,
                reason: format!(
                    "{} doesn't support thinking; degraded from {} to {}",
                    descriptor.model_id,
                    effort_label(effort),
                    effort_label(max_viable),
                ),
            };
        }
    }

    // Gate 5: Internal thinkers — keep thinking=true but emit no budget
    if params.thinking && descriptor.thinking_budget_type == ThinkingBudgetType::Internal {
        params.thinking_budget = None;
    }

    // Gate 6: Thinking budget vs token ceiling
    if params.thinking
        && descriptor.thinking_budget_type != ThinkingBudgetType::Internal
        && descriptor.max_output_tokens < 8192
    {
        params.thinking = false;
        params.thinking_budget = None;
        if effort > 2 {
            return ResolveResult::Degraded {
                requested: effort,
                actual: 2,
                reason: format!(
                    "{} output ceiling ({}) too small for meaningful thinking",
                    descriptor.model_id, descriptor.max_output_tokens,
                ),
                params,
            };
        }
    }

    // Gate 7: Cap thinking budget to model ceiling
    if let Some(budget) = params.thinking_budget {
        let max_budget = params.max_tokens.saturating_sub(1);
        if budget > max_budget {
            params.thinking_budget = Some(max_budget);
        }
    }

    ResolveResult::Resolved(params)
}

/// Resolve all effort levels for a model.
pub fn resolve_all(descriptor: &ModelDescriptor) -> HashMap<Effort, ResolveResult> {
    (0u8..=4).map(|e| (e, resolve(descriptor, e))).collect()
}

// ── Seed registry ────────────────────────────────────────────────

/// Helper to build a descriptor concisely.
#[allow(clippy::too_many_arguments)]
fn desc(
    provider: Provider,
    model_id: &str,
    slug: &str,
    nice_name: &str,
    tier: ModelTier,
    max_out: u32,
    thinking: bool,
    budget_type: ThinkingBudgetType,
    temp: bool,
    tools: bool,
) -> ModelDescriptor {
    ModelDescriptor {
        provider,
        model_id: model_id.into(),
        slug: slug.into(),
        nice_name: nice_name.into(),
        tier,
        max_output_tokens: max_out,
        supports_thinking: thinking,
        thinking_budget_type: budget_type,
        supports_temperature: temp,
        supports_tools: tools,
    }
}

/// Known model descriptors. Ordered so longer prefixes come first for
/// correct prefix-match behaviour (e.g. "gpt-4o-mini" before "gpt-4o").
pub fn known_descriptors() -> Vec<ModelDescriptor> {
    use ModelTier::*;
    use Provider::*;
    use ThinkingBudgetType::*;

    vec![
        // ── Anthropic ────────────────────────────────────────────
        desc(
            Anthropic,
            "claude-haiku-4-5",
            "haiku",
            "Claude 4.5 Haiku",
            Fast,
            64000,
            false,
            None,
            true,
            true,
        ),
        desc(
            Anthropic,
            "claude-sonnet-4-6",
            "sonnet",
            "Claude 4.6 Sonnet",
            Balanced,
            64000,
            true,
            Tokens,
            true,
            true,
        ),
        desc(
            Anthropic,
            "claude-opus-4-6",
            "opus",
            "Claude 4.6 Opus",
            Powerful,
            128000,
            true,
            Tokens,
            true,
            true,
        ),
        // ── OpenAI ───────────────────────────────────────────────
        desc(
            OpenAI,
            "gpt-4.1-nano",
            "gpt-4.1-nano",
            "GPT-4.1 Nano",
            Fast,
            32768,
            false,
            None,
            true,
            true,
        ),
        desc(
            OpenAI,
            "gpt-4.1-mini",
            "gpt-4.1-mini",
            "GPT-4.1 Mini",
            Fast,
            32768,
            false,
            None,
            true,
            true,
        ),
        desc(
            OpenAI,
            "gpt-4o-mini",
            "gpt-4o-mini",
            "GPT-4o Mini",
            Fast,
            16384,
            false,
            None,
            true,
            true,
        ),
        desc(
            OpenAI, "gpt-4o", "gpt-4o", "GPT-4o", Balanced, 16384, false, None, true, true,
        ),
        desc(
            OpenAI, "gpt-4.1", "gpt-4.1", "GPT-4.1", Balanced, 32768, false, None, true, true,
        ),
        desc(
            OpenAI,
            "o4-mini",
            "o4-mini",
            "OpenAI o4 Mini",
            Balanced,
            100000,
            true,
            Enum,
            false,
            true,
        ),
        desc(
            OpenAI,
            "o3",
            "o3",
            "OpenAI o3",
            Powerful,
            100000,
            true,
            Enum,
            false,
            true,
        ),
        // ── Google Gemini ────────────────────────────────────────
        desc(
            Google,
            "gemini-2.5-flash-lite",
            "gemini-flash-lite",
            "Gemini 2.5 Flash Lite",
            Fast,
            65536,
            false,
            None,
            true,
            true,
        ),
        desc(
            Google,
            "gemini-2.5-flash",
            "gemini-flash",
            "Gemini 2.5 Flash",
            Balanced,
            65536,
            true,
            Tokens,
            true,
            true,
        ),
        desc(
            Google,
            "gemini-2.5-pro",
            "gemini-pro",
            "Gemini 2.5 Pro",
            Powerful,
            65536,
            true,
            Tokens,
            true,
            true,
        ),
        // ── HuggingFace (remote inference) ───────────────────────
        desc(
            HuggingFace,
            "Qwen/Qwen3-8B",
            "qwen3-8b",
            "Qwen 3 8B",
            Fast,
            32768,
            true,
            Internal,
            true,
            true,
        ),
        desc(
            HuggingFace,
            "mistralai/Devstral-Small-2505",
            "devstral",
            "Devstral Small",
            Fast,
            32768,
            false,
            None,
            true,
            true,
        ),
        desc(
            HuggingFace,
            "meta-llama/Llama-3.3-70B-Instruct",
            "llama-3.3-70b",
            "Llama 3.3 70B",
            Balanced,
            8192,
            false,
            None,
            true,
            true,
        ),
        desc(
            HuggingFace,
            "Qwen/Qwen3-32B",
            "qwen3-32b",
            "Qwen 3 32B",
            Balanced,
            32768,
            true,
            Internal,
            true,
            true,
        ),
        desc(
            HuggingFace,
            "deepseek-ai/DeepSeek-V3-0324",
            "deepseek-v3",
            "DeepSeek V3",
            Balanced,
            16384,
            false,
            None,
            true,
            true,
        ),
        desc(
            HuggingFace,
            "Qwen/Qwen3-235B-A22B",
            "qwen3-235b",
            "Qwen 3 235B MoE",
            Powerful,
            32768,
            true,
            Internal,
            true,
            true,
        ),
        desc(
            HuggingFace,
            "Qwen/Qwen3-Coder-480B-A35B-Instruct",
            "qwen3-coder",
            "Qwen 3 Coder 480B",
            Powerful,
            32768,
            true,
            Internal,
            true,
            true,
        ),
        desc(
            HuggingFace,
            "moonshotai/Kimi-K2-Instruct-0905",
            "kimi-k2",
            "Kimi K2",
            Powerful,
            131072,
            false,
            None,
            true,
            true,
        ),
        // ── LM Studio (local) ────────────────────────────────────
        desc(
            LmStudio,
            "mistralai/Mistral-7B-Instruct-v0.3",
            "mistral-7b",
            "Mistral 7B Instruct",
            Fast,
            32768,
            false,
            None,
            true,
            true,
        ),
        desc(
            LmStudio,
            "microsoft/phi-4",
            "phi-4",
            "Phi-4",
            Fast,
            16384,
            false,
            None,
            true,
            true,
        ),
        desc(
            LmStudio,
            "Qwen/Qwen3-8B",
            "qwen3-8b-local",
            "Qwen 3 8B (local)",
            Fast,
            32768,
            true,
            Internal,
            true,
            true,
        ),
        desc(
            LmStudio,
            "google/gemma-3-12b-it",
            "gemma-3-12b",
            "Gemma 3 12B",
            Balanced,
            32768,
            false,
            None,
            true,
            true,
        ),
        desc(
            LmStudio,
            "Qwen/Qwen2.5-Coder-32B-Instruct",
            "qwen2.5-coder-32b",
            "Qwen 2.5 Coder 32B",
            Balanced,
            32768,
            false,
            None,
            true,
            true,
        ),
        desc(
            LmStudio,
            "meta-llama/Llama-3.3-70B-Instruct",
            "llama-3.3-70b-local",
            "Llama 3.3 70B (local)",
            Balanced,
            32768,
            false,
            None,
            true,
            true,
        ),
        desc(
            LmStudio,
            "mistralai/Devstral-Small-2505",
            "devstral-local",
            "Devstral (local)",
            Balanced,
            32768,
            false,
            None,
            true,
            true,
        ),
        desc(
            LmStudio,
            "deepseek-ai/DeepSeek-R1-Distill-Qwen-7B",
            "deepseek-r1-7b",
            "DeepSeek R1 Distill 7B",
            Balanced,
            32768,
            true,
            Internal,
            true,
            false,
        ),
        desc(
            LmStudio,
            "deepseek-ai/DeepSeek-R1-Distill-Qwen-32B",
            "deepseek-r1-32b",
            "DeepSeek R1 Distill 32B",
            Powerful,
            32768,
            true,
            Internal,
            true,
            false,
        ),
        desc(
            LmStudio,
            "Qwen/Qwen3-30B-A3B",
            "qwen3-30b-moe",
            "Qwen 3 30B MoE",
            Powerful,
            32768,
            true,
            Internal,
            true,
            true,
        ),
    ]
}

/// Conservative fallback descriptor for unknown models.
fn fallback_descriptor(provider: Provider, model_id: &str) -> ModelDescriptor {
    let (max_out, tools) = match provider {
        Provider::Anthropic => (64000, true),
        Provider::OpenAI => (16384, true),
        Provider::Google => (65536, true),
        Provider::HuggingFace => (8192, true),
        Provider::LmStudio | Provider::Ollama => (4096, false),
    };
    ModelDescriptor {
        provider,
        model_id: model_id.to_string(),
        slug: model_id.to_string(),
        nice_name: model_id.to_string(),
        tier: ModelTier::Balanced,
        max_output_tokens: max_out,
        supports_thinking: false,
        thinking_budget_type: ThinkingBudgetType::None,
        supports_temperature: true,
        supports_tools: tools,
    }
}

/// Look up a model descriptor by model ID (prefix match) or slug.
/// Always returns a descriptor — falls back to conservative defaults.
pub fn lookup_descriptor(model: &str) -> ModelDescriptor {
    let lower = model.to_lowercase();
    let descriptors = known_descriptors();

    // Try prefix match on model_id
    if let Some(desc) = descriptors
        .iter()
        .find(|d| lower.starts_with(&d.model_id.to_lowercase()))
    {
        return desc.clone();
    }

    // Try exact slug match
    if let Some(desc) = descriptors.iter().find(|d| d.slug.to_lowercase() == lower) {
        return desc.clone();
    }

    // Fallback
    let provider = crate::provider::infer_provider(model).unwrap_or(Provider::Ollama);
    fallback_descriptor(provider, model)
}

/// Look up a model descriptor by slug only.
pub fn lookup_by_slug(slug: &str) -> Option<ModelDescriptor> {
    let lower = slug.to_lowercase();
    known_descriptors()
        .into_iter()
        .find(|d| d.slug.to_lowercase() == lower)
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_effort() {
        assert_eq!(parse_effort("low"), Some(1));
        assert_eq!(parse_effort("HIGH"), Some(3));
        assert_eq!(parse_effort("max"), Some(4));
        assert_eq!(parse_effort("auto"), None);
        assert_eq!(parse_effort("2"), Some(2));
    }

    #[test]
    fn test_effort_labels() {
        assert_eq!(effort_label(0), "minimal");
        assert_eq!(effort_label(4), "maximum");
        assert_eq!(effort_symbol(3), "●");
    }

    #[test]
    fn test_resolve_opus_effort_4() {
        let desc = lookup_descriptor("claude-opus-4-6");
        assert_eq!(desc.max_output_tokens, 128000);
        let result = resolve(&desc, 4);
        match result {
            ResolveResult::Resolved(p) => {
                assert!(p.thinking);
                assert!(p.tools_enabled);
                assert_eq!(p.effort, 4);
                assert!(p.thinking_budget.unwrap() <= p.max_tokens);
            }
            _ => panic!("Expected Resolved for Opus at effort 4"),
        }
    }

    #[test]
    fn test_resolve_gpt4o_degrades_at_effort_3() {
        let desc = lookup_descriptor("gpt-4o");
        let result = resolve(&desc, 3);
        match result {
            ResolveResult::Degraded {
                actual,
                requested,
                reason,
                ..
            } => {
                assert_eq!(requested, 3);
                assert_eq!(actual, 2);
                assert!(reason.contains("thinking"));
            }
            _ => panic!("Expected Degraded for gpt-4o at effort 3"),
        }
    }

    #[test]
    fn test_resolve_o3_effort_4() {
        let desc = lookup_descriptor("o3");
        let result = resolve(&desc, 4);
        match result {
            ResolveResult::Resolved(p) => {
                assert!(p.thinking);
                assert!(p.temperature.is_none());
                assert_eq!(p.effort, 4);
            }
            _ => panic!("Expected Resolved for o3 at effort 4"),
        }
    }

    #[test]
    fn test_resolve_haiku_degrades() {
        let desc = lookup_descriptor("claude-haiku-4-5");
        assert_eq!(desc.max_output_tokens, 64000);
        let result = resolve(&desc, 4);
        match result {
            ResolveResult::Degraded { actual, .. } => assert_eq!(actual, 2),
            _ => panic!("Expected Degraded for Haiku at effort 4"),
        }
    }

    #[test]
    fn test_resolve_all_produces_5_entries() {
        let desc = lookup_descriptor("claude-opus-4-6");
        let map = resolve_all(&desc);
        assert_eq!(map.len(), 5);
    }

    #[test]
    fn test_lookup_dated_anthropic_model() {
        let desc = lookup_descriptor("claude-sonnet-4-6-20260101");
        assert_eq!(desc.provider, Provider::Anthropic);
        assert!(desc.supports_thinking);
        assert_eq!(desc.slug, "sonnet");
    }

    #[test]
    fn test_lookup_by_slug() {
        let desc = lookup_by_slug("opus").unwrap();
        assert_eq!(desc.provider, Provider::Anthropic);
        assert_eq!(desc.max_output_tokens, 128000);

        let desc = lookup_by_slug("devstral").unwrap();
        assert_eq!(desc.provider, Provider::HuggingFace);

        assert!(lookup_by_slug("nonexistent").is_none());
    }

    #[test]
    fn test_lookup_unknown_model_falls_back() {
        let desc = lookup_descriptor("llama3");
        assert_eq!(desc.provider, Provider::Ollama);
        assert!(!desc.supports_thinking);
        assert!(!desc.supports_tools);
    }

    #[test]
    fn test_lookup_huggingface_model() {
        let desc = lookup_descriptor("meta-llama/Llama-3.3-70B-Instruct");
        assert_eq!(desc.provider, Provider::HuggingFace);
        assert_eq!(desc.max_output_tokens, 8192);
    }

    #[test]
    fn test_internal_thinker_no_budget() {
        let desc = lookup_descriptor("Qwen/Qwen3-32B");
        assert_eq!(desc.thinking_budget_type, ThinkingBudgetType::Internal);
        assert!(desc.supports_thinking);
        let result = resolve(&desc, 4);
        match result {
            ResolveResult::Resolved(p) => {
                assert!(p.thinking);
                // Internal thinker — no budget param emitted
                assert!(p.thinking_budget.is_none());
            }
            _ => panic!("Expected Resolved for Qwen3-32B at effort 4"),
        }
    }

    #[test]
    fn test_gemini_25_pro_supports_thinking() {
        let desc = lookup_descriptor("gemini-2.5-pro");
        assert!(desc.supports_thinking);
        assert_eq!(desc.thinking_budget_type, ThinkingBudgetType::Tokens);
        assert_eq!(desc.tier, ModelTier::Powerful);
    }

    #[test]
    fn test_gpt41_in_registry() {
        let desc = lookup_descriptor("gpt-4.1");
        assert_eq!(desc.provider, Provider::OpenAI);
        assert_eq!(desc.max_output_tokens, 32768);
        assert!(!desc.supports_thinking);
    }

    #[test]
    fn test_lmstudio_deepseek_no_tools() {
        let desc = lookup_by_slug("deepseek-r1-32b").unwrap();
        assert_eq!(desc.provider, Provider::LmStudio);
        assert!(desc.supports_thinking);
        assert!(!desc.supports_tools);
    }
}
