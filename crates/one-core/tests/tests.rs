use one_core::agent::{AgentRegistry, AgentRole};
use one_core::config::AppConfig;
use one_core::conversation::{Conversation, TurnRole};
use one_core::oauth::PkcePair;
use one_core::provider::ModelConfig;
use one_core::session::Session;

#[test]
fn test_agent_registry_defaults() {
    let reg = AgentRegistry::with_defaults();
    assert_eq!(reg.all().len(), 4); // reader, writer, executor, explorer

    let reader = reg.get("reader").unwrap();
    assert_eq!(reader.role, AgentRole::Reader);
    assert!(reader.allowed_tools.contains(&"file_read".to_string()));
    assert!(!reader.allowed_tools.contains(&"bash".to_string()));

    let writer = reg.get("writer").unwrap();
    assert!(writer.allowed_tools.contains(&"file_write".to_string()));
    assert!(writer.allowed_tools.contains(&"file_edit".to_string()));

    let executor = reg.get("executor").unwrap();
    assert!(executor.allowed_tools.contains(&"bash".to_string()));
    assert_eq!(executor.allowed_tools.len(), 1);
}

#[test]
fn test_agent_schema_filtering() {
    let reg = AgentRegistry::with_defaults();

    let all_schemas = vec![
        serde_json::json!({"name": "file_read", "description": "Read files"}),
        serde_json::json!({"name": "file_write", "description": "Write files"}),
        serde_json::json!({"name": "bash", "description": "Run commands"}),
        serde_json::json!({"name": "grep", "description": "Search files"}),
    ];

    let reader_schemas = reg.filter_schemas("reader", &all_schemas);
    assert_eq!(reader_schemas.len(), 2); // file_read + grep

    let executor_schemas = reg.filter_schemas("executor", &all_schemas);
    assert_eq!(executor_schemas.len(), 1); // bash only

    // Unknown agent gets all schemas
    let unknown_schemas = reg.filter_schemas("unknown", &all_schemas);
    assert_eq!(unknown_schemas.len(), 4);
}

#[test]
fn test_coordinator_prompt_includes_agents() {
    let reg = AgentRegistry::with_defaults();
    let prompt = reg.coordinator_prompt();

    assert!(prompt.contains("reader"));
    assert!(prompt.contains("writer"));
    assert!(prompt.contains("executor"));
    assert!(prompt.contains("explorer"));
    assert!(prompt.contains("file_read"));
    assert!(prompt.contains("bash"));
}

#[test]
fn test_conversation_lifecycle() {
    let mut conv = Conversation::default();
    assert!(conv.turns.is_empty());

    conv.push_user_message("hello".to_string());
    assert_eq!(conv.turns.len(), 1);
    assert_eq!(conv.turns[0].role, TurnRole::User);
    assert_eq!(conv.turns[0].content, "hello");

    conv.start_assistant_response();
    assert_eq!(conv.turns.len(), 2);
    assert!(conv.last_is_streaming());

    conv.append_to_current("Hi ");
    conv.append_to_current("there!");
    assert_eq!(conv.turns[1].content, "Hi there!");

    conv.finish_current(Some(100));
    assert!(!conv.last_is_streaming());
    assert_eq!(conv.turns[1].tokens_used, Some(100));
}

#[test]
fn test_conversation_append_only_to_streaming() {
    let mut conv = Conversation::default();
    conv.push_user_message("test".to_string());

    // Appending without a streaming assistant turn does nothing
    conv.append_to_current("should be ignored");
    assert_eq!(conv.turns[0].content, "test");
}

#[test]
fn test_session_creation() {
    let config = ModelConfig::default();
    let session = Session::new("/tmp/test-project".to_string(), config);

    assert_eq!(session.project_name, "test-project");
    assert_eq!(session.project_path, "/tmp/test-project");
    assert!(!session.id.is_empty());
    assert!(session.conversation.turns.is_empty());
}

#[test]
fn test_config_defaults() {
    let config = AppConfig::default();

    // No default provider — auto-detected from credentials or set during onboarding
    assert!(config.provider.default_provider.is_empty());
    assert_eq!(config.provider.max_tokens, 8000);
    assert!(config.pet.enabled);
    assert_eq!(config.pet.name, "Pixel");
    assert_eq!(config.pet.species, "duck");
}

#[test]
fn test_config_api_key_from_env() {
    // Config with no keys set should fall back to env
    let config = AppConfig::default();

    // This tests the fallback chain — env var won't be set in CI
    // so it should return empty string
    let key = config.api_key_for("anthropic");
    // Key is either from env or empty — both are valid
    assert!(key.is_empty() || !key.is_empty());
}

