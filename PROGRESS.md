# One — Development Progress

## Current Status (2026-04-09)

### Stats
- **184 tests**, 0 warnings, 0 failures
- **24 tools** (11 active + 13 deferred) — matches 24/26 of CC's tools
- **33+ slash commands** (extensible via skills)
- **~95% Claude Code parity**

### Working
- Full TUI with streaming, markdown, syntax highlighting, themes
- Multi-provider: Anthropic, OpenAI, Ollama, Google, Hugging Face, LM Studio
- 24 tools: file_read/write/edit, bash, grep, glob, agent, skill, todo_write, ask_user, tool_search, web_fetch/search, sleep, plan_mode, cron, mcp_resources, notebook_edit, enter/exit_worktree
- Sub-agents: sync + background + worktree + fork mode + per-agent model
- MCP: stdio + SSE transports, tool discovery, resource reading
- Skills: user-installable (.one/commands/), marketplace, CC-compatible
- Skill features: allowed-tools, !`command` interpolation, $ARGUMENTS, recursive dirs
- @file mentions, ! shell prefix, input history, multi-line input (Shift+Enter)
- Readline keybindings (Ctrl+A/E/K/U/W/L)
- Vim keybindings (normal/insert mode, 14 motions)
- Animated spinner, Escape to abort, enhanced status bar
- Permission system with tool approval prompts
- Persistent memory (user, feedback, project, reference types)
- Hooks (PreToolUse, PostToolUse, PostResponse, SessionStart)
- OAuth PKCE, macOS Keychain, credential sharing with Claude Code
- Session management, multi-project tabs, context compaction
- Git-aware system prompt, /commit and /pr with dynamic context
- One-shot mode with streaming, retry, --output-format json

### Remaining Items (external dependencies)
- [ ] Amazon Bedrock (AWS SDK integration)
- [ ] Google Vertex AI (GCP SDK integration)
- [ ] MCP OAuth + auto-reconnect
- [ ] LSP integration (go-to-definition, diagnostics)
- [ ] WASM plugin runtime (extism)
- [ ] GitHub OAuth (federated identity)
- [ ] IDE extensions (VS Code, JetBrains — separate codebase)
- [ ] Web interface — separate application
- [ ] Desktop app — separate application

### Known Blocker
TLS fingerprinting: Sonnet/Opus get HTTP 429 while Claude Code succeeds. Root cause: JA3/JA4 at Cloudflare level. Haiku works fine. All models work via `claude -p` proxy.
