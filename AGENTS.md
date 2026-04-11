# One — Development Guide

## Setup

After cloning, activate the pre-commit hooks (fmt + clippy + check):

```bash
git config core.hooksPath .github/hooks
```

## Project Overview

One is a multi-project, multi-provider AI coding terminal built in Rust. It's a 7-crate Cargo workspace.

## Build & Test

```bash
cargo build           # Debug build
cargo build --release # Release build
cargo check           # Type-check only (fast)
cargo test            # Run all tests
cargo audit           # Security audit
```

## Architecture

- **Event-driven**: All subsystems communicate through `Event` variants on a tokio broadcast channel
- **Dyn-compatible traits**: `AiProvider` and `Tool` use `Pin<Box<dyn Future>>` returns for dynamic dispatch
- **No circular deps**: `one-core` defines traits, other crates implement them. Tool executor is a closure passed from main.rs
- **Config layering**: keyring -> config file -> env var -> CLI flags

## Crate Dependency Graph

```
one-cli (binary)
├── one-tui    (TUI rendering)
├── one-core   (types, traits, state, config)
├── one-ai     (provider implementations)
├── one-tools  (tool implementations)
├── one-integrations (GitHub, Slack, Asana, Notion)
└── one-db     (SQLite persistence)
```

## Key Patterns

- `SharedState = Arc<RwLock<AppState>>` for concurrent state access
- `broadcast::Sender<Event>` for cross-subsystem communication
- `ToolExecutor` closure bridges one-tools into one-core without circular deps
- `AgentRegistry` filters tool schemas per agent role to reduce context window
- Pet moods are event-driven (on_user_message, on_error, etc.), not timer-driven

## Adding a New Tool

1. Create `crates/one-tools/src/my_tool.rs` implementing the `Tool` trait
2. Register in `create_default_registry()` in `crates/one-tools/src/lib.rs`
3. The tool automatically appears in the AI's system prompt

## Adding a New Provider

1. Create `crates/one-ai/src/my_provider.rs` implementing `AiProvider`
2. Add variant to `Provider` enum in `crates/one-core/src/provider.rs`
3. Wire in `create_provider()` in `crates/one-ai/src/lib.rs`

## Adding a New Integration

1. Create `crates/one-integrations/src/my_service.rs` implementing `Integration`
2. Add config struct in `crates/one-core/src/config.rs`
3. Wire startup in `crates/one-cli/src/main.rs`
