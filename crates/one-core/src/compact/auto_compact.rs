/// Auto-compaction system.
///
/// Monitors conversation token usage and triggers compaction when
/// approaching the context window limit.
use std::sync::Arc;

use crate::provider::{AiProvider, Message, ModelConfig, Role};

use super::prompt;

// --- Constants ---

/// Reserve tokens for output during compaction.
/// Based on p99.99 of compact summary output being 17,387 tokens.
const MAX_OUTPUT_TOKENS_FOR_SUMMARY: u32 = 20_000;

/// Buffer tokens subtracted from context window for autocompact threshold
pub const AUTOCOMPACT_BUFFER_TOKENS: u32 = 13_000;

/// Warning threshold buffer
pub const WARNING_THRESHOLD_BUFFER_TOKENS: u32 = 20_000;

/// Manual compact buffer (blocking limit)
pub const MANUAL_COMPACT_BUFFER_TOKENS: u32 = 3_000;

/// Stop trying autocompact after this many consecutive failures.
const MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES: u32 = 3;

// --- Types ---

/// Tracks autocompact state across turns.
#[derive(Debug, Clone)]
pub struct AutoCompactTracking {
    pub compacted: bool,
    pub turn_counter: u32,
    pub turn_id: String,
    pub consecutive_failures: u32,
}

impl Default for AutoCompactTracking {
    fn default() -> Self {
        Self {
            compacted: false,
            turn_counter: 0,
            turn_id: uuid::Uuid::new_v4().to_string(),
            consecutive_failures: 0,
        }
    }
}

/// Result of a compaction operation
#[derive(Debug)]
pub struct CompactionResult {
    /// The summary message to replace old conversation
    pub summary_message: String,
    /// Messages to keep after the summary (recent tail)
    pub messages_to_keep: Vec<Message>,
    /// Token count before compaction
    pub pre_compact_tokens: u32,
    /// Token count after compaction
    pub post_compact_tokens: u32,
}

/// Token warning state.
#[derive(Debug)]
pub struct TokenWarningState {
    pub percent_left: u32,
    pub is_above_warning_threshold: bool,
    pub is_above_auto_compact_threshold: bool,
    pub is_at_blocking_limit: bool,
}

// --- Functions ---

/// Get the context window size for a model
pub fn get_context_window_for_model(model: &str) -> u32 {
    if model.contains("[1m]") || model.contains("opus-4-6") {
        1_000_000
    } else {
        200_000
    }
}

/// Returns the context window size minus reserved output tokens
/// Get the effective context window size for a model.
pub fn get_effective_context_window_size(model: &str, max_output_tokens: u32) -> u32 {
    let reserved = max_output_tokens.min(MAX_OUTPUT_TOKENS_FOR_SUMMARY);
    let context_window = get_context_window_for_model(model);

    // Allow override via env var
    let window = if let Ok(val) = std::env::var("CLAUDE_CODE_AUTO_COMPACT_WINDOW") {
        val.parse::<u32>()
            .ok()
            .filter(|&v| v > 0)
            .map(|v| context_window.min(v))
            .unwrap_or(context_window)
    } else {
        context_window
    };

    window.saturating_sub(reserved)
}

/// Get the token threshold that triggers auto-compaction
/// Calculate the auto-compact threshold for a model.
pub fn get_auto_compact_threshold(model: &str, max_output_tokens: u32) -> u32 {
    let effective = get_effective_context_window_size(model, max_output_tokens);
    let threshold = effective.saturating_sub(AUTOCOMPACT_BUFFER_TOKENS);

    // Allow percentage override for testing
    if let Ok(pct) = std::env::var("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE")
        && let Ok(parsed) = pct.parse::<f64>()
        && parsed > 0.0
        && parsed <= 100.0
    {
        let pct_threshold = (effective as f64 * parsed / 100.0) as u32;
        return pct_threshold.min(threshold);
    }

    threshold
}

