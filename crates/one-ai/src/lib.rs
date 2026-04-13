pub mod anthropic;
pub mod mock;
pub mod providers;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use one_core::provider::{AiProvider, AiResponse, Message, ModelConfig, Provider};
use providers::{OpenAiCompatConfig, OpenAiCompatProvider};
use tokio::sync::Semaphore;

// ── Per-provider API lanes ────────────────────────────────────────────────────
//
// Each provider has its own Semaphore — its own "passing lane".  Anthropic
// calls queue against the Anthropic semaphore; OpenAI calls queue against
// the OpenAI semaphore.  A backlog in one lane never blocks another.
//
// When N parallel sub-agents all target the same provider, they queue in
// that lane instead of all firing at once and triggering a wave of 429s.
// Each sub-agent inherits the same Arc<GatedProvider>, so they all share
// the same lane semaphore automatically.
//
// Capacity defaults: 4 per lane.  Overrides via env:
//   ONE_LANE_ANTHROPIC=2   (per-provider)
//   ONE_LANE_CAPACITY=3    (global default)
//
// ── Dependency ordering (TODO) ────────────────────────────────────────────────
// The next layer above the lane gate is dependency tracking: a request can
// declare `depends_on: Option<RequestId>` and the queue will not enter the
// lane until that upstream request completes.  Dependencies may cross provider
// lanes (B depends on A even though B is OpenAI and A is Anthropic); the lane
// gate is acquired *after* the dependency wait, so unrelated requests in the
// same lane proceed without stalling.  Cyclic dependencies are busted by
// timestamps (the older request runs first).
// Implementation lives in one-core/src/api_queue.rs (not yet built).

type LaneRegistry = Arc<Mutex<HashMap<String, Arc<Semaphore>>>>;

static LANES: OnceLock<LaneRegistry> = OnceLock::new();

