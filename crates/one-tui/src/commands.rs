use one_core::state::SharedState;

/// Result of executing a slash command
pub enum CommandResult {
    /// Display this message in the conversation
    Message(String),
    /// Clear the conversation
    ClearConversation,
    /// Create a new session for a project path
    NewSession { project_path: String },
    /// Close the current session
    CloseSession,
    /// Switch to a session by project name
    SwitchSession { name: String },
    /// No visible output
    Silent,
    /// Start OAuth browser login flow
    OAuthLogin { provider: String },
    /// Send this text as a user message to the AI (for /pr, /commit, etc.)
    SendToAi(String),
    /// Not a recognized command — pass to AI
    NotACommand,
    /// Quit the application
    Quit,
    /// Open the interactive session import picker
    OpenImportPicker,
}

/// Process a slash command. Returns how to handle it.
pub async fn handle_command(
    input: &str,
    state: &SharedState,
    pet: &mut crate::pet::Pet,
) -> CommandResult {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return CommandResult::NotACommand;
    }

    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
    let cmd = parts[0];
    let args = parts.get(1).unwrap_or(&"");

    match cmd {
        "/help" => CommandResult::Message(help_text()),
        "/clear" => CommandResult::ClearConversation,
        "/pet" => handle_pet(args, pet),
        "/inbox" => handle_inbox(args, state).await,
        "/session" | "/sessions" => handle_sessions(state).await,
        "/new" => handle_new_session(args),
        "/close" => CommandResult::CloseSession,
        "/switch" => handle_switch(args, state).await,
        "/status" => handle_status(state).await,
        "/login" => handle_login(args),
        "/logout" => handle_logout(args),
        "/reset" => handle_reset(),
        "/provider" => handle_provider(args).await,
        "/plugin" | "/plugins" => handle_plugins(),
        "/import" => CommandResult::OpenImportPicker,
        "/history" => handle_all_sessions(),
        "/one-md" | "/onemd" => CommandResult::SendToAi(
            "Please use the OneMd tool to generate or update ONE.md for this project. \
             First read the existing file (action=read) — if it exists, preserve and improve it. \
             Then write a complete, accurate ONE.md (action=write) covering: project purpose, \
             tech stack, architecture overview, key design decisions, coding conventions, \
             build/test commands, and any important gotchas. Keep it concise and scannable."
                .to_string(),
        ),
        "/model" => handle_model(args, state).await,
        "/cost" => handle_cost(state).await,
        "/compact" => handle_compact(state).await,
        "/config" => handle_config(),
        "/version" => CommandResult::Message(format!("one v{}", env!("CARGO_PKG_VERSION"))),
        "/effort" => handle_effort(args, state).await,
        "/fast" => handle_fast(state).await,
        "/diff" => handle_diff(),
        "/git" => handle_git(args),
        "/doctor" => handle_doctor().await,
        "/bug" | "/issue" => CommandResult::Message(
            "Report issues at: https://github.com/one-artificial/cli/issues".to_string(),
        ),
        "/debug" => handle_debug(state).await,
        "/plan" => handle_plan(state).await,
        "/permissions" | "/perms" => handle_permissions(),
        "/mcp" => handle_mcp(),
        "/memory" | "/memories" => handle_memory(args, state).await,
        "/remember" => handle_remember(args, state).await,
        "/tasks" | "/task" => handle_tasks(args, state).await,
        "/tools" => CommandResult::Message(handle_tools()),
        "/skills" | "/skill" => handle_skills(args, state).await,
        "/settings" => handle_settings(state).await,
        "/pr" => handle_pr(state).await,
        "/commit" => handle_commit(state).await,
        _ => {
            // Check for custom skills (user-installed slash commands)
            let cmd_name = cmd.strip_prefix('/').unwrap_or(cmd);
            let project_dir = {
                let s = state.read().await;
                s.active_session()
                    .map(|s| s.project_path.clone())
                    .unwrap_or_else(|| ".".to_string())
            };
            let skills = one_core::skills::load_skills(&project_dir);
            if let Some(skill) = skills.iter().find(|s| s.name == cmd_name) {
                // Prepare the skill prompt: substitute $ARGUMENTS and
                // interpolate !`command` patterns with real shell output
                let prompt = one_core::skills::prepare_skill_prompt(skill, args, &project_dir);
                CommandResult::SendToAi(prompt)
            } else {
                CommandResult::Message(format!(
                    "Unknown command: {cmd}\nType /help for available commands."
                ))
            }
        }
    }
}

fn help_text() -> String {
    "## One — Commands

/help              Show this help message
/clear             Clear the current conversation
/compact           Compact conversation to save context
/model <name>      Switch model (e.g. /model opus, /model haiku)
/cost              Show token usage and estimated cost
/config            Show current configuration
/version           Show version
/new <path>        Open a new project session
/close             Close the current session
/switch <name>     Switch to a session by project name
/session           List active sessions
/login              Sign in with Hugging Face (browser OAuth)
/logout             Sign out (clears HF identity, keeps API keys)
/reset              Re-run setup wizard (exits app, onboarding on next launch)
/provider           Show providers and auth status
/pet               Show your pet's status
/pet name <n>      Rename your pet
/pet species <s>   Change species (duck, cat, dog, fox, crab)
/inbox             Show notification count
/status            Show connection and provider status
/plugin            List installed plugins
/import            Import a session (Claude Code, Codex, Gemini picker)
/history           Browse previous sessions (text list)
/one-md            Generate or update ONE.md (AI writes project context file)
/effort <level>    Set reasoning effort (low/medium/high/max/auto)
/fast              Toggle fast mode (faster streaming)
/diff              Show git diff summary
/git <cmd>         Run a git command
/doctor            Check system health
/bug               Report an issue
/plan              Toggle plan mode (describe actions without executing)
/debug             Toggle debug mode (show background activity as muted lines)
/permissions       Show permission settings and rules
/mcp               Show MCP server connections and tools
/memory            List saved memories (or /memory search <query>)
/memory delete <n> Delete a memory by name
/remember <text>   Save a quick project memory
/tasks             List tasks (or /tasks add <desc>, /tasks done <id>)
/tools             List available tools (built-in + deferred)
/skills            List installed custom commands
/settings          Show all current settings
/commit            Create a git commit (AI-guided)
/pr                Create a pull request (AI-guided)

## Special Syntax

! <command>        Run a shell command inline (e.g. ! git status)
@path/to/file      Include file contents in your message
Shift+Enter        Insert newline (multi-line input)
Up/Down arrow      Browse input history
PageUp/PageDown    Scroll conversation
Ctrl+O             Open transcript view (full conversation)
Ctrl+E             Toggle show all / recent in transcript
Ctrl+B             Toggle ONE.md sidebar (project context)
Ctrl+T             New session
Ctrl+W             Close session (with confirmation)
Ctrl+Shift+[/]     Cycle between tabs
Ctrl+N/P           Next/previous tab
Ctrl+A/E           Move to start/end of line
Ctrl+K/U           Kill to end/start of line
Alt+Backspace      Delete word backward
Ctrl+Backspace     Delete entire line
Ctrl+C             Clear input / cancel stream / exit
Ctrl+L             Clear screen
Escape             Abort current AI request
"
    .to_string()
}

