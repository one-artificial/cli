# One

A multi-project, multi-provider AI coding terminal built in Rust.

One is a Claude Code / Codex alternative that manages multiple project sessions simultaneously, integrates with GitHub/Slack/Asana/Notion, routes to specialist agents for efficiency, and lets you choose your AI provider.

## Quick Start

```bash
# Set your API key
export ANTHROPIC_API_KEY="sk-ant-..."
# or
export OPENAI_API_KEY="sk-..."

# Build and run
cargo build --release
./target/release/one
```

## Usage

```bash
# Open on the current directory
one

# Open a specific project
one --project /path/to/project

# Multiple projects (each gets a tab)
one -p /project/one -p /project/two

# Provider is inferred from model name — no --provider needed
one --model gpt-4o 'say hi'
one --model gemini-2.0-flash 'say hi'
one --model meta-llama/Llama-3.1-8B-Instruct 'say hi'

# Or specify explicitly
one --provider anthropic --model claude-sonnet-4-20250514
one --provider lmstudio 'say hi'
```

## Features

### Multi-Project Sessions
Each project gets its own tab with independent conversation history. Switch with `Ctrl+N` / `Ctrl+P` or use `/switch <name>`. Add projects mid-session with `/new <path>`.

### Model Agnostic
Works with 6 providers out of the box:
- **Anthropic** (Claude) — `/login anthropic <key>` or `ANTHROPIC_API_KEY` env var
- **OpenAI** (GPT) — `/login openai <key>` or `OPENAI_API_KEY` env var
- **Google** (Gemini) — `/login google <key>` or `GOOGLE_API_KEY` env var
- **Hugging Face** — `/login huggingface` for OAuth (also unlocks HF Inference API)
- **Ollama** — local models, no auth needed
- **LM Studio** — local models, no auth needed

Check status with `/provider`. The provider is auto-detected from `--model` or available credentials.

### Agent Routing
Automatically classifies user intent and routes to specialist agents (Reader, Writer, Executor, Explorer) with reduced tool sets. This cuts context window usage by 60-80% compared to sending all tools on every call.

### Integrations
Receives notifications from GitHub (PRs, issues), Slack (messages), Asana (tasks), and Notion (page updates). Configure in `~/.one/config.toml`:

```toml
[integrations.github]
token = "ghp_..."
repos = ["owner/repo"]

[integrations.slack]
token = "xoxb-..."
channels = ["C01234567"]
```

Switch to the inbox tab (`Ctrl+P` to tab 0) to see all notifications.

### Pet Companion
A small ASCII pet lives in the status bar, reacts to events, and occasionally comments. Customize with `/pet name Pixel` and `/pet species cat`. Available species: duck, cat, dog, fox, crab.

### Tools
8 built-in tools available to the AI:
- `file_read` — Read files with line numbers
- `file_write` — Create/overwrite files
- `file_edit` — Search-and-replace in files
- `bash` — Execute shell commands
- `grep` — Search file contents (uses ripgrep)
- `glob` — Find files by pattern
- `web_fetch` — Fetch URLs and extract readable text
- `web_search` — Search the web via DuckDuckGo

### Slash Commands
```
/help              Show all commands
/clear             Clear conversation
/compact           Compact conversation to save context
/model <name>      Switch model (opus, sonnet, haiku, gpt-4o, etc.)
/cost              Show token usage and estimated cost
/config            Show current configuration
/version           Show version
/new <path>        Open a new project session
/close             Close the current session
/switch <name>     Switch session by project name
/session           List active sessions
/login <p>         Browser OAuth login (huggingface)
/login <p> <key>   Store API key in OS keychain
/logout <p>        Remove stored key
/provider          Show auth status for all providers
/plan              Toggle plan mode (describe without executing)
/permissions       Show permission settings and rules
/mcp               Show MCP server connections and tools
/memory            List saved memories
/remember <text>   Save a quick project memory
/tasks             List/manage tasks
/pet               Pet status and customization
/inbox             Notification summary
/status            Connection and provider info
/fast              Toggle fast mode
/diff              Show git diff summary
/git <cmd>         Run a git command
/history           Browse previous sessions
/plugin            List installed plugins
/doctor            Check system health
/bug               Report an issue
```

### Persistence
Conversations are saved to SQLite (`~/.one/one.db`) and restored automatically when you open the same project again.

### Configuration
Config lives at `~/.one/config.toml` (auto-created with defaults on first run). Values are overridden by environment variables, which are overridden by CLI flags.

## Architecture

```
one/
├── one-cli          Binary entry point
├── one-tui          Terminal UI (ratatui + crossterm)
├── one-core         Event bus, state, config, agents, plugins
├── one-ai           AI providers (Anthropic, OpenAI)
├── one-tools        Tool implementations
├── one-integrations External service polling (GitHub, Slack, Asana, Notion)
└── one-db           SQLite persistence
```

## License

Apache-2.0
