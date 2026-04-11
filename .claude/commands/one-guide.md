---
description: One self-reference — load when answering any question about how One works, its commands, key bindings, TUI behaviour, internal architecture, Evergreen, effort/reasoning, context management, agent routing, or wiring between systems. Use proactively on any "how do I", "can One", "what does X do", "is there a way to", or "how does One handle" question.
---

You are running inside **One** — a multi-project, multi-provider AI coding terminal built in Rust. Use this reference to answer questions about One's own features and internals accurately.

---

## Commands, Tools, and Skills

Three distinct concepts — understanding the difference matters when answering "how do I do X":

| Concept | Who runs it | How | Examples |
|---|---|---|---|
| **Commands** | The **user** | Typed with `/` prefix in the input box | `/debug`, `/compact`, `/model opus`, `/plan` |
| **Tools** | The **model** | Autonomously called during a response turn | `Bash`, `Read`, `Edit`, `Glob`, `Grep`, `Agent` |
| **Skills** | The **model** (or user via `/skill-name`) | Loaded on demand — injects a prompt into the conversation as extra context | `/one-guide`, `/commit`, `/pr`, any `.md` in `commands/` |

**How they interact:**
- A command (`/compact`) triggers TUI logic that may update state and display a result
- A tool (`Bash`) is called by the model, permission-checked, executed via `ToolExecutor`, and results are fed back into the next message turn
- A skill (`/commit`) loads a markdown prompt template which the model follows as instructions; the model can then call whatever tools the skill's `allowed-tools` frontmatter permits
- Skills can be invoked by the model autonomously via the `Skill` tool when it recognises a relevant context — the system prompt lists all available skills and their descriptions

**Design principle: open standards outrank platform standards.** Within each tier, platform-specific tools are loaded first (lowest priority) and One-native / open-standard paths are loaded last (highest priority). Conflicts always resolve in favour of the more-open format.

**Skill loading levels** (lowest → highest priority, higher wins on name conflict):

| Level | Order within level (last = wins) |
|---|---|
| Profile | `~/.gemini/commands/` → `~/.claude/commands/` → `~/.one/commands/` |
| Git root | `<root>/.gemini/commands/` → `<root>/.claude/commands/` → `<root>/.one/commands/` |
| Project | `.gemini/commands/` → `.claude/commands/` → `.one/commands/` |

Git root is detected by walking up from the project directory until a `.git` entry is found — enabling monorepo-level skills shared across all packages. A project-level skill with the same name as a profile-level one always wins.

**Skill file format:**

```markdown
---
description: Shown in autocomplete and system prompt — controls when model auto-invokes
allowed-tools: Bash(git add:*), Bash(git commit:*)   # optional
argument-hint: <branch>                               # optional
---

Prompt body. $ARGUMENTS is replaced with text after the command name.
!`git branch --show-current`  — inline shell commands replaced at invocation time.
```

---

## Design Principles

**Open standards outrank platform standards.** When loading config, instructions, skills, or MCP servers from multiple sources, more-open formats always win over vendor-specific ones:

- `AGENTS.md` > `CLAUDE.md` / `GEMINI.md` / `.cursorrules`
- `.mcp.json` (MCP spec) > `claude_desktop_config.json`
- `.one/commands/` > `.claude/commands/` > `.gemini/commands/`

This means a developer's `AGENTS.md` instruction always takes precedence over any platform-specific instruction file.

---

## Architecture Overview

```
one-cli (binary)
├── one-tui    — ratatui TUI: rendering, input, autocomplete, commands
├── one-core   — types, traits, state, config, query engine, effort, evergreen, compact
├── one-ai     — provider implementations (Anthropic, OpenAI, Google, Ollama, HF, LM Studio)
├── one-tools  — tool implementations (Bash, Read, Edit, Glob, Grep, Agent, …)
├── one-integrations — GitHub, Slack, Asana, Notion
└── one-db     — SQLite persistence (session records, evergreen storage)
```

**Core patterns:**
- `SharedState = Arc<RwLock<AppState>>` — all subsystems share one state object
- `broadcast::Sender<Event>` — event bus wires AI, tools, TUI, and integrations without circular dependencies
- `ToolExecutor` closure — bridges `one-tools` into `one-core` without a circular dep
- `AgentRegistry` — filters tool schemas per agent role to narrow context window
- No trait objects for state — everything is concrete types behind shared Arc