fn handle_pet(args: &str, pet: &mut crate::pet::Pet) -> CommandResult {
    let parts: Vec<&str> = args.splitn(2, ' ').collect();

    match parts.first().copied().unwrap_or("") {
        "" => {
            // Show pet status
            CommandResult::Message(format!(
                "{} the {:?} — mood: {:?}",
                pet.name, pet.species, pet.mood
            ))
        }
        "name" => {
            if let Some(name) = parts.get(1) {
                let old_name = pet.name.clone();
                pet.name = name.to_string();
                CommandResult::Message(format!("Renamed {old_name} to {}!", pet.name))
            } else {
                CommandResult::Message("Usage: /pet name <new_name>".to_string())
            }
        }
        "species" => {
            if let Some(species) = parts.get(1) {
                let new_pet = crate::pet::Pet::new(pet.name.clone(), species, pet.enabled);
                *pet = new_pet;
                CommandResult::Message(format!("{} is now a {:?}!", pet.name, pet.species))
            } else {
                CommandResult::Message("Usage: /pet species <duck|cat|dog|fox|crab>".to_string())
            }
        }
        _ => CommandResult::Message(
            "Unknown pet command. Try /pet, /pet name, /pet species".to_string(),
        ),
    }
}

async fn handle_inbox(args: &str, state: &SharedState) -> CommandResult {
    let parts: Vec<&str> = args.splitn(2, ' ').collect();
    let subcmd = parts.first().copied().unwrap_or("");

    match subcmd {
        "open" => {
            // Open a notification as a working session
            let idx_str = parts.get(1).unwrap_or(&"1").trim();
            let idx: usize = idx_str.parse().unwrap_or(1).max(1) - 1;

            let s = state.read().await;
            if idx >= s.notifications.len() {
                return CommandResult::Message(format!(
                    "No notification at index {}. {} notifications available.",
                    idx + 1,
                    s.notifications.len()
                ));
            }

            let notif = &s.notifications[idx];
            let source = format!("{:?}", notif.source);
            let prompt = format!(
                "I received a notification from {} that I need help with:\n\n\
                 **{}**\n\n{}\n\n{}Please help me address this.",
                source,
                notif.title,
                notif.body,
                notif
                    .url
                    .as_ref()
                    .map(|u| format!("URL: {u}\n\n"))
                    .unwrap_or_default()
            );
            drop(s);

            CommandResult::SendToAi(prompt)
        }
        "clear" => {
            let mut s = state.write().await;
            let count = s.notifications.len();
            s.notifications.clear();
            CommandResult::Message(format!("Cleared {count} notifications."))
        }
        "" | "list" => {
            let s = state.read().await;
            let count = s.notifications.len();

            if count == 0 {
                return CommandResult::Message("No notifications.".to_string());
            }

            let mut lines = vec![format!("## Inbox ({count} notifications)\n")];
            for (i, n) in s.notifications.iter().enumerate() {
                let source = format!("{:?}", n.source);
                let age = chrono::Utc::now()
                    .signed_duration_since(n.timestamp)
                    .num_minutes();
                let time = if age < 60 {
                    format!("{age}m ago")
                } else {
                    format!("{}h ago", age / 60)
                };
                lines.push(format!("  {}. [{source}] {} — {time}", i + 1, n.title));
            }
            lines.push(String::new());
            lines.push("Use `/inbox open <N>` to work on a notification".to_string());
            lines.push("Use `/inbox clear` to clear all".to_string());

            CommandResult::Message(lines.join("\n"))
        }
        _ => CommandResult::Message(
            "Usage:\n  /inbox             List notifications\n  \
             /inbox open <N>     Open notification as a task\n  \
             /inbox clear        Clear all notifications"
                .to_string(),
        ),
    }
}

fn handle_new_session(args: &str) -> CommandResult {
    let path = args.trim();

    // If no path given, use current working directory
    let resolved = if path.is_empty() {
        std::env::current_dir()
            .map(|cwd| cwd.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".to_string())
    } else if std::path::Path::new(path).is_absolute() {
        path.to_string()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path).to_string_lossy().to_string())
            .unwrap_or_else(|_| path.to_string())
    };

    if !std::path::Path::new(&resolved).is_dir() {
        return CommandResult::Message(format!("Not a directory: {resolved}"));
    }

    CommandResult::NewSession {
        project_path: resolved,
    }
}

async fn handle_switch(args: &str, state: &SharedState) -> CommandResult {
    let name = args.trim().to_lowercase();
    if name.is_empty() {
        return CommandResult::Message(
            "Usage: /switch <project_name>\nUse /session to see available sessions.".to_string(),
        );
    }

    let s = state.read().await;
    for (id, session) in &s.sessions {
        if session.project_name.to_lowercase().contains(&name) {
            return CommandResult::SwitchSession { name: id.clone() };
        }
    }

    CommandResult::Message(format!(
        "No session matching \"{name}\". Use /session to list sessions."
    ))
}

async fn handle_sessions(state: &SharedState) -> CommandResult {
    let s = state.read().await;
    let active = s.active_session_id.as_deref().unwrap_or("none");

    let mut lines = vec![format!("{} sessions:", s.sessions.len())];
    for (id, session) in &s.sessions {
        let marker = if id == active { " *" } else { "" };
        let turns = session.conversation.turns.len();
        lines.push(format!(
            "  {} ({} turns){marker}",
            session.project_name, turns
        ));
    }

    CommandResult::Message(lines.join("\n"))
}

