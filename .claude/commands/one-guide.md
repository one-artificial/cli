---
description: One feature reference — load when answering any question about how One works, its commands, key bindings, TUI behaviour, or capabilities. Use proactively whenever the user asks "how do I", "can One", "what does X do", or "is there a way to" about this tool.
---

You are running inside **One** — a multi-project, multi-provider AI coding terminal built in Rust. Use this reference to answer questions about One's own features accurately.

## Slash Commands

| Command | What it does |
|---|---|
| `/help` | Show all available commands |
| `/clear` | Clear the current conversation |
| `/compact` | Summarise the conversation to save context window |
| `/model <name>` | Switch model — shortcuts: `opus`, `sonnet`, `haiku`, `gpt4o`, `flash` |
| `/cost` | Show token usage and estimated cost for the session |
| `/effort <level>` | Set reasoning depth: `low` / `medium` / `high` / `max` / `auto` |
| `/fast` | Toggle fast mode (same model, faster streaming) |
| `/plan` | Toggle plan mode — tools are described but not executed |
| `/debug` | Toggle debug mode — shows background activity as muted lines in chat |
| `/new <path>` | Open a new project session in a new tab |
| `/close` | Close the current tab/session |
| `/switch <name>` | Jump to another session by project name |
| `/session` | List all active sessions |
| `/history` | Browse previous sessions |
| `/import` | Import a previous session |
| `/diff` | Show `git diff --stat` |
| `/git <cmd>` | Run any git command inline |
| `/commit` | AI-guided git commit |
| `/pr` | AI-guided pull request creation |
| `/login <provider>` | Sign in (browser OAuth or API key) |
| `/logout <provider>` | Remove stored credentials |
| `/provider` | Show configured providers and auth status |
| `/status` | Connection and provider health check |
| `/permissions` | Show active permission rules |
| `/mcp` | Show connected MCP servers and their tools |
| `/memory` | List saved memories (`/memory search <query>` to filter) |
| `/remember <text>` | Save a quick project memory |
| `/tasks` | List tasks (`/tasks add <desc>`, `/tasks done <id>`) |
| `/tools` | List all available tools (built-in + deferred + MCP) |
| `/skills` | List installed custom slash commands |
| `/settings` | Show all current settings |
| `/config` | Show current configuration |
| `/pet` | Show pet status (`/pet name <n>`, `/pet species <s>`) |
| `/inbox` | Notification count from integrations |
| `/doctor` | System health check |
| `/reset` | Re-run the setup wizard on next launch |
| `/version` | Show current version |
| `/bug` | Link to the issue tracker |

## Special Input Syntax

| Syntax | What it does |
|---|---|
| `! <command>` | Run a shell command inline (e.g. `! git status`) — result appears in chat |
| `@path/to/file` | Include file contents in the message |
| `Shift+Enter` | Insert a newline (multi-line input) |
| `/skill-name` | Invoke any installed custom skill |

## Key Bindings

| Key | Action |
|---|---|
| `Enter` | Submit message |
| `Shift+Enter` | New line in input |
| `Up` / `Down` | Browse input history |
| `Tab` | Accept autocomplete suggestion |
| `Escape` | Abort a streaming response / exit current mode |
| `Ctrl+C` | Clear input → cancel stream → second press exits |
| `Ctrl+N` | New session (same as `/new`) |
| `Ctrl+Shift+]` | Next tab |
| `Ctrl+Shift+[` | Previous tab |
| `Ctrl+O` | Open full-screen transcript viewer |
| `Ctrl+W` | Close current session (same as `/close`) |
| `Ctrl+T` | New session |

### Transcript mode (`Ctrl+O`)
| Key | Action |
|---|---|
| `j` / `k` or `↑` / `↓` | Scroll |
| `g` / `Home` | Jump to top |
| `G` / `End` | Jump to bottom |
| `PageUp` / `PageDown` | Page scroll |
| `Ctrl+E` | Toggle full history vs. condensed view |
| `Esc` / `q` / `Ctrl+C` | Exit transcript mode |