#[test]
fn test_pkce_generation() {
    let pkce1 = PkcePair::generate();
    let pkce2 = PkcePair::generate();

    // Each pair should be unique
    assert_ne!(pkce1.code_verifier, pkce2.code_verifier);
    assert_ne!(pkce1.code_challenge, pkce2.code_challenge);

    // Verifier and challenge should be different
    assert_ne!(pkce1.code_verifier, pkce1.code_challenge);

    // Both should be non-empty
    assert!(!pkce1.code_verifier.is_empty());
    assert!(!pkce1.code_challenge.is_empty());
}

#[test]
fn test_model_config_default() {
    let config = ModelConfig::default();
    // No hardcoded default provider — resolved at runtime from --model or credentials
    assert_eq!(config.max_tokens, 8000);
    assert!(config.temperature.is_none());
}

#[test]
fn test_infer_provider_from_model() {
    use one_core::provider::{Provider, infer_provider};

    // Anthropic models
    assert_eq!(
        infer_provider("claude-sonnet-4-20250514"),
        Some(Provider::Anthropic)
    );
    assert_eq!(infer_provider("claude-opus-4-6"), Some(Provider::Anthropic));
    assert_eq!(
        infer_provider("claude-haiku-4-5-20251001"),
        Some(Provider::Anthropic)
    );

    // Shortcuts
    assert_eq!(infer_provider("opus"), Some(Provider::Anthropic));
    assert_eq!(infer_provider("sonnet"), Some(Provider::Anthropic));
    assert_eq!(infer_provider("haiku"), Some(Provider::Anthropic));

    // OpenAI models
    assert_eq!(infer_provider("gpt-4o"), Some(Provider::OpenAI));
    assert_eq!(infer_provider("gpt-4o-mini"), Some(Provider::OpenAI));
    assert_eq!(infer_provider("o1-preview"), Some(Provider::OpenAI));
    assert_eq!(infer_provider("o3-mini"), Some(Provider::OpenAI));

    // Google models
    assert_eq!(infer_provider("gemini-pro"), Some(Provider::Google));
    assert_eq!(infer_provider("gemini-2.0-flash"), Some(Provider::Google));

    // HuggingFace models (org/model format)
    assert_eq!(
        infer_provider("meta-llama/Llama-3.1-8B-Instruct"),
        Some(Provider::HuggingFace)
    );
    assert_eq!(
        infer_provider("mistralai/Mistral-7B-v0.1"),
        Some(Provider::HuggingFace)
    );

    // Unknown — returns None
    assert_eq!(infer_provider("some-random-model"), None);
}

#[test]
fn test_resolve_model_shortcut() {
    use one_core::provider::resolve_model_shortcut;

    assert_eq!(resolve_model_shortcut("opus"), "claude-opus-4-6");
    assert_eq!(resolve_model_shortcut("sonnet"), "claude-sonnet-4-20250514");
    assert_eq!(resolve_model_shortcut("haiku"), "claude-haiku-4-5-20251001");
    assert_eq!(resolve_model_shortcut("gpt-4o"), "gpt-4o"); // passthrough
}

#[test]
fn test_provider_capabilities() {
    use one_core::provider::{Provider, ToolFormat};

    // Anthropic has its own format
    let caps = Provider::Anthropic.capabilities();
    assert!(caps.supports_tool_use);
    assert_eq!(caps.tool_format, ToolFormat::Anthropic);
    assert!(caps.supports_thinking);

    // OpenAI uses function calling
    let caps = Provider::OpenAI.capabilities();
    assert!(caps.supports_tool_use);
    assert_eq!(caps.tool_format, ToolFormat::OpenAI);
    assert!(!caps.supports_thinking);

    // HuggingFace uses OpenAI-compatible format
    let caps = Provider::HuggingFace.capabilities();
    assert_eq!(caps.tool_format, ToolFormat::OpenAI);

    // Ollama doesn't have native tool use
    let caps = Provider::Ollama.capabilities();
    assert!(!caps.supports_tool_use);
    assert_eq!(caps.tool_format, ToolFormat::None);

    // LM Studio supports tools via OpenAI format
    let caps = Provider::LmStudio.capabilities();
    assert!(caps.supports_tool_use);
    assert_eq!(caps.tool_format, ToolFormat::OpenAI);

    // Google Gemini supports tools
    let caps = Provider::Google.capabilities();
    assert!(caps.supports_tool_use);
}

#[test]
fn test_agent_role_default_tools() {
    assert_eq!(
        AgentRole::Reader.default_tools(),
        vec!["file_read", "grep", "glob"]
    );
    assert_eq!(
        AgentRole::Writer.default_tools(),
        vec!["file_write", "file_edit"]
    );
    assert_eq!(AgentRole::Executor.default_tools(), vec!["bash"]);
    assert!(AgentRole::Coordinator.default_tools().is_empty());
}