/// Calculate token warning state
/// Calculate token warning state based on usage.
pub fn calculate_token_warning_state(
    token_usage: u32,
    model: &str,
    max_output_tokens: u32,
) -> TokenWarningState {
    let threshold = get_auto_compact_threshold(model, max_output_tokens);

    let percent_left = if threshold > 0 {
        ((threshold.saturating_sub(token_usage)) as f64 / threshold as f64 * 100.0)
            .max(0.0)
            .round() as u32
    } else {
        0
    };

    let warning_threshold = threshold.saturating_sub(WARNING_THRESHOLD_BUFFER_TOKENS);
    let effective = get_effective_context_window_size(model, max_output_tokens);
    let blocking_limit = effective.saturating_sub(MANUAL_COMPACT_BUFFER_TOKENS);

    TokenWarningState {
        percent_left,
        is_above_warning_threshold: token_usage >= warning_threshold,
        is_above_auto_compact_threshold: token_usage >= threshold,
        is_at_blocking_limit: token_usage >= blocking_limit,
    }
}

/// Estimate token count from messages (rough: ~4 chars per token)
pub fn estimate_token_count(messages: &[Message]) -> u32 {
    let total_chars: usize = messages.iter().map(|m| m.content.len()).sum();
    (total_chars / 4) as u32
}

/// Check if auto-compaction is enabled
pub fn is_auto_compact_enabled() -> bool {
    if std::env::var("DISABLE_COMPACT")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false)
    {
        return false;
    }
    if std::env::var("DISABLE_AUTO_COMPACT")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false)
    {
        return false;
    }
    true
}

/// Check if messages should be auto-compacted
/// Determine if auto-compaction should trigger.
pub fn should_auto_compact(messages: &[Message], model: &str, max_output_tokens: u32) -> bool {
    if !is_auto_compact_enabled() {
        return false;
    }

    let token_count = estimate_token_count(messages);
    let state = calculate_token_warning_state(token_count, model, max_output_tokens);

    tracing::debug!(
        "autocompact: tokens={} threshold={} effective_window={}",
        token_count,
        get_auto_compact_threshold(model, max_output_tokens),
        get_effective_context_window_size(model, max_output_tokens),
    );

    state.is_above_auto_compact_threshold
}

/// Perform auto-compaction by calling the model to summarize the conversation
/// Run the core summarization call to compact a conversation.
pub async fn compact_conversation(
    messages: &[Message],
    provider: &Arc<dyn AiProvider>,
    config: &ModelConfig,
) -> anyhow::Result<CompactionResult> {
    let pre_compact_tokens = estimate_token_count(messages);

    // Build the compaction request: system prompt + all conversation + compact instruction
    let compact_prompt = prompt::get_compact_prompt(None);

    let mut compact_messages = Vec::new();

    // Include conversation history as context
    for msg in messages {
        compact_messages.push(msg.clone());
    }

    // Add the compaction instruction as the final user message
    compact_messages.push(Message {
        role: Role::User,
        content: compact_prompt,
    });

    // Call the model to generate the summary
    let response = provider.send_message(&compact_messages, config).await?;

    let _formatted = prompt::format_compact_summary(&response.content);
    let summary_user_msg = prompt::get_compact_user_summary_message(
        &response.content,
        true, // suppress follow-up questions for auto-compact
        None,
    );

    let post_compact_tokens = (summary_user_msg.len() / 4) as u32;

    Ok(CompactionResult {
        summary_message: summary_user_msg,
        messages_to_keep: Vec::new(),
        pre_compact_tokens,
        post_compact_tokens,
    })
}

/// Auto-compact if needed, with circuit breaker
/// Auto-compact the conversation if token usage exceeds the threshold.
pub async fn auto_compact_if_needed(
    messages: &[Message],
    provider: &Arc<dyn AiProvider>,
    config: &ModelConfig,
    tracking: &mut AutoCompactTracking,
) -> Option<CompactionResult> {
    // Circuit breaker: stop after N consecutive failures
    if tracking.consecutive_failures >= MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES {
        return None;
    }

    if !should_auto_compact(messages, &config.model, config.max_tokens) {
        return None;
    }

    match compact_conversation(messages, provider, config).await {
        Ok(result) => {
            tracking.compacted = true;
            tracking.consecutive_failures = 0;
            Some(result)
        }
        Err(e) => {
            tracking.consecutive_failures += 1;
            tracing::warn!(
                "autocompact failed (attempt {}): {e}",
                tracking.consecutive_failures,
            );
            if tracking.consecutive_failures >= MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES {
                tracing::warn!(
                    "autocompact: circuit breaker tripped after {} consecutive failures",
                    tracking.consecutive_failures,
                );
            }
            None
        }
    }
}