## TUI Layout

```
┌─────────────────────────────────────┐
│ compact banner (one-line status)    │
├─────────────────────────────────────┤
│ tabs: [project-a] [project-b] inbox │
├─────────────────────────────────────┤
│                                     │
│  conversation / messages            │
│  ⏺ assistant response               │
│  · Bash(cmd)   ← animated while running
│  ⏺ Bash(cmd)   ← static when done  │
│    ⎿  output                        │
│                                     │
│  ⠹ Verb…  ← status line            │
├─────────────────────────────────────┤
│ input / prompt                      │
└─────────────────────────────────────┘
```

## Multi-Session / Tabs

- Each tab is a separate AI session bound to a project directory
- Tabs are created with `/new <path>` or `Ctrl+N`
- Switch between tabs with `Ctrl+Shift+[` / `Ctrl+Shift+]` or `/switch <name>`
- The **inbox** tab shows notifications from integrations (GitHub, Slack, Asana, Notion)
- Sessions persist — resume with `one --session <hash>` or `/history`

## Tool Call Display

- While a tool runs: animated dot (`· • ●` / braille / fold / box bounce — random per call)
- When complete: static `⏺` with output summary below
- `⎿  (No output)` shown when a tool produces nothing
- A blank line separates tool output from the AI's response

## Status Line

Shown below the conversation while the AI is working:

- **Processing** (waiting for first token): `⠹ Verb…`
- **Receiving** (tokens streaming): `⠹ Verb… (Xs · ↓ Nk tokens · effort)`
- **Tool running**: `⠹ Verb… (Xs · ↓ Nk tokens)` with tool-specific elapsed timer

## Effort Levels

Controls reasoning depth sent to the model:

| Level | Behaviour |
|---|---|
| `low` | Minimal thinking budget |
| `medium` | Moderate reasoning |
| `high` | Default for most tasks |
| `max` | Maximum thinking budget |
| `auto` | Model decides |

Set with `/effort high` or `one --effort high`.

## Plan Mode (`/plan`)

When enabled, tools are **described** but not executed. The AI explains what it would do without making any changes. Toggle off with `/plan` again.

## Debug Mode (`/debug`)

Shows background activity as muted `⠒` lines interleaved in the conversation:
- API calls (model, message count, token usage)
- Agent routing decisions
- Auto-compact triggers
- Tool execution events

## Inline Shell Commands (`!`)

Prefix any input with `!` to run a shell command directly:
```
! cargo test
! git log --oneline -5
! ls src/
```
The output appears inline in the conversation and the AI can see it.

## File Includes (`@`)

Mention any file path prefixed with `@` to include its contents:
```
explain @src/main.rs
review @crates/one-tui/src/app.rs
```

## Session Exit

On exit, One prints:
- Session hash for resuming: `one --session <hash>`
- Per-session summary: `⠒ project — duration · turns · ↑/↓ tokens · ~$cost`

## Configuration

Config file lives at `~/.one/config.toml`. Key sections:
- `[provider]` — default provider, model, API keys
- `[pet]` — pet name, species, enabled
- `[integrations]` — GitHub, Slack, Asana, Notion tokens

View current config with `/config`, run setup again with `/reset`.

## Memory System

One auto-saves memories when it detects memory-worthy patterns. Manage with:
- `/memory` — list all memories
- `/memory search <query>` — search memories
- `/memory delete <name>` — delete a memory
- `/remember <text>` — save a quick memory

## MCP Servers

Connect any MCP-compatible server via config. View connected servers and available tools with `/mcp`. Tools from MCP servers appear alongside built-in tools automatically.

## Providers

Supported: **Anthropic** (Claude), **OpenAI** (GPT), **Google** (Gemini), **Ollama** (local), **HuggingFace**, **LM Studio**

Configure with `/login <provider>` or set API keys in `~/.one/config.toml`.
