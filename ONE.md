# One — AI Coding Terminal

## What this is
A multi-project, multi-provider AI coding terminal built in Rust. Think Claude Code, but open and self-hostable. It's a TUI application that lets you run AI agents with tools (bash, file edit, grep, web search, etc.) against your local codebase.

## Tech stack
- **Rust** (2024 edition), Cargo workspace with 7 crates
- **ratatui** + **crossterm** for the TUI
- **tokio** for async runtime
- **reqwest** for HTTP (AI provider APIs)
- **SQLite** via `one-db` for session persistence
- **keyring** for secure credential storage

## Crate map
| Crate | Role |
|---|---|
| `one-cli` | Binary entrypoint, wires everything together |
| `one-tui` | TUI rendering — app, tabs, input, pet, markdown, theme |
| `one-core` | Types, traits, state, config, query engine, events |
| `one-ai` | AI provider implementations (Anthropic + OpenAI-compat) |
| `one-tools` | Tool implementations registered in `AgentRegistry` |
| `one-integrations` | GitHub, Slack, Asana, Notion |
| `one-db` | SQLite persistence for sessions and memory |

## Key architecture patterns
- **Event-driven**: all subsystems communicate via `broadcast::Sender<Event>` (see `one-core/src/event.rs`)
- **SharedState**: `Arc<RwLock<AppState>>` for concurrent state access
- **ToolExecutor**: closure bridging `one-tools` into `one-core` without circular deps
- **AgentRegistry**: filters tool schemas per agent role to control context window usage
- **Config layering**: keyring → config file → env var → CLI flags
- **Dyn-compatible traits**: `AiProvider` and `Tool` use `Pin<Box<dyn Future>>` for dynamic dispatch

## Adding things
- **New tool**: implement `Tool` trait in `one-tools/src/`, register in `create_default_registry()` in `one-tools/src/lib.rs`
- **New provider**: implement `AiProvider` in `one-ai/src/`, add variant to `Provider` enum in `one-core/src/provider.rs`, wire in `create_provider()` in `one-ai/src/lib.rs`
- **New integration**: implement `Integration` in `one-integrations/src/`, add config in `one-core/src/config.rs`, wire startup in `one-cli/src/main.rs`

## Tests
- 112 tests total, all passing
- Coverage: `one-ai` (tool shim), `one-cli` (integration), `one-core` (unit — cron, effort, compaction, config, MCP, keybindings)
- No tests yet in: `one-db`, `one-integrations`, `one-tools`, `one-tui`

## Dev setup
```bash
git config core.hooksPath .github/hooks  # activate pre-commit hooks (fmt + clippy + check)
cargo build
cargo test
cargo audit
```