---

## Event Bus

All cross-subsystem communication goes through typed events on a tokio broadcast channel. Key events:

| Event | Direction | Purpose |
|---|---|---|
| `UserMessage { session_id, content }` | TUI → QueryEngine | User submitted a message |
| `AiResponseChunk { session_id, content, done }` | QueryEngine → TUI | Text delta or stream end |
| `ToolRequest { session_id, tool_name, input, call_id }` | QueryEngine → TUI | Tool call announced |
| `ToolResult { session_id, call_id, output, is_error }` | QueryEngine → TUI | Tool result ready |
| `ToolDenied { session_id, call_id, reason, warning }` | QueryEngine → TUI | Permission denied |
| `DebugLog { session_id, message }` | QueryEngine → TUI | Internal diagnostic (shown in debug mode) |
| `EvergreenCompressed { session_id, turns_compressed }` | Evergreen → TUI | Background compression completed |
| `PermissionPrompt { … }` | QueryEngine → TUI | Waiting for user permission |
| `UserQuestion { … }` | QueryEngine → TUI | AI asking user a question (AskUserQuestion tool) |
| `SessionCreated / SessionClosed / SessionSwitched` | TUI internal | Tab lifecycle |
| `Quit` | Any → all | Shutdown signal |

The QueryEngine subscribes to `UserMessage` and all tool result events. The TUI subscribes to everything else. `DebugLog` events are accumulated in `session.debug_events` and shown as muted `⠒` lines when `/debug` is on.

---

## Query Engine Loop

The core request loop in `one-core/src/query_engine.rs`:

1. **Intent classification** — `classify_intent(content)` keyword-matches the user message to pick an `AgentRole`. Unambiguous messages get a narrowed tool set; ambiguous ones get all tools.

2. **Message building** — Constructs `Vec<Message>` from: system prompt (with agent role suffix) + conversation turns (User/Assistant only; ToolResult turns are serialised differently).

3. **Auto-compact check** — Before each API call, `should_auto_compact()` estimates token count. If above threshold, `auto_compact_if_needed()` replaces the message list with an AI-generated summary + recent tail (see Compact section).

4. **Stream signal** — An empty `AiResponseChunk { done: false }` is sent to the TUI *before* the API call. This activates the spinning verb status immediately, so the user sees thinking is happening even before any text arrives.

5. **API call** — `provider.stream_message(messages, config, on_chunk)`. The `on_chunk` callback appends text to the current assistant turn and emits `AiResponseChunk` for each delta. **Thinking/reasoning tokens are consumed silently in the provider layer** — only text reaches `on_chunk`.

6. **Tool call processing** — For each tool call in the response, the engine intercepts special tools before execution:
   - `Agent` → sub-agent fork with optional worktree isolation and background spawn
   - `enter_plan_mode` / `exit_plan_mode` → toggle plan mode on AppState
   - `list_mcp_resources` / `read_mcp_resource` → MCP resource access
   - `cron_create` / `cron_delete` / `cron_list` → cron scheduler
   - `Skill` → load skill file and inject prompt into next assistant turn
   - All others → permission check → `tool_executor` closure → `ToolResult` event

7. **Loop** — Tool results are appended to messages as `tool_result` blocks and the whole loop repeats (max 200 turns). No tool calls → stream done event and return.

---

## Effort & Reasoning System

`one-core/src/effort.rs` — five effort levels (0–4) translate to concrete model parameters:

| Effort | Label | Max Tokens | Temp | Thinking | Budget | Tools | Context |
|--------|-------|-----------|------|----------|--------|-------|---------|
| 0 | minimal | 256 | 0.0 | ✗ | — | ✗ | LastTurn |
| 1 | low | 1024 | 0.2 | ✗ | — | ✗ | Recent |
| 2 | medium | 4096 | 0.5 | ✗ | — | ✓ | Full |
| 3 | high | 8192 | 0.7 | ✓ | 5,000 | ✓ | Rag |
| 4 | max | 16384 | — | ✓ | 20,000 | ✓ | RagSummary |

**How thinking budget is expressed per provider:**