fn lane_capacity(provider_name: &str) -> usize {
    let env_key = format!("ONE_LANE_{}", provider_name.to_uppercase());
    std::env::var(env_key)
        .ok()
        .or_else(|| std::env::var("ONE_LANE_CAPACITY").ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(4)
}

fn lane_for(provider_name: &str) -> Arc<Semaphore> {
    let registry = LANES.get_or_init(|| Arc::new(Mutex::new(HashMap::new())));
    let mut map = registry.lock().unwrap();
    map.entry(provider_name.to_string())
        .or_insert_with(|| Arc::new(Semaphore::new(lane_capacity(provider_name))))
        .clone()
}

// ── Retry helpers ────────────────────────────────────────────────────────────

const MAX_RETRIES: u32 = 4;

/// True for HTTP 429 / 502 / 503 and common rate-limit phrases.
/// These are errors the provider will likely recover from if we back off.
fn is_retryable(e: &anyhow::Error) -> bool {
    let msg = e.to_string().to_lowercase();
    msg.contains("429")
        || msg.contains("503")
        || msg.contains("502")
        || msg.contains("rate limit")
        || msg.contains("too many requests")
        || msg.contains("overloaded")
        || msg.contains("capacity")
}

/// Exponential backoff with ±20 % jitter: 1 s, 2 s, 4 s, 8 s.
fn backoff_delay(attempt: u32) -> Duration {
    let base_ms: u64 = 1000 * (1 << attempt.min(6));
    // ±20 % jitter using the attempt as a deterministic seed substitute.
    // True randomness isn't worth the dependency here.
    let jitter_ms = base_ms / 5 * (attempt as u64 % 3); // 0 %, 20 %, 40 % of base
    Duration::from_millis(base_ms + jitter_ms)
}

/// Wraps an `AiProvider` so every call acquires a permit from the provider's
/// lane before hitting the API.
///
/// The lane is looked up by `provider_name()`, so all instances that share the
/// same name share the same semaphore regardless of how many Arc clones exist.
struct GatedProvider {
    inner: Arc<dyn AiProvider>,
    lane: Arc<Semaphore>,
}

impl AiProvider for GatedProvider {
    fn send_message(
        &self,
        messages: &[Message],
        config: &ModelConfig,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<AiResponse>> + Send + '_>> {
        let inner = self.inner.clone();
        let lane = self.lane.clone();
        let messages = messages.to_vec();
        let config = config.clone();
        Box::pin(async move {
            let mut attempt = 0u32;
            loop {
                let permit = lane
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|e| anyhow::anyhow!("API lane closed: {e}"))?;
                match inner.send_message(&messages, &config).await {
                    Ok(r) => return Ok(r),
                    Err(e) if is_retryable(&e) && attempt < MAX_RETRIES => {
                        drop(permit); // release lane before sleeping
                        tracing::warn!(
                            "retryable API error (attempt {}/{}): {e}",
                            attempt + 1,
                            MAX_RETRIES
                        );
                        tokio::time::sleep(backoff_delay(attempt)).await;
                        attempt += 1;
                    }
                    Err(e) => return Err(e),
                }
            }
        })
    }

    fn stream_message(
        &self,
        messages: &[Message],
        config: &ModelConfig,
        on_chunk: Box<dyn Fn(String) + Send + Sync>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<AiResponse>> + Send + '_>> {
        let inner = self.inner.clone();
        let lane = self.lane.clone();
        let messages = messages.to_vec();
        let config = config.clone();
        // Arc so the closure can be shared across retry attempts without moving.
        let on_chunk: Arc<dyn Fn(String) + Send + Sync> = Arc::from(on_chunk);
        Box::pin(async move {
            let mut attempt = 0u32;
            loop {
                // Track whether any chunks reached the caller.  Once the TUI has
                // received partial output we cannot safely retry — the conversation
                // state already has that content.
                let chunks_started = Arc::new(AtomicBool::new(false));
                let flag = chunks_started.clone();
                let cb = on_chunk.clone();
                let wrapped: Box<dyn Fn(String) + Send + Sync> = Box::new(move |s: String| {
                    flag.store(true, Ordering::Relaxed);
                    cb(s);
                });

                let permit = lane
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|e| anyhow::anyhow!("API lane closed: {e}"))?;
                match inner.stream_message(&messages, &config, wrapped).await {
                    Ok(r) => return Ok(r),
                    Err(e)
                        if is_retryable(&e)
                            && attempt < MAX_RETRIES
                            && !chunks_started.load(Ordering::Relaxed) =>
                    {
                        drop(permit); // release lane before sleeping
                        tracing::warn!(
                            "retryable API error (attempt {}/{}): {e}",
                            attempt + 1,
                            MAX_RETRIES
                        );
                        tokio::time::sleep(backoff_delay(attempt)).await;
                        attempt += 1;
                    }
                    Err(e) => return Err(e),
                }
            }
        })
    }

    fn is_configured(&self) -> bool {
        self.inner.is_configured()
    }

    fn provider_name(&self) -> &str {
        self.inner.provider_name()
    }
}

/// Wrap `inner` in a single gate layer keyed by `inner.provider_name()`.
fn gate(inner: Arc<dyn AiProvider>) -> Arc<dyn AiProvider> {
    let lane = lane_for(inner.provider_name());
    Arc::new(GatedProvider { inner, lane })
}

// ── Raw (un-gated) builders — private ────────────────────────────────────────
// Public factories gate *once* at the outermost layer. Raw builders produce
// un-gated providers so wrappers like ToolShimProvider don't accidentally
// acquire two permits for one call.