async fn handle_status(state: &SharedState) -> CommandResult {
    let s = state.read().await;
    let provider = s
        .active_session()
        .map(|s| format!("{}", s.model_config.provider))
        .unwrap_or_else(|| "none".to_string());
    let model = s
        .active_session()
        .map(|s| s.model_config.model.clone())
        .unwrap_or_else(|| "none".to_string());
    let sessions = s.sessions.len();
    let notifs = s.notifications.len();

    CommandResult::Message(format!(
        "Provider: {provider}\nModel: {model}\nSessions: {sessions}\nNotifications: {notifs}"
    ))
}

fn handle_reset() -> CommandResult {
    // Reset onboarding flag and exit — next launch re-runs setup
    if let Ok(mut config) = one_core::config::AppConfig::load() {
        config.has_completed_onboarding = false;
        let _ = config.save();
    }
    CommandResult::Quit
}

fn handle_login(_args: &str) -> CommandResult {
    // Login is HF identity only — opens browser OAuth
    CommandResult::OAuthLogin {
        provider: "huggingface".to_string(),
    }
}

fn handle_logout(_args: &str) -> CommandResult {
    // Logout clears HF identity only — API keys are auth artifacts, not identity
    let mut messages = Vec::new();

    // Delete HF OAuth tokens
    match one_core::credentials::CredentialStore::delete("huggingface_oauth") {
        Ok(()) => messages.push("Logged out of Hugging Face.".to_string()),
        Err(e) => messages.push(format!("Failed to clear HF auth: {e}")),
    }
    let _ = one_core::credentials::CredentialStore::delete("huggingface");

    messages.push("API keys for model providers are unchanged.".to_string());
    messages.push("Use /reset to re-run setup.".to_string());

    CommandResult::Message(messages.join("\n"))
}

async fn handle_provider(_args: &str) -> CommandResult {
    let providers = [
        ("anthropic", "ANTHROPIC_API_KEY"),
        ("openai", "OPENAI_API_KEY"),
        ("google", "GOOGLE_API_KEY"),
        ("huggingface", "HF_TOKEN"),
        ("ollama", ""),
    ];

    let mut lines = vec!["Provider auth status:".to_string()];

    for (name, env_var) in &providers {
        // Check OAuth tokens
        let oauth_status = one_core::credentials::CredentialStore::get(&format!("{name}_oauth"))
            .ok()
            .flatten()
            .and_then(|json| serde_json::from_str::<one_core::oauth::OAuthTokens>(&json).ok())
            .map(|tokens| {
                if tokens.is_expired() {
                    "oauth (expired)".to_string()
                } else if let Some(ref email) = tokens.account_email {
                    format!("oauth ({email})")
                } else {
                    "oauth".to_string()
                }
            });

        let keyring = one_core::credentials::CredentialStore::get(name)
            .ok()
            .flatten()
            .is_some();
        let env = !env_var.is_empty() && std::env::var(env_var).is_ok();

        let status = if let Some(ref oauth) = oauth_status {
            oauth.as_str()
        } else if *name == "ollama" {
            "local (no auth needed)"
        } else if keyring {
            "api key (keyring)"
        } else if env {
            "api key (env var)"
        } else {
            "not configured"
        };

        lines.push(format!("  {name}: {status}"));
    }

    lines.push(String::new());
    lines.push(
        "Identity: /login (Hugging Face)  |  API keys: set during onboarding or in config.toml"
            .to_string(),
    );

    CommandResult::Message(lines.join("\n"))
}

fn handle_plugins() -> CommandResult {
    let registry = one_core::plugin::PluginRegistry::discover();
    let plugins = registry.all();

    if plugins.is_empty() {
        return CommandResult::Message(
            "No plugins installed.\n\n\
             To create a plugin, add a directory to ~/.one/plugins/ with a plugin.toml:\n\n\
             name = \"my-plugin\"\n\
             version = \"0.1.0\"\n\
             description = \"My custom plugin\"\n\n\
             [plugin_type.Script]\n\
             entrypoint = \"run.sh\"\n\n\
             [[commands]]\n\
             name = \"my-command\"\n\
             description = \"Does something cool\""
                .to_string(),
        );
    }

    let mut lines = vec![format!("{} plugins:", plugins.len())];
    for plugin in plugins {
        let status = if plugin.enabled {
            "enabled"
        } else {
            "disabled"
        };
        lines.push(format!(
            "  {} v{} — {} [{}]",
            plugin.manifest.name, plugin.manifest.version, plugin.manifest.description, status
        ));

        for cmd in &plugin.manifest.commands {
            lines.push(format!("    /{}: {}", cmd.name, cmd.description));
        }
    }

    CommandResult::Message(lines.join("\n"))
}

async fn handle_model(args: &str, state: &SharedState) -> CommandResult {
    let model_name = args.trim();
    if model_name.is_empty() {
        let s = state.read().await;
        let current = s
            .active_session()
            .map(|s| format!("{} ({})", s.model_config.model, s.model_config.provider))
            .unwrap_or_else(|| "none".to_string());
        return CommandResult::Message(format!(
            "Current model: {current}\n\n\
             Switch with: /model <name>\n\
             Shortcuts: opus, sonnet, haiku, gpt-4o\n\
             HuggingFace: /model meta-llama/Llama-3.1-8B-Instruct\n\
             Provider auto-detected from model name."
        ));
    }

    let resolved = one_core::provider::resolve_model_shortcut(model_name).to_string();

    // Auto-infer provider from model name
    let inferred_provider = one_core::provider::infer_provider(&resolved);

    {
        let mut s = state.write().await;
        if let Some(session) = s.active_session_mut() {
            session.model_config.model = resolved.clone();
            if let Some(provider) = inferred_provider {
                session.model_config.provider = provider;
            }
        }
    }

    // Persist as the default model for this provider
    if let Ok(mut config) = one_core::config::AppConfig::load() {
        config.provider.default_model = resolved.clone();
        if let Some(provider) = inferred_provider {
            config.provider.default_provider = format!("{provider}");
        }
        let _ = config.save();
    }

    let provider_note = inferred_provider
        .map(|p| format!(" (provider: {p})"))
        .unwrap_or_default();

    CommandResult::Message(format!(
        "Switched to model: {resolved}{provider_note} (saved as default)"
    ))
}

async fn handle_cost(state: &SharedState) -> CommandResult {
    let s = state.read().await;
    if let Some(session) = s.active_session() {
        let model = &session.model_config.model;
        let turns = session.conversation.turns.len();
        let input_tokens = session.total_input_tokens;
        let output_tokens = session.total_output_tokens;
        let total_tokens = input_tokens + output_tokens;

        CommandResult::Message(format!(
            "Session: {turns} turns, {total_tokens} tokens ({input_tokens} in / {output_tokens} out)\n\
             Model: {model}\n\
             Estimated cost: ${:.4}",
            session.cost_usd
        ))
    } else {
        CommandResult::Message("No active session.".to_string())
    }
}

