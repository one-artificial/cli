# One

A multi-project, multi-provider AI coding terminal built in Rust.

One runs multiple project sessions simultaneously in a tabbed TUI, routes to specialist agents to minimise context window usage, compresses conversation history in the background using a three-tier symbolic + neural pipeline, and treats open standards (AGENTS.md, MCP spec) as first-class citizens over any single vendor's format.

---

## Quick Start

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
cargo build --release
./target/release/one
```

Or with any other provider — One infers the provider from the model name:

```bash
one --model gpt-4o
one --model gemini-2.0-flash
one --model meta-llama/Llama-3.1-8B-Instruct
```

### One-Shot Mode

Pass a prompt as a positional argument to get a single response and exit — useful for scripts and piping:

```bash
one 'what does this project do?'
echo "$(git diff)" | one 'summarise these changes'
one --model haiku 'say hi'

# Generate shell completions
one --completions zsh >> ~/.zshrc
one --completions bash >> ~/.bashrc
```

### CLI Flags

| Flag | Description |
|---|---|
| `--project` / `-p` | Project directory (repeatable for multiple tabs) |
| `--model` / `-m` | Model name or shortcut (`opus`, `sonnet`, `haiku`, `gpt-4o`, …) |
| `--provider` | Override provider detection |
| `--effort` | Starting effort level (0–4 or name) |
| `--max-tokens` | Maximum output tokens |
| `--continue` / `-c` | Resume the last session for this project |
| `--session <hash>` | Resume a specific session by its 6-char hash |
| `--no-tools` | Disable all tools (text-only mode) |
| `--allowed-tools` | Pre-approve specific tools (repeatable) |
| `--dangerously-skip-permissions` | Bypass all permission checks |
| `--system-prompt` | Replace the system prompt entirely |
| `--append-system-prompt` | Append to the default system prompt |
| `--max-turns` | Max tool execution turns per query (default: 200) |
| `--verbose` | Enable debug logging to stderr |
| `--completions <shell>` | Print shell completions and exit |

---

## Features

### Multi-Project Tabs

Each project directory gets its own independent conversation tab. All tabs are live simultaneously.

```bash
one -p /work/api -p /work/frontend -p /work/infra
```

- `Ctrl+Shift+]` / `[` — cycle tabs
- `/new <path>` — open a new project tab mid-session
- `/switch <name>` — jump to a tab by project name
- `/close` — close the current tab (with confirmation)
- `/session` — list all active sessions

An **inbox tab** receives notifications from connected integrations (GitHub, Slack, Asana, Notion) without interrupting any active session.

---

### Six AI Providers

| Provider | Auth | Model shortcuts |
|---|---|---|
| Anthropic (Claude) | `ANTHROPIC_API_KEY` or `/login anthropic <key>` | `opus`, `sonnet`, `haiku` |
| OpenAI (GPT) | `OPENAI_API_KEY` or `/login openai <key>` | `gpt-4o`, `gpt-4o-mini`, `o3` |
| Google (Gemini) | `GOOGLE_API_KEY` or `/login google <key>` | `flash`, `pro` |
| Hugging Face | `/login huggingface` (browser OAuth) or `HF_TOKEN` | any `org/model` |
| Ollama | no auth | any local model name |
| LM Studio | no auth | any local model name |

Provider is auto-detected from `--model`; `--provider` overrides. `/provider` shows auth status for all.

Switch mid-session: `/model opus`, `/model gpt-4o`, `/model gemini-2.0-flash`.

---

### Effort System

Five effort levels control reasoning depth, token budget, and tool access. The model's actual capabilities are respected — effort degrades gracefully if a model doesn't support thinking.

| Level | Command | Max tokens | Thinking budget | Tool access | Context |
|---|---|---|---|---|---|
| 0 — minimal | `/effort 0` | 256 | — | none | last turn only |
| 1 — low | `/effort low` | 1,024 | — | none | recent turns |
| 2 — medium | `/effort medium` | 4,096 | — | auto | full history |
| 3 — high | `/effort high` | 8,192 | 5,000 tokens | auto | RAG |
| 4 — max | `/effort max` | 16,384 | 20,000 tokens | required | RAG + summary |

Thinking budgets are expressed in the format each provider expects: `budget_tokens` for Anthropic, `reasoning_effort` for OpenAI o-series, `thinkingBudget` for Gemini 2.5+, or silently omitted for models with internal reasoning (Qwen3, DeepSeek R1).

---

### Evergreen: Three-Tier Context Compression

One compresses conversation history in the background using a tiered pipeline, keeping the context window lean across long sessions without losing signal.

```
┌──────────────┬───────────────────────┬────────────────────────┐
│  COLD (warm) │   WARM (arc, 150–250w)│   HOT (recent, 300–500w│
│  coming soon │  session arc          │  structured record     │
└──────────────┴───────────────────────┴────────────────────────┘
                                              ↑ always verbatim ↑
                                         last 10 turns (write tier)
```

**Hot tier** (first-pass) produces a structured, machine-readable record:

```
GOAL: what we're building or fixing
STATE: current build/test status
DECIDED:
- [decision] — instead of [rejected] — because [reason]
ARTEFACTS:
- exact/file/paths.rs, functionName, ENV_VAR, table_name
ERRORS:
- exact error message → how resolved | OPEN if unresolved
OPEN:
- unresolved questions and next actions
RECALL_GAPS:
- anything suspected to be missing from this excerpt
```

**Warm tier** (second-pass) compresses hot summaries into a 150–250 word session arc, preserving the APPROACH, STABLE_ARTEFACTS, CONSTRAINTS, and SHARP_EDGES that constrain future decisions.

**Symbolic recall** — at query time, One uses:
- **BM25 scoring** to rank chunks by relevance to the current query
- **Artefact matching** to always include chunks that mention files or names in the query
- Cold/warm tiers always injected; hot tier filtered to the top-3 by relevance

The compression plan, parse results, and recall injection all appear as muted lines in the chat when `/debug` is on.

Toggle background compression: `/evergreen`

---

### Agent Routing

One classifies intent before every API call and routes to a specialist agent with a reduced tool set, cutting context window usage by 60–80%.

| Agent | Triggered by | Tools |
|---|---|---|
| Reader | "explain", "show", "what does", "how does" | `file_read`, `grep`, `glob` |
| Explorer | "find", "search", "where", "locate" | `grep`, `glob`, `file_read` |
| Writer | "edit", "fix", "implement", "refactor" | `file_write`, `file_edit`, `file_read` |
| Executor | "run", "test", "build", "cargo", "npm" | `bash`, `file_read` |
| Coordinator | ambiguous intent | delegates only |

Unambiguous queries get a narrow tool set. Ambiguous queries get all tools. Agent selection appears in the debug log.

---

### Open Standards First

One reads instruction files and config from multiple tools, with open standards taking precedence over platform-specific ones.

**Instruction files loaded into system prompt** (later = higher priority):

```
~/.gemini/GEMINI.md           ← platform, global
~/.codex/instructions.md      ← platform, global
~/.one/AGENTS.md              ← open standard, global  ┐
                                                         │ open standards
GEMINI.md / .cursorrules      ← platform, project       │ outrank platform
.clinerules / codex.md        ← platform, project       │ standards
AGENTS.md                     ← open standard, project ─┘ (always wins)
CLAUDE.md / .claude/CLAUDE.md ← CC-compat
CLAUDE.local.md               ← personal git-ignored overrides (highest)
```

**MCP servers** loaded in priority order (project wins over global):

| Source | Path |
|---|---|
| Lowest | `{config_dir}/Claude/claude_desktop_config.json` |
| | `~/.one/mcp.json` |
| | `<git-root>/.mcp.json` or `<git-root>/mcp.json` |
| Highest | `<project>/.mcp.json` or `<project>/mcp.json` |

All files use the standard `{ "mcpServers": { … } }` format. Env vars expand: `"${MY_TOKEN}"`.

**Skill loading** (custom slash commands) uses the same priority: `.gemini/commands/` → `.claude/commands/` → `.one/commands/`, at each of profile / git-root / project levels. Git-root level enables monorepo-wide shared skills.

---

### Custom Skills

Any `.md` file in a `commands/` directory becomes a slash command.

```markdown
---
description: Shown in autocomplete — controls when the model auto-invokes this skill
allowed-tools: Bash(git add:*), Bash(git commit:*)
argument-hint: <branch>
---

Review the changes in `$ARGUMENTS` for correctness and style.
Current branch: !`git branch --show-current`
```

- `$ARGUMENTS` — replaced with text after the command name
- `` !`command` `` — replaced with shell output at invocation time
- `allowed-tools` — restricts which tools the model can use while following this skill

Skills appear in autocomplete. The model can also invoke them autonomously via the `Skill` tool when the description matches.

---

### Background Systems

One runs several background tasks independently of the main conversation. Each can be toggled off to reduce token usage.

| Command | System | Status |
|---|---|---|
| `/evergreen` | Three-tier context compression | Built |
| `/chronicle` | Cross-session synthesis into cold-tier landmark records | On by default |
| `/prelude` | Next-prompt prediction with similarity matching | On by default |
| `/calibrate` | Skill improvement from detected preference corrections | On by default |
| `/palimpsest` | Living doc maintenance for `<!-- one:autodoc -->` files | On by default |

All background activity appears as muted `⠒` lines in the chat when `/debug` is enabled.

---

### Debug Mode

`/debug` toggles a stream of muted diagnostic lines visible in the conversation, showing every background system as it runs:

```
⠒ agent → Executor
⠒ api → claude-sonnet-4-6 (12 messages)
⠒ evergreen → compress pass: 8 turns ≈3,200 tokens [ids 45..52]
⠒ evergreen: parsed — 6 artefacts · 2 open · 1 errors · 4 decided
⠒ evergreen ← 8 turns → ≈280 tokens (91% reduction, saved ≈2,920)
⠒ evergreen: recall store — 1 hot · 0 warm · 0 cold · 6 total artefacts
⠒ api ← ↑ 4,102 / ↓ 312 tokens
⠒ recall: injecting 1 hot · 0 warm · 0 cold chunks into system prompt
⠒ memory: auto-saved project «prefer snake_case for Rust functions»
⠒ github: 2 new notification(s)
```

Lines are interleaved with conversation turns by timestamp, so the sequence reflects exactly when each system ran.

---

### Tool Display

While a tool is running, One animates the header dot using a randomly-selected style — different for every tool call:

```
· Bash(cargo test --workspace)       ← growing   · • ● • ·
  ⎿  running…
⠹ Wrangling… (12s · ↓ 4.1k tokens)
```

```
⠁ Read(crates/one-core/src/session.rs)   ← falling sand  ⠁⠂⠄⡀⣀…
  ⎿  83 lines
```

When complete, the dot becomes static `⏺` and output is summarised:

```
⏺ Bash(cargo test --workspace)
  ⎿  201 passed, 0 failed
```

The status line below shows three distinct phases:

| Phase | Display |
|---|---|
| Processing (waiting for first token) | `⠹ Verb…` |
| Receiving (tokens streaming in) | `⠹ Verb… (12s · ↓ 4.1k tokens)` |
| Tool running | `⠹ Verb… (3s · ↓ 4.1k tokens)` with per-tool timer |

---

### Available Tools

23 built-in tools available to the AI, split into active (always loaded) and deferred (loaded on demand via `tool_search`):

**File system**
- `file_read` — Read files with line numbers, offset/limit, image support, notebook rendering
- `file_write` — Create or overwrite files
- `file_edit` — Exact string search-and-replace (requires prior `file_read`)
- `glob` — Find files by pattern, sorted by modification time
- `grep` — Search file contents (ripgrep, regex, multiline)
- `notebook_edit` — Edit Jupyter notebook cells

**Execution**
- `bash` — Run shell commands; supports background execution
- `sleep` — Pause between commands

**Web**
- `web_fetch` — Fetch a URL and extract readable text
- `web_search` — Search the web via DuckDuckGo

**AI and agents**
- `agent` — Delegate work to a sub-agent (supports background spawn, worktree isolation, model override)
- `ask_user` — Ask the user a question with optional multiple-choice options
- `skill` — Load and invoke a custom skill by name

**Context and memory**
- `recall_detail` — Retrieve the original uncompressed messages from an Evergreen-compressed span by message ID range
- `todo_write` — Write a structured task list visible in the TUI

**Planning**
- `enter_plan_mode` / `exit_plan_mode` — Toggle plan mode from within a tool loop

**MCP**
- `list_mcp_resources` — List resources exposed by connected MCP servers
- `read_mcp_resource` — Read a specific MCP resource by URI

**Scheduling**
- `cron_create` — Schedule a recurring task (5-field cron expression)
- `cron_delete` — Remove a scheduled task by ID
- `cron_list` — List all scheduled tasks

**Worktree**
- `enter_worktree` / `exit_worktree` — Isolate tool execution in a temporary git worktree (deferred)

**Discovery**
- `tool_search` — Load deferred tool schemas on demand by keyword or `select:Name` query

**Plugin tools** — Script-type plugins in `~/.one/plugins/` register additional tools automatically.

---

### Inline Shell and File Includes

Prefix with `!` to run a shell command without asking the AI:

```
! cargo test
! git log --oneline -10
! ls -la src/
```

Prefix a file path with `@` to include its contents in your next message:

```
explain @src/main.rs
review the changes in @crates/one-tui/src/app.rs
```

---

### Integrations

Four external services poll for updates in the background and surface notifications in the inbox tab.

| Integration | What it tracks | Config key |
|---|---|---|
| GitHub | PRs, issues, comments | `[integrations.github]` |
| Slack | Channel messages | `[integrations.slack]` |
| Asana | Task assignments and updates | `[integrations.asana]` |
| Notion | Page updates | `[integrations.notion]` |

```toml
[integrations.github]
token = "ghp_..."
repos = ["owner/repo", "owner/other-repo"]

[integrations.slack]
token = "xoxb-..."
channels = ["C01234567"]

[integrations.asana]
token = "..."
workspace = "..."

[integrations.notion]
token = "secret_..."
```

`/inbox` shows notification count. Switch to the inbox tab to browse all.

---

### Session Persistence and Recall

Every conversation is stored in a per-session SQLite database. Sessions are restored automatically when opening the same project directory.

```bash
# Resume last session for this project
one -c

# Resume a specific session by hash
one --session abc123

# Browse all sessions
one          # then use /history
```

Evergreen chunks are stored in the session DB with full structured fields (goal, artefacts, errors, open items) for deterministic symbolic retrieval — not just raw text blobs.

---

## Commands

### Session

| Command | Description |
|---|---|
| `/new <path>` | Open new project tab |
| `/close` | Close current tab (y/n confirm) |
| `/switch <name>` | Jump to tab by project name |
| `/session` | List active sessions |
| `/history` | Browse all previous sessions |
| `/import` | Import session from another tool |

### Model and Effort

| Command | Description |
|---|---|
| `/model <name>` | Switch model (opus, sonnet, haiku, gpt-4o, flash, …) |
| `/effort <level>` | Set reasoning depth (0–4 or minimal/low/medium/high/max) |
| `/cost` | Token usage and estimated cost |
| `/fast` | Toggle fast streaming mode |
| `/provider` | Auth status for all providers |

### Context

| Command | Description |
|---|---|
| `/compact` | Manually compact conversation |
| `/clear` | Clear conversation |
| `/config` | Show current configuration |

### Background Systems

| Command | Description |
|---|---|
| `/debug` | Toggle debug visibility (muted lines for all systems) |
| `/plan` | Toggle plan mode (describe tools without executing) |
| `/evergreen` | Toggle background context compression |
| `/chronicle` | Toggle cross-session synthesis (coming soon) |
| `/prelude` | Toggle speculative pre-computation (coming soon) |
| `/calibrate` | Toggle skill improvement analysis (coming soon) |
| `/palimpsest` | Toggle living doc maintenance (coming soon) |

### Memory and Tasks

| Command | Description |
|---|---|
| `/memory` | List memories (`/memory search <query>` to filter) |
| `/memory delete <n>` | Delete memory by name |
| `/remember <text>` | Save a quick project memory |
| `/tasks` | List tasks (`/tasks add <desc>`, `/tasks done <id>`) |

### Auth

| Command | Description |
|---|---|
| `/login <provider> [<key>]` | Store API key or browser OAuth |
| `/logout <provider>` | Remove stored credentials |
| `/reset` | Re-run setup wizard on next launch |

### Tooling

| Command | Description |
|---|---|
| `/tools` | List built-in + MCP tools |
| `/skills` | List installed custom commands |
| `/mcp` | MCP server connections and tools |
| `/permissions` | Active permission rules |
| `/diff` | `git diff --stat` |
| `/git <cmd>` | Run a git command inline |
| `/pr` | AI-guided pull request creation |
| `/commit` | AI-guided git commit |

### Info

| Command | Description |
|---|---|
| `/help` | All commands |
| `/status` | Connection and provider info |
| `/doctor` | System health check |
| `/inbox` | Notification count |
| `/version` | Current version |
| `/bug` | Report an issue |

---

## Key Bindings

### Navigation

| Key | Action |
|---|---|
| `Ctrl+Shift+]` / `Ctrl+N` | Next tab |
| `Ctrl+Shift+[` / `Ctrl+P` | Previous tab |
| `Ctrl+T` | New session |
| `Ctrl+O` | Full-screen transcript view |
| `PageUp` / `PageDown` | Scroll conversation |

### Input

| Key | Action |
|---|---|
| `Enter` | Submit message |
| `Shift+Enter` | Insert newline |
| `Up` / `Down` | Browse input history |
| `Tab` | Accept autocomplete |
| `Ctrl+C` | Clear input → cancel stream → exit (double) |
| `Escape` | Abort streaming response |

### Editing

| Key | Action |
|---|---|
| `Ctrl+A` | Move to start of line |
| `Ctrl+E` | Move to end of line |
| `Ctrl+U` | Delete to start of line |
| `Ctrl+K` | Delete to end of line |
| `Ctrl+W` / `Alt+Backspace` | Delete word backward |

> Use `/close` to close the current session.

### Transcript Mode (`Ctrl+O`)

| Key | Action |
|---|---|
| `j` / `k` or `↑` / `↓` | Scroll |
| `g` / `Home` | Jump to top |
| `G` / `End` | Jump to bottom |
| `Ctrl+E` | Toggle full history / condensed |
| `Esc` / `q` | Exit |

### Permission Prompts

| Key | Action |
|---|---|
| `y` / `1` | Allow once |
| `a` / `2` | Always allow |
| `n` / `3` | Deny |

---

## Configuration

Config lives at `~/.one/config.toml`, created automatically on first run.

```toml
[provider]
default_provider = "anthropic"
default_model    = "claude-sonnet-4-20250514"
max_tokens       = 8192

[provider.anthropic]
api_key = "sk-ant-..."   # or use ANTHROPIC_API_KEY env var

[provider.openai]
api_key = "sk-..."       # or use OPENAI_API_KEY env var

[provider.google]
api_key = "..."          # or use GOOGLE_API_KEY env var

[pet]
name    = "Pixel"
species = "duck"         # duck | cat | dog | fox | crab
enabled = true

[integrations.github]
token = "ghp_..."
repos = ["owner/repo"]

[integrations.slack]
token   = "xoxb-..."
channels = ["C01234567"]
```

**Priority order**: keyring → config file → environment variable → CLI flags (later overrides earlier).

### Hooks (`settings.json`)

One supports shell hooks that fire at conversation lifecycle events. Create `.one/settings.json` in a project (or `~/.one/settings.json` globally):

```json
{
  "hooks": {
    "PostResponse": [
      {
        "command": "notify-send 'One' 'Response ready'",
        "timeout": 5
      }
    ],
    "PreToolUse": [
      {
        "command": "echo 'About to run: $TOOL_NAME'",
        "if": "Bash(git *)"
      }
    ]
  }
}
```

| Hook event | Fires when |
|---|---|
| `PreToolUse` | Before a tool executes |
| `PostToolUse` | After a tool completes |
| `PostResponse` | After the AI produces a final response |
| `UserPromptSubmit` | When the user submits a message |
| `Stop` | When a response stream ends |
| `SessionStart` | When a session is created |

`if` matchers filter by tool and input (e.g., `"Bash(git *)"`, `"Read(*.ts)"`).

### Permissions (`settings.json`)

Permission modes control how aggressively One asks for approval:

| Mode | Behaviour |
|---|---|
| `default` | Prompt for non-read-only tools |
| `acceptEdits` | Auto-approve file edits; still prompt for shell |
| `bypassPermissions` | Skip all checks — dangerous, use with care |

Per-tool rules and session-level overrides are also supported. View current rules: `/permissions`.

### Keybindings

Custom key bindings live at `~/.one/keybindings.json` (or `~/.claude/keybindings.json` for CC compatibility):

```json
[
  { "key": "ctrl+enter", "action": "submit" },
  { "key": "ctrl+shift+n", "action": "tab_next" }
]
```

Available actions: `submit`, `newline`, `cancel`, `clear`, `history_prev`, `history_next`, `tab_next`, `tab_prev`, `scroll_up`, `scroll_down`, `autocomplete`, `interrupt`.

---

## Session Import

One can import conversation history from other AI coding tools. Run `/import` to browse available sessions.

| Source | Location |
|---|---|
| Claude Code | `~/.claude/projects/{project-hash}/{session-id}.jsonl` |
| OpenAI Codex | `~/.codex/state_5.sqlite` |
| Gemini CLI | `~/.gemini/tmp/{session-id}/checkpoint.json` |

---

## Themes

The first-run onboarding wizard offers five theme options (re-run with `/reset`):

| Theme | Description |
|---|---|
| Dark | Default dark terminal |
| Light | Light background |
| Dark (colorblind) | Blue/orange instead of green/red |
| Light (colorblind) | Light + colorblind-safe palette |
| Dark ANSI | 16-colour only, no RGB — for limited terminals |

---

## Setup

First run triggers interactive onboarding (provider selection, API key entry, theme picker). Re-run at any time with `/reset`.

For new contributor setup, activate pre-commit hooks (fmt + clippy + check):

```bash
git config core.hooksPath .github/hooks
```

---

## Architecture

```
one-cli (binary)
├── one-tui          — ratatui TUI: tabs, input, tool rendering, autocomplete, commands
├── one-core         — event bus, state, query engine, effort, evergreen, agent routing, MCP
├── one-ai           — provider implementations (Anthropic, OpenAI, Google, Ollama, HF, LM Studio)
├── one-tools        — tool implementations (Bash, Read, Edit, Glob, Grep, Agent, …)
├── one-integrations — GitHub, Slack, Asana, Notion background polling
└── one-db           — SQLite persistence (sessions, messages, tool calls, evergreen chunks)
```

All subsystems communicate through typed events on a tokio broadcast channel. No circular crate dependencies — `one-core` defines traits; other crates implement them.

---

## License

Apache-2.0