fn raw_provider(provider: &Provider, api_key: String) -> Arc<dyn AiProvider> {
    match provider {
        Provider::Anthropic => Arc::new(anthropic::AnthropicProvider::new(api_key)),
        Provider::OpenAI => Arc::new(create_openai_compat(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o",
            api_key,
        )),
        Provider::Ollama => Arc::new(create_openai_compat(
            "ollama",
            &std::env::var("OLLAMA_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:11434/v1".to_string()),
            "llama3",
            String::new(),
        )),
        Provider::Google => Arc::new(create_openai_compat(
            "google",
            "https://generativelanguage.googleapis.com/v1beta/openai",
            "gemini-2.0-flash",
            api_key,
        )),
        Provider::HuggingFace => Arc::new(create_openai_compat(
            "huggingface",
            "https://router.huggingface.co/v1",
            "meta-llama/Llama-3.1-8B-Instruct",
            api_key,
        )),
        Provider::LmStudio => Arc::new(create_openai_compat(
            "lmstudio",
            &std::env::var("LMSTUDIO_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:1234/v1".to_string()),
            "default",
            String::new(),
        )),
    }
}

fn raw_anthropic_with_tools(
    api_key: String,
    tool_schemas: Vec<serde_json::Value>,
) -> Arc<dyn AiProvider> {
    Arc::new(anthropic::AnthropicProvider::new(api_key).with_tools(tool_schemas))
}

// ── Public factory functions ──────────────────────────────────────────────────

/// Creates the appropriate AI provider client based on config.
pub fn create_provider(provider: Provider, api_key: String) -> Arc<dyn AiProvider> {
    gate(raw_provider(&provider, api_key))
}

/// Create an Anthropic provider with tool schemas included in every request.
pub fn create_anthropic_with_tools(
    api_key: String,
    tool_schemas: Vec<serde_json::Value>,
) -> Arc<dyn AiProvider> {
    gate(raw_anthropic_with_tools(api_key, tool_schemas))
}

/// Create a provider with tools.  Single gate at this layer regardless of
/// whether the inner implementation uses a shim.
pub fn create_provider_with_tools(
    provider: Provider,
    api_key: String,
    tool_schemas: Vec<serde_json::Value>,
) -> Arc<dyn AiProvider> {
    let inner: Arc<dyn AiProvider> = match provider {
        Provider::Anthropic => raw_anthropic_with_tools(api_key, tool_schemas),
        Provider::OpenAI => Arc::new(
            create_openai_compat("openai", "https://api.openai.com/v1", "gpt-4o", api_key)
                .with_tools(tool_schemas),
        ),
        Provider::HuggingFace => Arc::new(
            create_openai_compat(
                "huggingface",
                "https://router.huggingface.co/v1",
                "meta-llama/Llama-3.1-8B-Instruct",
                api_key,
            )
            .with_tools(tool_schemas),
        ),
        Provider::LmStudio => Arc::new(
            create_openai_compat(
                "lmstudio",
                &std::env::var("LMSTUDIO_BASE_URL")
                    .unwrap_or_else(|_| "http://localhost:1234/v1".to_string()),
                "default",
                String::new(),
            )
            .with_tools(tool_schemas),
        ),
        Provider::Google => Arc::new(
            create_openai_compat(
                "google",
                "https://generativelanguage.googleapis.com/v1beta/openai",
                "gemini-2.0-flash",
                api_key,
            )
            .with_tools(tool_schemas),
        ),
        // Ollama uses a prompt-based shim around a raw (un-gated) base so the
        // single outer gate below is the only permit acquisition.
        Provider::Ollama => Arc::new(providers::ToolShimProvider::new(
            raw_provider(&Provider::Ollama, String::new()),
            tool_schemas,
        )),
    };
    gate(inner)
}

/// Helper to create an OpenAI-compatible provider.
fn create_openai_compat(
    name: &str,
    base_url: &str,
    default_model: &str,
    api_key: String,
) -> OpenAiCompatProvider {
    OpenAiCompatProvider::new(
        OpenAiCompatConfig {
            base_url: base_url.to_string(),
            name: name.to_string(),
            default_model: default_model.to_string(),
            send_tools: true,
        },
        api_key,
    )
}