fn handle_config() -> CommandResult {
    let config_path = dirs_next::home_dir()
        .unwrap_or_default()
        .join(".one")
        .join("config.toml");

    let config_str = std::fs::read_to_string(&config_path)
        .unwrap_or_else(|_| "Config file not found.".to_string());

    CommandResult::Message(format!(
        "Config: {}\n\n```toml\n{}\n```",
        config_path.display(),
        config_str.trim()
    ))
}

async fn handle_effort(args: &str, state: &SharedState) -> CommandResult {
    let arg = args.trim().to_lowercase();

    const VALID_LEVELS: &[&str] = &["low", "medium", "high", "max"];
    const DESCRIPTIONS: &[(&str, &str)] = &[
        ("low", "No extended thinking, 2k output — fast and cheap"),
        ("medium", "20% thinking budget, 8k output — balanced"),
        ("high", "50% thinking budget, 16k output — thorough"),
        (
            "max",
            "80% thinking budget, full output — deepest reasoning (requires thinking-capable model)",
        ),
    ];

    if arg.is_empty() || arg == "current" || arg == "status" {
        let s = state.read().await;
        let level = s
            .active_session()
            .and_then(|s| s.effort.as_deref())
            .unwrap_or("auto");
        return CommandResult::Message(format!("Effort level: {level}"));
    }

    if arg == "help" || arg == "-h" || arg == "--help" {
        let mut help =
            String::from("Usage: /effort [low|medium|high|max|auto]\n\nEffort levels:\n");
        for (level, desc) in DESCRIPTIONS {
            help.push_str(&format!("- **{level}**: {desc}\n"));
        }
        help.push_str(
            "- **auto**: dynamically picks level from message complexity \
             and available context headroom\n\n\
             Thinking budgets are fractions of the model's capability — \
             set automatically for the active model.",
        );
        return CommandResult::Message(help);
    }

    if arg == "auto" || arg == "unset" {
        let mut s = state.write().await;
        if let Some(session) = s.active_session_mut() {
            session.effort = None;
        }
        return CommandResult::Message("Effort level set to auto".to_string());
    }

    if !VALID_LEVELS.contains(&arg.as_str()) {
        return CommandResult::Message(format!(
            "Invalid argument: {arg}. Valid options: low, medium, high, max, auto"
        ));
    }

    // Warn (not block) if max is requested on a model without extended thinking
    if arg == "max" {
        let s = state.read().await;
        let supports = s
            .active_session()
            .map(|s| {
                one_core::provider::model_capabilities(&s.model_config.model).supports_thinking
            })
            .unwrap_or(false);
        if !supports {
            return CommandResult::Message(
                "max effort requires a thinking-capable model (e.g. claude-opus-4-6, \
                 claude-sonnet-4-6). The current model does not support extended thinking — \
                 use **high** instead."
                    .to_string(),
            );
        }
    }

    let desc = DESCRIPTIONS
        .iter()
        .find(|(l, _)| *l == arg)
        .map(|(_, d)| *d)
        .unwrap_or("");

    let mut s = state.write().await;
    if let Some(session) = s.active_session_mut() {
        session.effort = Some(arg.clone());
    }

    CommandResult::Message(format!("Effort → **{arg}**: {desc}"))
}

async fn handle_fast(state: &SharedState) -> CommandResult {
    // Toggle fast mode (uses same model with faster streaming)
    let mut s = state.write().await;
    let fast = s.config.provider.fast_mode.unwrap_or(false);
    s.config.provider.fast_mode = Some(!fast);
    if !fast {
        CommandResult::Message("Fast mode: **ON** (faster output, same model)".to_string())
    } else {
        CommandResult::Message("Fast mode: **OFF** (standard output)".to_string())
    }
}

fn handle_diff() -> CommandResult {
    let output = std::process::Command::new("git")
        .args(["diff", "--stat"])
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let diff = String::from_utf8_lossy(&o.stdout);
            if diff.trim().is_empty() {
                CommandResult::Message("No uncommitted changes.".to_string())
            } else {
                CommandResult::Message(format!("```\n{}\n```", diff.trim()))
            }
        }
        Ok(o) => CommandResult::Message(format!(
            "git diff failed: {}",
            String::from_utf8_lossy(&o.stderr)
        )),
        Err(e) => CommandResult::Message(format!("Failed to run git: {e}")),
    }
}

fn handle_git(args: &str) -> CommandResult {
    let subcmd = args.trim();
    if subcmd.is_empty() {
        return CommandResult::Message(
            "Usage: /git <command>\n\
             Examples: /git status, /git log --oneline -5, /git branch"
                .to_string(),
        );
    }

    let parts: Vec<&str> = subcmd.split_whitespace().collect();
    let output = std::process::Command::new("git").args(&parts).output();

    match output {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let stderr = String::from_utf8_lossy(&o.stderr);
            let combined = if stderr.is_empty() {
                stdout.to_string()
            } else {
                format!("{stdout}\n{stderr}")
            };
            CommandResult::Message(format!("```\n{}\n```", combined.trim()))
        }
        Err(e) => CommandResult::Message(format!("Failed to run git: {e}")),
    }
}

