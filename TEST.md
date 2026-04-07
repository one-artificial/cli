# One — Testing Guide

## Quick Start

```bash
cargo test                    # Run all tests
cargo test -p one-core       # Test specific crate
cargo test --release         # Run tests in release mode (faster)
cargo tarpaulin --out html   # Coverage report
cargo nextest run            # Use nextest for faster parallel testing
```

## Key Testing Patterns

### Async Testing
```rust
#[tokio::test]
async fn test_async_function() {
    let result = async_function().await;
    assert!(result.is_ok());
}
```

### Event-Driven Testing
```rust
#[tokio::test]
async fn test_events() {
    let (tx, mut rx) = broadcast::channel::<Event>(100);
    tx.send(Event::UserMessage("test".to_string())).unwrap();
    let received = rx.recv().await.unwrap();
    assert_matches!(received, Event::UserMessage(msg) if msg == "test");
}
```

### Trait Object Testing
```rust
#[tokio::test]
async fn test_provider() {
    let provider: Box<dyn AiProvider> = Box::new(MockProvider::new());
    let response = provider.generate_response("test").await;
    assert!(response.is_ok());
}
```

### Concurrent State Testing
```rust
#[tokio::test]
async fn test_shared_state() {
    let state = Arc::new(RwLock::new(AppState::default()));
    // Test concurrent access...
}
```

## Per-Crate Focus
- **one-core**: Traits, events, config, state
- **one-ai**: Provider implementations, mocked APIs
- **one-tools**: Tool execution, parameter validation
- **one-tui**: Rendering, input handling
- **one-db**: Persistence, migrations
- **one-integrations**: External APIs, webhooks

## Integration Testing
```rust
#[tokio::test]
async fn test_end_to_end_workflow() {
    // Setup complete system
    let (event_tx, mut event_rx) = broadcast::channel::<Event>(100);
    let state = Arc::new(RwLock::new(AppState::default()));
    let registry = create_default_registry();
    
    // Simulate user interaction
    event_tx.send(Event::UserMessage("run tests".to_string())).unwrap();
    
    // Verify tool execution
    let event = event_rx.recv().await.unwrap();
    assert_matches!(event, Event::ToolResult(result) if result.success);
}
```

## Mocking External Services
```rust
use wiremock::{MockServer, Mock, ResponseTemplate};

#[tokio::test]
async fn test_github_integration() {
    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(200)
            .set_body_json(&serde_json::json!({"login": "testuser"})))
        .mount(&mock_server)
        .await;
    
    // Test integration with mock server...
}
```

## Environment Setup
```bash
export ONE_TEST_MODE=true
export ONE_LOG_LEVEL=debug
```

## Debugging
```bash
cargo test -- --nocapture --test-threads=1
RUST_LOG=debug cargo test test_name -- --exact
cargo test --features=debug-mode
cargo test -- --show-output  # Show println! output even on success
```

## Performance Testing
```bash
cargo bench                  # Run benchmarks
cargo flamegraph --test test_name  # Profile test execution
hyperfine 'cargo test'       # Benchmark test suite performance
```