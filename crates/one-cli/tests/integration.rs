use std::sync::Arc;

use one_ai::mock::MockProvider;
use one_core::event::{Event, EventBus};
use one_core::provider::ModelConfig;
use one_core::query_engine::{QueryEngine, ToolExecResult};
use one_core::session::Session;
use one_core::state::new_shared_state;

/// Test the full query pipeline: user message → QueryEngine → mock AI → conversation update
#[tokio::test]
async fn test_full_query_pipeline() {
    let event_bus = EventBus::default();
    let state = new_shared_state();
    let provider: Arc<dyn one_core::provider::AiProvider> =
        Arc::new(MockProvider::new("Hello from mock AI!"));

    let model_config = ModelConfig::default();

    // Create a session
    let session = Session::new("/tmp/test-project".to_string(), model_config.clone());
    let session_id = session.id.clone();

    {
        let mut s = state.write().await;
        s.sessions.insert(session_id.clone(), session);
        s.active_session_id = Some(session_id.clone());
    }

    // Create tool registry
    let tool_registry = Arc::new(one_tools::create_default_registry());
    let tool_schemas = tool_registry.schemas();
    let registry_clone = tool_registry.clone();
    let read_files: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
    let tool_executor: one_core::query_engine::ToolExecutor = Arc::new(
        move |name: String, input: serde_json::Value, working_dir: String| {
            let registry = registry_clone.clone();
            let read_files = read_files.clone();
            Box::pin(async move {
                let ctx = one_tools::ToolContext {
                    working_dir,
                    session_id: String::new(),
                    read_files,
                };
                match registry.get(&name) {
                    Some(tool) => match tool.execute(input, &ctx).await {
                        Ok(result) => ToolExecResult {
                            output: result.output,
                            is_error: result.is_error,
                        },
                        Err(e) => ToolExecResult {
                            output: format!("Tool error: {e}"),
                            is_error: true,
                        },
                    },
                    None => ToolExecResult {
                        output: format!("Unknown tool: {name}"),
                        is_error: true,
                    },
                }
            })
        },
    );

    // Spawn the query engine
    let engine = QueryEngine::new(state.clone(), provider, model_config, event_bus.sender())
        .with_tools(tool_schemas, tool_executor);
    let _handle = engine.spawn();

    // Add user message to conversation
    {
        let mut s = state.write().await;
        let session = s.sessions.get_mut(&session_id).unwrap();
        session.conversation.push_user_message("Hello!".to_string());
    }

    // Send user message event
    let _ = event_bus.sender().send(Event::UserMessage {
        session_id: session_id.clone(),
        content: "Hello!".to_string(),
    });

    // Wait for the response to arrive
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Verify the conversation has the AI response
    let s = state.read().await;
    let session = s.sessions.get(&session_id).unwrap();
    let turns = &session.conversation.turns;

    assert_eq!(turns.len(), 2, "Should have user + assistant turns");
    assert_eq!(turns[0].role, one_core::conversation::TurnRole::User);
    assert_eq!(turns[0].content, "Hello!");
    assert_eq!(turns[1].role, one_core::conversation::TurnRole::Assistant);
    assert!(
        turns[1].content.contains("Hello from mock AI"),
        "Assistant response should contain mock content, got: {}",
        turns[1].content
    );
    assert!(!turns[1].is_streaming, "Response should be finished");
}

/// Test echo mock provider
#[tokio::test]
async fn test_echo_provider() {
    let event_bus = EventBus::default();
    let state = new_shared_state();
    let provider: Arc<dyn one_core::provider::AiProvider> = Arc::new(MockProvider::echo());

    let model_config = ModelConfig::default();

    let session = Session::new("/tmp/echo-test".to_string(), model_config.clone());
    let session_id = session.id.clone();

    {
        let mut s = state.write().await;
        s.sessions.insert(session_id.clone(), session);
        s.active_session_id = Some(session_id.clone());
    }

    let engine = QueryEngine::new(state.clone(), provider, model_config, event_bus.sender());
    let _handle = engine.spawn();

    {
        let mut s = state.write().await;
        let session = s.sessions.get_mut(&session_id).unwrap();
        session
            .conversation
            .push_user_message("Testing echo".to_string());
    }

    let _ = event_bus.sender().send(Event::UserMessage {
        session_id: session_id.clone(),
        content: "Testing echo".to_string(),
    });

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let s = state.read().await;
    let session = s.sessions.get(&session_id).unwrap();
    let turns = &session.conversation.turns;

    assert_eq!(turns.len(), 2);
    assert!(
        turns[1].content.contains("Echo:"),
        "Should echo the user message, got: {}",
        turns[1].content
    );
    assert!(turns[1].content.contains("Testing echo"));
}

/// Test session creation and structure
#[test]
fn test_session_structure() {
    let config = ModelConfig::default();
    let session = Session::new("/home/user/projects/my-app".to_string(), config);

    assert_eq!(session.project_name, "my-app");
    assert_eq!(session.project_path, "/home/user/projects/my-app");
    assert!(session.conversation.turns.is_empty());
    assert_eq!(session.cost_usd, 0.0);
}

/// Test database operations
#[test]
fn test_db_round_trip() {
    let db = one_db::Database::open_in_memory().unwrap();

    // Save a session
    db.save_session(&one_db::SessionRecord {
        id: "test-123".to_string(),
        project_path: "/tmp/test".to_string(),
        project_name: "test".to_string(),
        model_provider: "anthropic".to_string(),
        model_name: "claude-sonnet".to_string(),
        created_at: "2026-04-07T00:00:00Z".to_string(),
        cost_usd: 0.0,
    })
    .unwrap();

    // Save messages
    db.save_message("test-123", "user", "hello", "2026-04-07T00:00:01Z")
        .unwrap();
    db.save_message("test-123", "assistant", "hi there!", "2026-04-07T00:00:02Z")
        .unwrap();

    // Find session
    let found = db.find_session_by_project("/tmp/test").unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().id, "test-123");

    // Load messages
    let messages = db.load_messages("test-123").unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[0].content, "hello");
    assert_eq!(messages[1].role, "assistant");
    assert_eq!(messages[1].content, "hi there!");

    // Count
    let count = db.message_count("test-123").unwrap();
    assert_eq!(count, 2);
}