async fn handle_doctor() -> CommandResult {
    let mut checks = Vec::new();

    // Check git
    let git = std::process::Command::new("git").arg("--version").output();
    checks.push(match git {
        Ok(o) if o.status.success() => {
            format!("  git: {} ✓", String::from_utf8_lossy(&o.stdout).trim())
        }
        _ => "  git: not found ✗".to_string(),
    });

    // Check claude
    let claude = std::process::Command::new("claude")
        .arg("--version")
        .output();
    checks.push(match claude {
        Ok(o) if o.status.success() => {
            format!("  claude: {} ✓", String::from_utf8_lossy(&o.stdout).trim())
        }
        _ => "  claude: not found ✗ (needed for Sonnet/Opus fallback)".to_string(),
    });

    // Check ripgrep (required by grep + glob tools)
    let rg = std::process::Command::new("rg").arg("--version").output();
    checks.push(match rg {
        Ok(o) if o.status.success() => format!(
            "  rg: {} ✓",
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .next()
                .unwrap_or("unknown")
        ),
        _ => "  rg: not found ✗ (needed for grep/glob tools)".to_string(),
    });

    // Check gh CLI (needed for /pr)
    let gh = std::process::Command::new("gh").arg("--version").output();
    checks.push(match gh {
        Ok(o) if o.status.success() => format!(
            "  gh: {} ✓",
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .next()
                .unwrap_or("unknown")
        ),
        _ => "  gh: not found ─ (needed for /pr)".to_string(),
    });

    // Check credentials
    let has_key = one_core::credentials::CredentialStore::get("anthropic")
        .ok()
        .flatten()
        .is_some();
    checks.push(if has_key {
        "  anthropic key: found ✓".to_string()
    } else {
        "  anthropic key: not found ✗".to_string()
    });

    // Check config
    let config_path = dirs_next::home_dir()
        .unwrap_or_default()
        .join(".one")
        .join("config.toml");
    checks.push(if config_path.exists() {
        format!("  config: {} ✓", config_path.display())
    } else {
        "  config: not found ✗".to_string()
    });

    // Check HuggingFace credentials
    let has_hf = one_core::credentials::CredentialStore::get("huggingface")
        .ok()
        .flatten()
        .is_some()
        || std::env::var("HF_TOKEN").is_ok();
    checks.push(if has_hf {
        "  huggingface: credentials found ✓".to_string()
    } else {
        "  huggingface: not logged in ✗ (/login)".to_string()
    });

    // Check OpenAI credentials
    let has_openai = one_core::credentials::CredentialStore::get("openai")
        .ok()
        .flatten()
        .is_some()
        || std::env::var("OPENAI_API_KEY").is_ok();
    checks.push(if has_openai {
        "  openai: credentials found ✓".to_string()
    } else {
        "  openai: not configured ─".to_string()
    });

    // Check settings.json
    let settings_path = one_core::settings::global_settings_path();
    checks.push(if settings_path.exists() {
        "  settings.json: found ✓".to_string()
    } else {
        "  settings.json: not found ─ (using defaults)".to_string()
    });

    // Check MCP config
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let mcp_configs = one_core::mcp::config::load_mcp_configs(&cwd);
    checks.push(if mcp_configs.is_empty() {
        "  mcp servers: none configured ─".to_string()
    } else {
        format!("  mcp servers: {} configured ✓", mcp_configs.len())
    });

    // Check memory
    let store = one_core::memory::MemoryStore::for_project(&cwd);
    let memories = store.load_all();
    let global_store = one_core::memory::MemoryStore::global();
    let global_memories = global_store.load_all();
    let total = memories.len() + global_memories.len();
    checks.push(if total > 0 {
        format!("  memories: {total} saved ✓")
    } else {
        "  memories: none ─".to_string()
    });

    // Check ripgrep
    let rg = std::process::Command::new("rg").arg("--version").output();
    checks.push(match rg {
        Ok(o) if o.status.success() => {
            let ver = String::from_utf8_lossy(&o.stdout);
            let first = ver.lines().next().unwrap_or("rg");
            format!("  ripgrep: {first} ✓")
        }
        _ => "  ripgrep: not found ✗ (grep tool will fall back to grep)".to_string(),
    });

    CommandResult::Message(format!("## Doctor\n\n{}", checks.join("\n")))
}

async fn handle_debug(state: &SharedState) -> CommandResult {
    let mut s = state.write().await;
    s.debug_mode = !s.debug_mode;
    if s.debug_mode {
        CommandResult::Message(
            "Debug mode: **ON** — background activity will appear as muted lines in chat.\n\
             Use /debug again to hide."
                .to_string(),
        )
    } else {
        CommandResult::Message("Debug mode: **OFF**".to_string())
    }
}

async fn handle_plan(state: &SharedState) -> CommandResult {
    let mut s = state.write().await;
    let is_plan = s.plan_mode;
    s.plan_mode = !is_plan;

    if !is_plan {
        CommandResult::Message(
            "Plan mode: **ON** — tools will be described but not executed.\n\
             The AI will explain what it would do without making changes.\n\
             Use /plan again to return to normal mode."
                .to_string(),
        )
    } else {
        CommandResult::Message("Plan mode: **OFF** — tools will execute normally.".to_string())
    }
}

fn handle_permissions() -> CommandResult {
    let global_path = one_core::settings::global_settings_path();
    let global_exists = global_path.exists();

    let mut lines = vec!["## Permission Settings".to_string(), String::new()];

    if global_exists {
        if let Ok(content) = std::fs::read_to_string(&global_path)
            && let Ok(settings) = serde_json::from_str::<one_core::settings::Settings>(&content)
        {
            let mode = settings
                .permissions
                .default_mode
                .map(|m| format!("{m:?}"))
                .unwrap_or_else(|| "Default".to_string());
            lines.push(format!("Mode: {mode}"));

            if !settings.permissions.allow.is_empty() {
                lines.push(format!("Allow: {}", settings.permissions.allow.join(", ")));
            }
            if !settings.permissions.deny.is_empty() {
                lines.push(format!("Deny: {}", settings.permissions.deny.join(", ")));
            }
            if !settings.permissions.ask.is_empty() {
                lines.push(format!("Ask: {}", settings.permissions.ask.join(", ")));
            }
        }
    } else {
        lines.push("No settings.json found.".to_string());
    }

    lines.push(String::new());
    lines.push(format!("Global: {}", global_path.display()));
    lines.push(String::new());
    lines.push("Example settings.json:".to_string());
    lines.push("```json".to_string());
    lines.push(
        r#"{
  "permissions": {
    "allow": ["file_read", "grep", "glob", "bash(git:*)"],
    "deny": ["bash(rm -rf)"],
    "defaultMode": "default"
  }
}"#
        .to_string(),
    );
    lines.push("```".to_string());

    CommandResult::Message(lines.join("\n"))
}

fn handle_mcp() -> CommandResult {
    // Load MCP config to show what's configured
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".to_string());

    let configs = one_core::mcp::config::load_mcp_configs(&cwd);

    if configs.is_empty() {
        return CommandResult::Message(
            "No MCP servers configured.\n\n\
             Add a `.mcp.json` in your project root:\n\
             ```json\n\
             {\n  \
               \"mcpServers\": {\n    \
                 \"filesystem\": {\n      \
                   \"command\": \"npx\",\n      \
                   \"args\": [\"-y\", \"@modelcontextprotocol/server-filesystem\", \"/tmp\"]\n    \
                 }\n  \
               }\n\
             }\n\
             ```"
            .to_string(),
        );
    }

    let mut lines = vec![
        format!("{} MCP servers configured:", configs.len()),
        String::new(),
    ];

    for (name, config) in &configs {
        match config {
            one_core::mcp::config::McpServerConfig::Stdio { command, args, .. } => {
                lines.push(format!("  {name}: stdio — {command} {}", args.join(" ")));
            }
            one_core::mcp::config::McpServerConfig::Remote {
                transport_type,
                url,
                ..
            } => {
                lines.push(format!("  {name}: {transport_type} — {url}"));
            }
        }
    }

    CommandResult::Message(lines.join("\n"))
}

