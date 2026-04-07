pub mod anthropic;
pub mod mock;
pub mod providers;

use std::sync::Arc;

use one_core::provider::{AiProvider, Provider};
use providers::{OpenAiCompatConfig, OpenAiCompatProvider};

/// Creates the appropriate AI provider client based on config.
/// For Ollama, the api_key is ignored (no auth needed for local models).
pub fn create_provider(provider: Provider, api_key: String) -> Arc<dyn AiProvider> {
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
            String::new(), // Ollama doesn't need auth
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
            String::new(), // LM Studio doesn't need auth
        )),
    }
}

/// Create an Anthropic provider with tool schemas included in every request.
/// Anthropic has its own unique format — NOT OpenAI-compatible.
pub fn create_anthropic_with_tools(
    api_key: String,
    tool_schemas: Vec<serde_json::Value>,
) -> Arc<dyn AiProvider> {
    Arc::new(anthropic::AnthropicProvider::new(api_key).with_tools(tool_schemas))
}

/// Create a provider with tools using the OpenAI-compatible format.
/// Works for OpenAI, HuggingFace, LM Studio, and any OpenAI-compat endpoint.
pub fn create_provider_with_tools(
    provider: Provider,
    api_key: String,
    tool_schemas: Vec<serde_json::Value>,
) -> Arc<dyn AiProvider> {
    match provider {
        Provider::Anthropic => create_anthropic_with_tools(api_key, tool_schemas),
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
        // Ollama doesn't support native tool use — use prompt-based shim
        Provider::Ollama => {
            let base = create_provider(provider, api_key);
            Arc::new(providers::ToolShimProvider::new(base, tool_schemas))
        }
    }
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