| Provider type | ThinkingBudgetType | Wire format |
|---|---|---|
| Anthropic Claude | `Tokens` | `{ "type": "enabled", "budget_tokens": N }` |
| OpenAI o-series | `Enum` | `reasoning_effort: "low"/"medium"/"high"` |
| Gemini 2.5+ | `Dynamic` | `thinkingBudget: N` (integer) |
| Qwen3, DeepSeek R1 | `Internal` | No param — reasoning is opaque |
| Most others | `None` | Thinking disabled |

**`resolve(descriptor, effort) → ResolveResult`** applies a cascade of gates:
1. Clamp `max_tokens` to model's `max_output_tokens`
2. If model doesn't support temperature → remove it
3. If model doesn't support tools → disable tools
4. If model doesn't support thinking → degrade to effort 2
5. If internal thinker → keep `thinking: true` but emit no budget param
6. If model `max_output_tokens < 8192` → disable thinking (can't fit budget + response)
7. Cap thinking budget to `max_tokens - 1`

Returns `Resolved(ResolvedParams)`, `Degraded { requested, actual, reason }`, or `Unsupported`. The TUI can surface degradation to the user.

**Model lookup:** `lookup_descriptor(model_id)` prefix-matches against 40+ known models, then slug-matches ("opus", "devstral"), then falls back to conservative defaults. The registry covers all major Anthropic, OpenAI, Google, HuggingFace, and LM Studio models.

**User control:** `/effort low|medium|high|max|auto` or `one --effort <level>`. Stored on the session, not globally.

---

## Interleaved Reasoning / Thinking

**Goal: provider-agnostic reasoning events.** The ambition is that regardless of whether the model natively supports thinking (Anthropic extended thinking, OpenAI o-series, Gemini 2.5+, DeepSeek R1), or has no built-in reasoning API, One should surface equivalent reasoning signals through a combination of **native API**, **prompting**, **abstraction**, and **instrumentation**.

**Current state:** Thinking tokens are **invisible to the TUI**. The Anthropic provider consumes `"type": "thinking"` SSE blocks silently — they never reach `on_chunk`. Only text blocks are forwarded. The spinning verb (`⠹ Skedaddling…`) activates from the pre-stream empty chunk, so users see that the model is working but can't see the raw reasoning.

**How reasoning surfaces per provider type:**

| Provider type | Mechanism | Current One handling |
|---|---|---|
| Anthropic (native thinking) | `budget_tokens` in request; thinking SSE blocks in stream | Consumed silently; only text forwarded |
| OpenAI o-series | `reasoning_effort` enum; reasoning is opaque | Opaque — status line only |
| Gemini 2.5+ | `thinkingBudget` integer | Opaque — status line only |
| Internal thinkers (Qwen3, DeepSeek R1) | No API param; model reasons internally | Opaque — status line only |
| Non-thinking models via prompting | XML scratchpad in prompt (`<thinking>…</thinking>`) | Can be detected and stripped from final output |

**Normalisation strategy (three paths):**

1. **Native API** — use `budget_tokens` / `reasoning_effort` / `thinkingBudget` where supported; emit a `ThinkingChunk` event for each delta rather than consuming silently
2. **Prompting** — for models without native support, inject a `<thinking>…</thinking>` XML scratchpad instruction into the system prompt. Detect the opening tag in the stream and emit `ThinkingChunk` events; strip it before the final turn content
3. **Instrumentation** — for fully opaque models (o-series, internal thinkers), estimate thinking time from the gap between pre-stream empty chunk and first text delta; surface as "thought for Xs" in the status line

**Implementation additions needed:**
- `Event::ThinkingChunk { session_id, content }` — or a `thinking: bool` flag on `AiResponseChunk`
- `Session.thinking_buffer: String` — accumulates thinking content for the current turn
- TUI: show thinking collapsed in main view, expanded in transcript mode (`Ctrl+O`)
- `AiResponse.thinking: Option<String>` — persist reasoning for `/history`
- Status line "thought for Xs" slot — `processing_started` timer set when the empty pre-stream chunk fires, cleared when first text delta arrives
- Evergreen: decide whether to compress/archive thinking blocks separately from text

**Extended thinking request (Anthropic, current):**
```rust
// anthropic.rs — sent when budget_tokens > 0:
body["thinking"] = json!({ "type": "enabled", "budget_tokens": budget });
// In stream: "type": "thinking" blocks consumed silently, only text forwarded
```

---

## Context Management

### Auto-compact

`one-core/src/compact/auto_compact.rs` — fires automatically before each API call when token count approaches the model's context window.

**Thresholds:**
- Context window: model-specific (Claude: 200k, Opus: 1M)
- Auto-compact threshold: `context_window - max_output_tokens - 13,000 (buffer)`
- Warning threshold: threshold + 7,000 (shown before compaction fires)

**Summarisation strategy — the AI is asked to produce a structured summary with:**
1. Primary request and intent
2. Key concepts and decisions
3. Files and code modified
4. Errors encountered
5. Problem-solving approaches
6. All user messages verbatim
7. Pending tasks
8. Current work in progress
9. Optional next step

The AI response is wrapped in `<analysis>` (scratchpad, stripped) and `<summary>` (kept) XML blocks. Tool use is forbidden during compaction.

**What gets kept:** The compacted summary becomes a single User message at the start. Recent messages are preserved verbatim after it. The circuit breaker stops trying after 3 consecutive failures.

**Manual compact:** `/compact` — same summarisation but user-initiated, without the token threshold guard.

### Evergreen (Background Compression)

`one-core/src/evergreen.rs` + `one-cli/src/tasks/evergreen.rs` — a background task that compresses old conversation turns into SQLite while the session is active, independent of the in-memory context.

**Three-tier architecture:**
- **Write tier** (newest 10 turns): Always verbatim, never touched
- **Compress tier** (turns 11–50): First-pass AI summarisation
- **Archive tier** (turns 51+): Second-pass summarisation (summaries of summaries)

**Key constants:**
```
WRITE_TIER_TURNS = 10
COMPRESS_TIER_MAX_TURNS = 50
MIN_ELIGIBLE_TO_COMPRESS = 5     — don't fire unless 5+ turns are eligible
MIN_SPAN_TOKENS_TO_COMPRESS = 500 — skip tiny spans
```

**ROI gate:** A compression pass only runs if `tokens_saved > compression_api_cost` (estimated at 1,000 tokens). This ensures every compression call breaks even within one subsequent request.

**Token estimation:** `1 token ≈ 4 UTF-8 bytes` (fast heuristic, no tokeniser needed).

**Event bus:** On completion, emits `Event::EvergreenCompressed { session_id, turns_compressed }` back to the TUI.

**SQLite structure:** Each session gets its own DB at `~/.one/{project}/{session}/session.db`. Turns are marked `summarized` after compression; summary turns carry a `tier` field (`CompressFirst` or `ArchiveSecond`).

**Relationship to auto-compact:** Auto-compact operates on the in-memory `Vec<Message>` passed to the API. Evergreen operates on the SQLite-persisted conversation independently. They don't conflict — Evergreen is for long-term storage efficiency; auto-compact is for keeping API calls within context window limits.

---

## Agent Routing & Tool Filtering

`one-core/src/agent.rs` — keyword-based intent classification narrows the tool set per request.

**Agent roles and their keyword triggers:**

| Role | Keywords | Tools granted |
|---|---|---|
| Reader | "read", "show", "explain", "what does", "how does", "look at", "display" | `file_read`, `grep`, `glob` |
| Explorer | "find", "search", "where", "locate", "which file", "how many", "list all" | `grep`, `glob`, `file_read` |
| Writer | "change", "edit", "modify", "write", "create", "fix", "implement", "refactor" | `file_write`, `file_edit`, `file_read` |
| Executor | "run", "test", "build", "cargo", "npm", "docker", "compile", "deploy" | `bash`, `file_read` |
| Coordinator | ambiguous / routing only | no tools — describes available agents |

`classify_intent()` returns `Option<AgentRole>` — `None` means ambiguous, all tools are granted. The agent role's system prompt is appended to the main system prompt explaining the specialist context.

`filter_schemas(role, all_schemas)` keeps only tools in the role's `allowed_tools` list, reducing the token cost of the system prompt and narrowing what the model can reach for.

---

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
| `/debug` | Toggle debug mode — shows background activity as muted `⠒` lines in chat |
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
| `/debug` | Toggle debug mode |

## Special Input Syntax

| Syntax | What it does |
|---|---|
| `! <command>` | Run a shell command inline — result appears in chat |
| `@path/to/file` | Include file contents in the message |
| `Shift+Enter` | Insert a newline (multi-line input) |

## Key Bindings

| Key | Action |
|---|---|
| `Enter` | Submit message |
| `Shift+Enter` | New line in input |
| `Up` / `Down` | Browse input history |
| `Tab` | Accept autocomplete suggestion |
| `Escape` | Abort streaming / exit current mode |
| `Ctrl+C` | Clear input → cancel stream → double-press exits |
| `Ctrl+N` / `Ctrl+T` | New session |
| `Ctrl+W` | Close current session |
| `Ctrl+Shift+]` | Next tab |
| `Ctrl+Shift+[` | Previous tab |
| `Ctrl+O` | Full-screen transcript viewer |

### Transcript mode (`Ctrl+O`)
`j`/`k` scroll · `g`/`G` top/bottom · `PageUp`/`PageDown` page · `Ctrl+E` toggle full history · `Esc`/`q` exit

---

## TUI Rendering Details

**Tool call display:**
- Running: animated dot cycling through a random style (growing `· • ●`, falling-sand braille, fold `-≻›⟩|`, box bounce `▖▘▝▗`, or standard braille `⠋⠙⠹…`) — style picked via `subsec_nanos()` at `ToolRequest` time, frozen for the tool's duration
- Complete: static `⏺` with summarised output below
- Empty output: `⎿  (No output)` always shown

**Status line phases:**
- Processing (streaming, no first chunk yet): `⠹ Verb…`
- Receiving (chunks arriving): `⠹ Verb… (Xs · ↓ Nk tokens · effort)`
- Tool running: `⠹ Verb… (Xs · ↓ Nk tokens)` with per-tool elapsed timer

**Debug mode (`/debug`):** Muted `⠒ message` lines interleaved by timestamp with conversation turns. Sourced from `session.debug_events: Vec<(DateTime<Utc>, String)>` — ephemeral, not persisted.

**Banner:** Full welcome banner shown only while all sessions have empty conversations. Collapses globally (all tabs including inbox) the moment any session receives its first message.

---

## Session Lifecycle

- Sessions stored in `AppState.sessions: HashMap<String, Session>`
- Each session: `id` (UUID), `session_hash` (6-char hex for `--session` resume), `db_path` (SQLite), `branch` (git branch at creation), `project_path`, `conversation`, token/cost accumulators, `debug_events`
- On exit: prints session hash (`one --session <hash>`) and `⠒ project — duration · turns · ↑/↓ tokens · ~$cost`
- Resume with `one --session <hash>` or `one -c` (continue last session) or `/history`

---

## Configuration

Config at `~/.one/config.toml`. Key sections:
- `[provider]` — default provider, model, API keys, `fast_mode`, `max_tokens`
- `[pet]` — name, species, enabled
- `[integrations]` — GitHub, Slack, Asana, Notion tokens and settings

Config layering: **keyring → config file → env var → CLI flags** (later wins).

View with `/config`. Re-run setup with `/reset`.

## MCP Servers

Connect any MCP-compatible server. All sources use the standard `{ "mcpServers": { … } }` format from the MCP spec. Sources are loaded in priority order — project-level wins:

| Priority | Path |
|---|---|
| Lowest | Claude Desktop compat: `{config_dir}/Claude/claude_desktop_config.json` |
| | One global: `~/.one/mcp.json` |
| | Git root: `<git-root>/.mcp.json` · `<git-root>/mcp.json` |
| Highest | Project: `<project>/.mcp.json` · `<project>/mcp.json` |

Stdio server example (`.mcp.json`):
```json
{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
      "env": { "LOG_LEVEL": "info" }
    }
  }
}
```

Remote/SSE server example:
```json
{
  "mcpServers": {
    "remote": { "type": "sse", "url": "https://mcp.example.com/sse" }
  }
}
```

Env vars expand in all config values: `"${MY_TOKEN}"`. Schemas from MCP servers are merged with built-in tool schemas at startup. Deferred tools (loaded via `tool_search`) are listed in the system prompt by name so the model knows they exist without token cost. `/mcp` shows connected servers and tool counts.