async fn handle_memory(args: &str, state: &SharedState) -> CommandResult {
    let project_dir = {
        let s = state.read().await;
        s.active_session()
            .map(|s| s.project_path.clone())
            .unwrap_or_else(|| ".".to_string())
    };

    let store = one_core::memory::MemoryStore::for_project(&project_dir);
    let global = one_core::memory::MemoryStore::global();

    let parts: Vec<&str> = args.splitn(2, ' ').collect();
    let subcmd = parts.first().copied().unwrap_or("");

    match subcmd {
        "" | "list" => {
            let project_memories = store.load_all();
            let global_memories = global.load_all();

            if project_memories.is_empty() && global_memories.is_empty() {
                return CommandResult::Message(
                    "No memories saved.\n\n\
                     Save one with: /remember <text>\n\
                     The AI can also save memories during conversation."
                        .to_string(),
                );
            }

            let mut lines = Vec::new();

            if !global_memories.is_empty() {
                lines.push(format!("**Global** ({} memories):", global_memories.len()));
                for m in &global_memories {
                    lines.push(format!(
                        "  [{}] {} — {}",
                        m.memory_type, m.name, m.description
                    ));
                }
                lines.push(String::new());
            }

            if !project_memories.is_empty() {
                lines.push(format!(
                    "**Project** ({} memories):",
                    project_memories.len()
                ));
                for m in &project_memories {
                    lines.push(format!(
                        "  [{}] {} — {}",
                        m.memory_type, m.name, m.description
                    ));
                }
            }

            CommandResult::Message(lines.join("\n"))
        }
        "search" => {
            let query = parts.get(1).unwrap_or(&"").trim();
            if query.is_empty() {
                return CommandResult::Message("Usage: /memory search <query>".to_string());
            }

            let mut results = store.find(query);
            results.extend(global.find(query));

            if results.is_empty() {
                CommandResult::Message(format!("No memories matching \"{query}\""))
            } else {
                let mut lines = vec![format!("{} memories found:", results.len())];
                for m in &results {
                    lines.push(format!(
                        "  [{}] {} — {}",
                        m.memory_type, m.name, m.description
                    ));
                    // Show first line of content
                    if let Some(first_line) = m.content.lines().next() {
                        let preview = if first_line.len() > 80 {
                            format!("{}...", &first_line[..80])
                        } else {
                            first_line.to_string()
                        };
                        lines.push(format!("    {preview}"));
                    }
                }
                CommandResult::Message(lines.join("\n"))
            }
        }
        "delete" => {
            let name = parts.get(1).unwrap_or(&"").trim();
            if name.is_empty() {
                return CommandResult::Message("Usage: /memory delete <name>".to_string());
            }

            let deleted_project = store.delete(name).unwrap_or(false);
            let deleted_global = global.delete(name).unwrap_or(false);

            if deleted_project || deleted_global {
                CommandResult::Message(format!("Deleted memory matching \"{name}\""))
            } else {
                CommandResult::Message(format!("No memory found matching \"{name}\""))
            }
        }
        _ => CommandResult::Message(
            "Usage:\n\
             /memory              List all memories\n\
             /memory search <q>   Search memories\n\
             /memory delete <n>   Delete by name\n\
             /remember <text>     Save a quick memory"
                .to_string(),
        ),
    }
}

async fn handle_remember(args: &str, state: &SharedState) -> CommandResult {
    let text = args.trim();
    if text.is_empty() {
        return CommandResult::Message(
            "Usage: /remember <text>\n\
             Saves a project memory that persists across sessions.\n\n\
             Examples:\n\
             /remember uses pnpm, not npm\n\
             /remember API keys are in .env.local\n\
             /remember deploy via `fly deploy`"
                .to_string(),
        );
    }

    let project_dir = {
        let s = state.read().await;
        s.active_session()
            .map(|s| s.project_path.clone())
            .unwrap_or_else(|| ".".to_string())
    };

    let store = one_core::memory::MemoryStore::for_project(&project_dir);

    // Generate a name from the first few words
    let name: String = text
        .split_whitespace()
        .take(5)
        .collect::<Vec<_>>()
        .join(" ");

    let memory = one_core::memory::Memory {
        name: name.clone(),
        description: text.to_string(),
        memory_type: one_core::memory::MemoryType::Project,
        content: text.to_string(),
        file_path: std::path::PathBuf::new(),
    };

    match store.save(&memory) {
        Ok(_) => CommandResult::Message(format!("Remembered: \"{name}\"")),
        Err(e) => CommandResult::Message(format!("Failed to save memory: {e}")),
    }
}

async fn handle_compact(state: &SharedState) -> CommandResult {
    let s = state.read().await;
    if let Some(session) = s.active_session() {
        let turn_count = session.conversation.turns.len();
        if turn_count <= 2 {
            return CommandResult::Message(
                "Nothing to compact — conversation is already short.".to_string(),
            );
        }
        drop(s);

        // Use AI-powered summarization via the compaction prompt.
        // The AI will analyze the full conversation and produce a structured summary
        // matching CC's exact format (analysis + summary with 9 sections).
        CommandResult::SendToAi(one_core::compact::prompt::get_compact_prompt(None))
    } else {
        CommandResult::Message("No active session.".to_string())
    }
}

async fn handle_tasks(args: &str, state: &SharedState) -> CommandResult {
    let parts: Vec<&str> = args.splitn(2, ' ').collect();
    let subcmd = parts.first().copied().unwrap_or("");

    match subcmd {
        "" | "list" => {
            let s = state.read().await;
            CommandResult::Message(s.tasks.summary())
        }
        "add" | "create" => {
            let desc = parts.get(1).unwrap_or(&"").trim();
            if desc.is_empty() {
                return CommandResult::Message("Usage: /tasks add <description>".to_string());
            }
            let mut s = state.write().await;
            let id = s.tasks.create(desc);
            CommandResult::Message(format!("Created {id}: {desc}"))
        }
        "done" | "complete" => {
            let id = parts.get(1).unwrap_or(&"").trim();
            if id.is_empty() {
                return CommandResult::Message("Usage: /tasks done <task_id>".to_string());
            }
            let mut s = state.write().await;
            if s.tasks
                .update_status(id, one_core::tasks::TaskStatus::Completed)
            {
                CommandResult::Message(format!("Completed: {id}"))
            } else {
                CommandResult::Message(format!("Task not found: {id}"))
            }
        }
        "start" => {
            let id = parts.get(1).unwrap_or(&"").trim();
            if id.is_empty() {
                return CommandResult::Message("Usage: /tasks start <task_id>".to_string());
            }
            let mut s = state.write().await;
            if s.tasks
                .update_status(id, one_core::tasks::TaskStatus::InProgress)
            {
                CommandResult::Message(format!("Started: {id}"))
            } else {
                CommandResult::Message(format!("Task not found: {id}"))
            }
        }
        "cancel" => {
            let id = parts.get(1).unwrap_or(&"").trim();
            if id.is_empty() {
                return CommandResult::Message("Usage: /tasks cancel <task_id>".to_string());
            }
            let mut s = state.write().await;
            if s.tasks
                .update_status(id, one_core::tasks::TaskStatus::Cancelled)
            {
                CommandResult::Message(format!("Cancelled: {id}"))
            } else {
                CommandResult::Message(format!("Task not found: {id}"))
            }
        }
        _ => CommandResult::Message(
            "Usage:\n\
             /tasks               List all tasks\n\
             /tasks add <desc>    Create a new task\n\
             /tasks start <id>    Mark as in progress\n\
             /tasks done <id>     Mark as completed\n\
             /tasks cancel <id>   Cancel a task"
                .to_string(),
        ),
    }
}

fn handle_all_sessions() -> CommandResult {
    let mut lines = vec!["Previous sessions (all backends):\n".to_string()];

    // Imported sessions (JSONL format)
    if let Ok(sessions) = one_core::storage::list_claude_code_sessions()
        && !sessions.is_empty()
    {
        lines.push(format!("  Imported ({} sessions):", sessions.len()));
        for s in sessions.iter().take(10) {
            let date = if s.timestamp.is_empty() {
                "unknown".to_string()
            } else {
                s.timestamp[..10].to_string()
            };
            lines.push(format!(
                "    {} | {} turns | {}",
                date, s.turns, s.project_path
            ));
            lines.push(format!("    \"{}\"", s.first_message));
            lines.push(format!("    one --session {}", s.session_id));
            lines.push(String::new());
        }
    }

    // Codex sessions
    if let Ok(sessions) = one_core::storage::list_codex_sessions()
        && !sessions.is_empty()
    {
        lines.push(format!("  Codex ({} sessions):", sessions.len()));
        for s in sessions.iter().take(10) {
            let date = if s.timestamp.len() >= 10 {
                &s.timestamp[..10]
            } else {
                &s.timestamp
            };
            lines.push(format!(
                "    {} | {} | {}",
                date, s.project_path, s.first_message
            ));
            lines.push(format!("    one --session {}", s.session_id));
            lines.push(String::new());
        }
    }

    if lines.len() == 1 {
        lines.push("  No previous sessions found.".to_string());
        lines.push(String::new());
        lines.push("  Searches: ~/.claude/projects/ and ~/.codex/".to_string());
    }

    lines.push(String::new());
    lines.push("Resume any session with: one --session <id>".to_string());

    CommandResult::Message(lines.join("\n"))
}

fn handle_tools() -> String {
    let mut lines = vec!["## Tools (14 built-in)\n".to_string()];

    lines.push("**Active** (9 — sent in every request):".to_string());
    lines.push("  Read         Read files [read-only]".to_string());
    lines.push("  Write        Write/create files".to_string());
    lines.push("  Edit         Find-and-replace in files".to_string());
    lines.push("  Bash         Execute shell commands [destructive]".to_string());
    lines.push("  Grep         Search file contents [read-only]".to_string());
    lines.push("  Glob         Search file names [read-only]".to_string());
    lines.push("  Agent        Spawn sub-agent for focused tasks".to_string());
    lines.push("  ask_user     Ask the user a question [read-only]".to_string());
    lines.push("  tool_search  Load deferred tool schemas [read-only]".to_string());

    lines.push(String::new());
    lines.push("**Deferred** (5 — loaded via tool_search):".to_string());
    lines.push("  web_fetch       Fetch URL / web page content".to_string());
    lines.push("  web_search      Search the web via DuckDuckGo".to_string());
    lines.push("  sleep           Wait for N seconds (1-300)".to_string());
    lines.push("  enter_plan_mode Enter plan mode (describe without executing)".to_string());
    lines.push("  exit_plan_mode  Exit plan mode and present plan".to_string());

    // MCP tools
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".to_string());
    let configs = one_core::mcp::config::load_mcp_configs(&cwd);
    if !configs.is_empty() {
        lines.push(String::new());
        lines.push(format!("**MCP** ({} servers configured):", configs.len()));
        for name in configs.keys() {
            lines.push(format!("  mcp__{name}__* (connect to discover tools)"));
        }
    }

    lines.join("\n")
}

async fn handle_skills(args: &str, state: &SharedState) -> CommandResult {
    let parts: Vec<&str> = args.splitn(2, ' ').collect();
    let subcmd = parts.first().copied().unwrap_or("");

    let project_dir = {
        let s = state.read().await;
        s.active_session()
            .map(|s| s.project_path.clone())
            .unwrap_or_else(|| ".".to_string())
    };

    match subcmd {
        "install" => {
            let url = parts.get(1).unwrap_or(&"").trim();
            if url.is_empty() {
                return CommandResult::Message(
                    "Usage: /skills install <url>\n\
                     Downloads a .md skill file and saves to ~/.one/commands/\n\n\
                     Example: /skills install https://raw.githubusercontent.com/user/repo/main/review.md"
                        .to_string(),
                );
            }

            // Extract filename from URL
            let filename = url.rsplit('/').next().unwrap_or("skill.md");
            let filename = if filename.ends_with(".md") {
                filename.to_string()
            } else {
                format!("{filename}.md")
            };

            // Save to ~/.one/commands/
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            let commands_dir = std::path::PathBuf::from(&home)
                .join(".one")
                .join("commands");
            let _ = std::fs::create_dir_all(&commands_dir);
            let dest = commands_dir.join(&filename);

            // Download via curl
            let output = tokio::process::Command::new("curl")
                .args(["-fsSL", "-o", &dest.to_string_lossy(), url])
                .output()
                .await;

            match output {
                Ok(o) if o.status.success() => CommandResult::Message(format!(
                    "Installed skill: /{}\nSaved to: {}",
                    filename.trim_end_matches(".md"),
                    dest.display()
                )),
                Ok(o) => {
                    let stderr = String::from_utf8_lossy(&o.stderr);
                    CommandResult::Message(format!("Download failed: {stderr}"))
                }
                Err(e) => CommandResult::Message(format!("Failed to run curl: {e}")),
            }
        }
        "uninstall" | "remove" => {
            let name = parts.get(1).unwrap_or(&"").trim();
            if name.is_empty() {
                return CommandResult::Message("Usage: /skills uninstall <name>".to_string());
            }

            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            let commands_dir = std::path::PathBuf::from(&home)
                .join(".one")
                .join("commands");
            let path = commands_dir.join(format!("{name}.md"));

            if path.exists() {
                match std::fs::remove_file(&path) {
                    Ok(()) => CommandResult::Message(format!("Uninstalled skill: /{name}")),
                    Err(e) => CommandResult::Message(format!("Failed to remove: {e}")),
                }
            } else {
                CommandResult::Message(format!("Skill not found: /{name}"))
            }
        }
        "" | "list" => {
            let skills = one_core::skills::load_skills(&project_dir);

            if skills.is_empty() {
                return CommandResult::Message(
                    "No custom skills installed.\n\n\
                     Install from URL: /skills install <url>\n\
                     Create manually: add .md files to ~/.one/commands/\n\n\
                     Example: ~/.one/commands/review.md\n\
                     ```text\n\
                     ---\n\
                     description: Review the current changes\n\
                     ---\n\
                     Review all changes in this PR for bugs, security issues, and style.\n\
                     ```"
                    .to_string(),
                );
            }

            let mut lines = vec![format!("## Custom Skills ({} installed)\n", skills.len())];
            for skill in &skills {
                lines.push(format!("  /{:<16} {}", skill.name, skill.description));
                lines.push(format!("    Source: {}", skill.source.display()));
            }
            lines.push(String::new());
            lines.push("Install: /skills install <url>".to_string());
            lines.push("Remove:  /skills uninstall <name>".to_string());

            CommandResult::Message(lines.join("\n"))
        }
        _ => CommandResult::Message(
            "Usage:\n  /skills              List installed skills\n  \
             /skills install <url>  Install skill from URL\n  \
             /skills uninstall <n>  Remove a skill"
                .to_string(),
        ),
    }
}

async fn handle_settings(state: &SharedState) -> CommandResult {
    let s = state.read().await;
    let config = &s.config;

    let mut lines = vec!["## Current Settings\n".to_string()];

    // Provider
    lines.push(format!(
        "**Provider:** {}",
        if config.provider.default_provider.is_empty() {
            "auto-detect"
        } else {
            &config.provider.default_provider
        }
    ));
    lines.push(format!(
        "**Model:** {}",
        if config.provider.default_model.is_empty() {
            "auto"
        } else {
            &config.provider.default_model
        }
    ));
    lines.push(format!("**Max tokens:** {}", config.provider.max_tokens));
    lines.push(format!(
        "**Fast mode:** {}",
        config.provider.fast_mode.unwrap_or(false)
    ));

    // UI
    lines.push(format!("\n**Theme:** {}", config.ui.theme));
    lines.push(format!("**Line numbers:** {}", config.ui.line_numbers));

    // Pet
    lines.push(format!(
        "\n**Pet:** {} the {} ({})",
        config.pet.name,
        config.pet.species,
        if config.pet.enabled {
            "enabled"
        } else {
            "disabled"
        }
    ));

    // Sessions
    lines.push(format!("\n**Active sessions:** {}", s.sessions.len()));
    lines.push(format!("**Plan mode:** {}", s.plan_mode));

    // Notifications
    lines.push(format!("**Notifications:** {}", s.notifications.len()));

    // Paths
    lines.push("\n**Config paths:**".to_string());
    lines.push("  Config: ~/.one/config.toml".to_string());
    lines.push("  Settings: ~/.one/settings.json".to_string());
    lines.push("  Keybindings: ~/.one/keybindings.json".to_string());
    lines.push("  Commands: ~/.one/commands/".to_string());
    lines.push("  Plugins: ~/.one/plugins/".to_string());

    CommandResult::Message(lines.join("\n"))
}

async fn handle_pr(state: &SharedState) -> CommandResult {
    let project_dir = {
        let s = state.read().await;
        s.active_session()
            .map(|s| s.project_path.clone())
            .unwrap_or_else(|| ".".to_string())
    };

    // Embed real-time git context (mirrors CC's commit-push-pr skill)
    let prompt_template = "\
## Context

- Current git status: !`git status`
- Current git diff (staged and unstaged changes): !`git diff HEAD`
- Current branch: !`git branch --show-current`
- Recent commits: !`git log --oneline -10`

## Your task

Based on the above changes, create a pull request:

1. Create a new branch if on main
2. Create a single commit with an appropriate message
3. Push the branch to origin
4. Create a pull request using `gh pr create` with a concise title (<70 chars) and body with Summary + Test plan
5. Return the PR URL

Do NOT push to remote unless necessary for the PR. Stage and commit in a single message.";

    let prompt = one_core::skills::interpolate_commands(prompt_template, &project_dir);
    CommandResult::SendToAi(prompt)
}

async fn handle_commit(state: &SharedState) -> CommandResult {
    let project_dir = {
        let s = state.read().await;
        s.active_session()
            .map(|s| s.project_path.clone())
            .unwrap_or_else(|| ".".to_string())
    };

    // Embed real-time git context (mirrors CC's commit skill)
    let prompt_template = "\
## Context

- Current git status: !`git status`
- Current git diff (staged and unstaged changes): !`git diff HEAD`
- Current branch: !`git branch --show-current`
- Recent commits: !`git log --oneline -10`

## Your task

Based on the above changes, create a single git commit.

Stage and create the commit using a single message. Do NOT push unless explicitly asked. \
Follow the repo's existing commit message style based on the recent commits above.";

    let prompt = one_core::skills::interpolate_commands(prompt_template, &project_dir);
    CommandResult::SendToAi(prompt)
}
