mod tasks;

use std::sync::Arc;

use anyhow::Result;
use clap::{CommandFactory, Parser};
use tracing_subscriber::EnvFilter;

use one_core::config::AppConfig;
use one_core::event::{Event, EventBus};
use one_core::provider::{ModelConfig, Provider};
use one_core::query_engine::{QueryEngine, ToolExecResult};
use one_core::session::Session;
use one_core::state::new_shared_state;
use one_integrations::Integration;
use one_tui::app::App;

#[derive(Parser)]
#[command(name = "one", about = "One — multi-project AI coding terminal")]
struct Cli {
    /// Project directory to open (can be specified multiple times)
    #[arg(long)]
    project: Vec<String>,

    /// AI provider to use (overrides config file)
    #[arg(long)]
    provider: Option<String>,

    /// Model to use (overrides config file)
    #[arg(long)]
    model: Option<String>,

    /// Max tokens for AI responses (overrides config file)
    #[arg(long)]
    max_tokens: Option<u32>,

    /// Resume the last session for each project
    #[arg(long, short = 'c')]
    continue_session: bool,

    /// Resume a specific session by ID
    #[arg(long)]
    session: Option<String>,

    /// Generate shell completions and exit
    #[arg(long, value_enum)]
    completions: Option<clap_complete::Shell>,

    /// Print version and exit
    #[arg(short = 'V', long)]
    version: bool,

    /// Skip all permission prompts (use with caution)
    #[arg(long)]
    dangerously_skip_permissions: bool,

    /// Allow specific tools without prompting (can be repeated)
    #[arg(long = "allowedTools")]
    allowed_tools: Vec<String>,

    /// Print-only mode: suppress tool call display, output only final text.
    /// Useful for piping: one -p 'prompt' | other_command
    #[arg(short = 'p', long = "print")]
    print_only: bool,

    /// Output format for one-shot mode: "text" (default) or "json"
    #[arg(long = "output-format", default_value = "text")]
    output_format: String,

    /// Override the system prompt entirely
    #[arg(long = "system-prompt")]
    system_prompt: Option<String>,

    /// Append text to the default system prompt
    #[arg(long = "append-system-prompt")]
    append_system_prompt: Option<String>,

    /// Max turns for the tool execution loop (default: 200)
    #[arg(long = "max-turns")]
    max_turns: Option<usize>,

    /// Disable all tools (text-only mode)
    #[arg(long = "no-tools")]
    no_tools: bool,

    /// Enable verbose output (debug logging, token counts, timing)
    #[arg(long, short = 'v')]
    verbose: bool,

    /// One-shot prompt: send a message and print the response (no TUI)
    /// Usage: one 'say hi'
    #[arg(trailing_var_arg = true)]
    prompt: Vec<String>,
}

/// Auto-detect the best provider based on available credentials.
/// Checks keychain, env vars, and config file credentials.
/// Falls back to Ollama (no auth needed) if nothing else is available.
fn auto_detect_provider(config: &AppConfig) -> (String, Provider) {
    // Check Anthropic (most capable — prefer if available)
    let anthropic_key = one_core::credentials::CredentialStore::resolve(
        "anthropic",
        {
            let k = config.api_key_for("anthropic");
            if k.is_empty() { None } else { Some(k) }
        }
        .as_deref(),
        "ANTHROPIC_API_KEY",
    );
    if !anthropic_key.is_empty() {
        tracing::info!("Auto-detected provider: anthropic (credentials found)");
        return ("anthropic".to_string(), Provider::Anthropic);
    }

    // Check OpenAI
    let openai_key = one_core::credentials::CredentialStore::resolve(
        "openai",
        {
            let k = config.api_key_for("openai");
            if k.is_empty() { None } else { Some(k) }
        }
        .as_deref(),
        "OPENAI_API_KEY",
    );
    if !openai_key.is_empty() {
        tracing::info!("Auto-detected provider: openai (credentials found)");
        return ("openai".to_string(), Provider::OpenAI);
    }

    // Check Google
    if std::env::var("GOOGLE_API_KEY").is_ok() {
        tracing::info!("Auto-detected provider: google (env var found)");
        return ("google".to_string(), Provider::Google);
    }

    // Check Hugging Face
    let hf_key = one_core::credentials::CredentialStore::resolve(
        "huggingface",
        {
            let k = config.api_key_for("huggingface");
            if k.is_empty() { None } else { Some(k) }
        }
        .as_deref(),
        "HF_TOKEN",
    );
    if !hf_key.is_empty() {
        tracing::info!("Auto-detected provider: huggingface (credentials found)");
        return ("huggingface".to_string(), Provider::HuggingFace);
    }

    // Check LM Studio (localhost:1234 — no auth needed, just check if it's running)
    if std::net::TcpStream::connect_timeout(
        &"127.0.0.1:1234".parse().unwrap(),
        std::time::Duration::from_millis(200),
    )
    .is_ok()
    {
        tracing::info!("Auto-detected provider: lmstudio (localhost:1234 reachable)");
        return ("lmstudio".to_string(), Provider::LmStudio);
    }

    // Fallback to Ollama (local, no auth needed)
    tracing::info!("No provider credentials found, defaulting to ollama (local)");
    ("ollama".to_string(), Provider::Ollama)
}

fn default_model_for(provider: Provider) -> String {
    match provider {
        Provider::Anthropic => "claude-sonnet-4-20250514".to_string(),
        Provider::OpenAI => "gpt-4o".to_string(),
        Provider::Ollama => "llama3".to_string(),
        Provider::Google => "gemini-2.0-flash".to_string(),
        Provider::HuggingFace => "meta-llama/Llama-3.1-8B-Instruct".to_string(),
        Provider::LmStudio => "default".to_string(),
    }
}

fn parse_provider(s: &str) -> Result<Provider> {
    match s.to_lowercase().as_str() {
        "anthropic" => Ok(Provider::Anthropic),
        "openai" => Ok(Provider::OpenAI),
        "ollama" => Ok(Provider::Ollama),
        "google" => Ok(Provider::Google),
        "huggingface" | "hf" => Ok(Provider::HuggingFace),
        "lmstudio" | "lm-studio" => Ok(Provider::LmStudio),
        other => anyhow::bail!("Unknown provider: {other}"),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Init logging: --verbose enables debug, RUST_LOG overrides, otherwise off
    let default_filter = if cli.verbose { "debug" } else { "off" };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter)),
        )
        .with_writer(std::io::stderr)
        .init();

    // Handle early-exit flags
    if cli.version {
        println!("one {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    if let Some(shell) = cli.completions {
        clap_complete::generate(shell, &mut Cli::command(), "one", &mut std::io::stdout());
        return Ok(());
    }

    // Run first-time onboarding if needed (before loading config for provider)
    if one_core::onboarding::check_onboarding() == one_core::onboarding::OnboardingState::Needed {
        let mut config = AppConfig::load()?;
        one_tui::onboarding::run_onboarding(&mut config).await?;
    }

    // Load config — fresh after onboarding may have changed default_provider
    let config = AppConfig::load()?;

    // Resolve provider and model.
    //
    // Resolution priority:
    //   1. --provider flag (explicit)
    //   2. --model flag → infer provider from model name (e.g. claude-* → Anthropic)
    //   3. Config file default_provider + default_model
    //   4. Auto-detect from available credentials
    //
    // This means `one --model claude-sonnet-4-20250514 'say hi'` works in CI
    // without needing --provider anthropic.
    use one_core::provider::{infer_provider, resolve_model_shortcut};

    let (provider_str, provider, model) = if let Some(ref p) = cli.provider {
        // Explicit --provider
        let provider = parse_provider(p)?;
        let model = cli
            .model
            .as_deref()
            .map(|m| resolve_model_shortcut(m).to_string())
            .or_else(|| {
                if !config.provider.default_model.is_empty() {
                    Some(config.provider.default_model.clone())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| default_model_for(provider));
        (p.clone(), provider, model)
    } else if let Some(ref m) = cli.model {
        // --model provided without --provider → infer provider from model name
        let resolved = resolve_model_shortcut(m).to_string();
        if let Some(provider) = infer_provider(&resolved) {
            let name = match provider {
                Provider::Anthropic => "anthropic",
                Provider::OpenAI => "openai",
                Provider::Ollama => "ollama",
                Provider::Google => "google",
                Provider::HuggingFace => "huggingface",
                Provider::LmStudio => "lmstudio",
            };
            (name.to_string(), provider, resolved)
        } else {
            // Can't infer → fall back to config or auto-detect
            let (pstr, prov) = if !config.provider.default_provider.is_empty() {
                let p = config.provider.default_provider.to_lowercase();
                (p.clone(), parse_provider(&p)?)
            } else {
                auto_detect_provider(&config)
            };
            (pstr, prov, resolved)
        }
    } else if !config.provider.default_provider.is_empty() {
        // Config has a default provider
        let p = config.provider.default_provider.to_lowercase();
        let provider = parse_provider(&p)?;
        let model = if !config.provider.default_model.is_empty() {
            config.provider.default_model.clone()
        } else {
            default_model_for(provider)
        };
        (p, provider, model)
    } else {
        // No flags, no config → auto-detect from credentials
        let (pstr, prov) = auto_detect_provider(&config);
        let model = default_model_for(prov);
        (pstr, prov, model)
    };

    let max_tokens = cli.max_tokens.unwrap_or(config.provider.max_tokens);

    let model_config = ModelConfig {
        provider,
        model: model.to_string(),
        max_tokens,
        temperature: None,
        budget_tokens: None,
    };

    // Resolve API key: keyring (incl. OAuth tokens) → config file → env var
    // Ollama needs no key — skip resolution entirely
    let api_key = if provider == Provider::Ollama || provider == Provider::LmStudio {
        String::new() // Local providers don't need auth
    } else {
        let env_var = match provider {
            Provider::Anthropic => "ANTHROPIC_API_KEY",
            Provider::OpenAI => "OPENAI_API_KEY",
            Provider::Google => "GOOGLE_API_KEY",
            Provider::HuggingFace => "HF_TOKEN",
            Provider::Ollama | Provider::LmStudio => unreachable!(),
        };
        let config_key = config.api_key_for(&provider_str);
        one_core::credentials::CredentialStore::resolve(
            &provider_str,
            if config_key.is_empty() {
                None
            } else {
                Some(&config_key)
            },
            env_var,
        )
    };

    // One-shot mode: `one 'say hi'` — send prompt, print response, exit
    // Build prompt from CLI args or stdin (supports `echo "prompt" | one`)
    let mut prompt_text = cli.prompt.join(" ");
    if prompt_text.is_empty() {
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() {
            // Read from piped stdin
            use std::io::Read;
            let mut stdin_content = String::new();
            if std::io::stdin().read_to_string(&mut stdin_content).is_ok() {
                prompt_text = stdin_content.trim().to_string();
            }
        }
    }
    if !prompt_text.is_empty() {
        // Expand @file references in the prompt
        let cwd_for_expand = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".to_string());
        prompt_text = one_core::skills::expand_at_mentions(&prompt_text, &cwd_for_expand);

        // Build tool registry so we can include schemas in the request
        let tool_registry = Arc::new(one_tools::create_default_registry());
        // Send only active (non-deferred) schemas to the API to save tokens.
        // Deferred tools are loaded on-demand via tool_search.
        // --no-tools disables all tools for text-only mode.
        let mut tool_schemas = if cli.no_tools {
            Vec::new()
        } else {
            tool_registry.active_schemas()
        };

        // Initialize MCP servers and merge their tools
        let cwd = std::env::current_dir()?.to_string_lossy().to_string();
        let mut mcp_manager = one_core::mcp::client::McpManager::new();
        if !cli.no_tools {
            mcp_manager.connect_all(&cwd).await;
            tool_schemas.extend(mcp_manager.tool_schemas());
        }

        // Load permissions (CLI flags override settings)
        let mut permission_engine = one_core::settings::load_permission_engine(&cwd);
        if cli.dangerously_skip_permissions {
            permission_engine.mode = one_core::permissions::PermissionMode::BypassPermissions;
        }
        for tool_name in &cli.allowed_tools {
            permission_engine.add_session_rule(
                tool_name,
                None,
                one_core::permissions::PermissionBehavior::Allow,
            );
        }

        // All providers get tool schemas — each formats them natively
        let ai_provider = one_ai::create_provider_with_tools(provider, api_key, tool_schemas);

        if !ai_provider.is_configured() {
            eprintln!(
                "Error: {} is not configured. Run `one` to set up credentials.",
                ai_provider.provider_name()
            );
            std::process::exit(1);
        }

        // Build system prompt (CLI flags can override or extend)
        let system_prompt = if let Some(ref custom) = cli.system_prompt {
            custom.clone()
        } else {
            let deferred_names = tool_registry.deferred_tool_names();
            let deferred_refs: Vec<&str> = deferred_names.to_vec();
            let mut prompt =
                one_core::system_prompt::build_with_deferred_tools(&cwd, &deferred_refs);
            if let Some(ref append) = cli.append_system_prompt {
                prompt.push_str("\n\n");
                prompt.push_str(append);
            }
            prompt
        };

        let messages = vec![
            one_core::provider::Message {
                role: one_core::provider::Role::System,
                content: system_prompt,
            },
            one_core::provider::Message {
                role: one_core::provider::Role::User,
                content: prompt_text,
            },
        ];

        // Tool execution loop for one-shot mode
        let max_turns = cli.max_turns.unwrap_or(200);
        let mut msgs = messages;
        let cwd_for_tools = cwd.clone();
        let read_files: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>> =
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));

        for _turn in 0..max_turns {
            // Retry with exponential backoff on transient errors (matches CC's 10-retry pattern)
            let response = {
                let mut last_err = None;
                let mut result = None;
                for attempt in 0..10u32 {
                    if attempt > 0 {
                        // Exponential backoff: 500ms * 2^attempt + jitter
                        let base_ms = 500u64 * 2u64.pow(attempt.min(6));
                        // Simple jitter using process timing
                        let jitter = (std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .subsec_nanos() as u64)
                            % (base_ms / 2 + 1);
                        let delay = std::time::Duration::from_millis(base_ms + jitter);
                        if !cli.print_only {
                            eprintln!(
                                "\x1b[33m⎿  Retry {attempt}/10 after {}s...\x1b[0m",
                                delay.as_secs()
                            );
                        }
                        tokio::time::sleep(delay).await;
                    }

                    let on_chunk_retry: Box<dyn Fn(String) + Send + Sync> =
                        Box::new(|text: String| {
                            use std::io::Write;
                            print!("{text}");
                            let _ = std::io::stdout().flush();
                        });

                    match ai_provider
                        .stream_message(&msgs, &model_config, on_chunk_retry)
                        .await
                    {
                        Ok(r) => {
                            result = Some(r);
                            break;
                        }
                        Err(e) => {
                            let err_str = e.to_string();
                            let is_retryable = err_str.contains("429")
                                || err_str.contains("500")
                                || err_str.contains("502")
                                || err_str.contains("503")
                                || err_str.contains("529")
                                || err_str.contains("overloaded")
                                || err_str.contains("rate limit");
                            if !is_retryable || attempt == 9 {
                                return Err(e);
                            }
                            last_err = Some(e);
                        }
                    }
                }
                result.ok_or_else(|| {
                    last_err.unwrap_or_else(|| anyhow::anyhow!("All retries exhausted"))
                })?
            };

            // Verbose: show token usage
            if cli.verbose {
                eprintln!(
                    "\x1b[2m[tokens: in={} out={} | tools={}]\x1b[0m",
                    response.usage.input_tokens,
                    response.usage.output_tokens,
                    response.tool_calls.len(),
                );
            }

            if response.tool_calls.is_empty() {
                if cli.output_format == "json" {
                    // JSON output mode: structured result
                    let output = serde_json::json!({
                        "result": response.content,
                        "usage": {
                            "input_tokens": response.usage.input_tokens,
                            "output_tokens": response.usage.output_tokens,
                        }
                    });
                    // Clear streamed text if we're in json mode
                    // (streaming already printed the text — for json mode,
                    //  we'd need to suppress streaming. For now, append newline)
                    println!();
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&output).unwrap_or_default()
                    );
                } else {
                    println!();
                }
                return Ok(());
            }

            // Execute tools and feed results back
            let mut assistant_content: Vec<serde_json::Value> = Vec::new();
            if !response.content.is_empty() {
                assistant_content.push(serde_json::json!({
                    "type": "text", "text": response.content
                }));
            }
            let mut tool_results: Vec<serde_json::Value> = Vec::new();

            for tc in &response.tool_calls {
                assistant_content.push(serde_json::json!({
                    "type": "tool_use", "id": tc.id, "name": tc.name, "input": tc.input
                }));

                // Show tool call (like CC's one-shot mode display, suppressed with -p)
                let show_tools = !cli.print_only;
                let detail = match tc.name.as_str() {
                    "Read" | "Write" | "Edit" => {
                        tc.input["file_path"].as_str().unwrap_or("").to_string()
                    }
                    "Bash" => {
                        let cmd = tc.input["command"].as_str().unwrap_or("");
                        let desc = tc.input["description"].as_str().unwrap_or("");
                        if !desc.is_empty() {
                            desc.to_string()
                        } else if cmd.len() > 80 {
                            format!("{}...", &cmd[..80])
                        } else {
                            cmd.to_string()
                        }
                    }
                    "Grep" => {
                        let pattern = tc.input["pattern"].as_str().unwrap_or("");
                        let path = tc.input["path"].as_str().unwrap_or("");
                        if path.is_empty() {
                            pattern.to_string()
                        } else {
                            format!("{pattern} in {path}")
                        }
                    }
                    "Glob" => tc.input["pattern"].as_str().unwrap_or("").to_string(),
                    "Agent" => tc.input["description"]
                        .as_str()
                        .unwrap_or("sub-agent")
                        .to_string(),
                    "web_fetch" => tc.input["url"].as_str().unwrap_or("").to_string(),
                    "web_search" => tc.input["query"].as_str().unwrap_or("").to_string(),
                    "ask_user" => tc.input["question"].as_str().unwrap_or("").to_string(),
                    "tool_search" => tc.input["query"].as_str().unwrap_or("").to_string(),
                    _ => String::new(),
                };
                let face = match tc.name.as_str() {
                    "Edit" => "Update",
                    "Glob" => "Search",
                    "Agent" => "Agent",
                    "tool_search" => "ToolSearch",
                    _ => &tc.name,
                };
                if show_tools {
                    if detail.is_empty() {
                        eprintln!("\x1b[2m⎿  {face}\x1b[0m");
                    } else {
                        eprintln!("\x1b[2m⎿  {face} {detail}\x1b[0m");
                    }
                }

                // Check permissions before executing
                let decision = permission_engine.check(&tc.name, &tc.input);
                if let Some(ref warning) = decision.warning
                    && show_tools
                {
                    eprintln!("\x1b[33m   ⎿  {warning}\x1b[0m");
                }
                if decision.behavior == one_core::permissions::PermissionBehavior::Deny {
                    let result = (format!("Tool use denied: {}", decision.reason), true);
                    if show_tools {
                        eprintln!("\x1b[31m   ⎿  Denied: {}\x1b[0m", decision.reason);
                    }
                    tool_results.push(serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": tc.id,
                        "content": result.0,
                        "is_error": result.1,
                    }));
                    continue;
                }

                // Execute tool: MCP tools route to MCP manager, built-in tools to registry
                let result = if one_core::mcp::client::McpManager::is_mcp_tool(&tc.name) {
                    match mcp_manager.call_tool(&tc.name, tc.input.clone()).await {
                        Ok(output) => (output, false),
                        Err(e) => (format!("MCP error: {e}"), true),
                    }
                } else {
                    let ctx = one_tools::ToolContext {
                        working_dir: cwd_for_tools.clone(),
                        session_id: String::new(),
                        read_files: read_files.clone(),
                        db_path: None,
                    };
                    match tool_registry.get(&tc.name) {
                        Some(tool) => match tool.execute(tc.input.clone(), &ctx).await {
                            Ok(r) => (r.output, r.is_error),
                            Err(e) => (format!("Error: {e}"), true),
                        },
                        None => (format!("Unknown tool: {}", tc.name), true),
                    }
                };

                // Show tool result summary (collapsed view)
                match tc.name.as_str() {
                    // Read: "N lines" — content goes to model, not user
                    "Read" => {
                        let count = result.0.lines().count();
                        eprintln!("\x1b[2m   ⎿  {count} lines\x1b[0m");
                    }
                    // Write: "N lines" from first line of output
                    "Write" => {
                        let first = result.0.lines().next().unwrap_or("Done");
                        eprintln!("\x1b[2m   ⎿  {first}\x1b[0m");
                    }
                    // Bash: output lines
                    "Bash" => {
                        let lines: Vec<&str> = result.0.lines().collect();
                        if lines.is_empty() {
                            eprintln!("\x1b[2m   ⎿  (No output)\x1b[0m");
                        } else {
                            for line in lines.iter().take(6) {
                                eprintln!("\x1b[2m   ⎿  {line}\x1b[0m");
                            }
                            if lines.len() > 6 {
                                eprintln!("\x1b[2m   ⎿  … +{} more lines\x1b[0m", lines.len() - 6);
                            }
                        }
                    }
                    // Grep/Glob: "Found N files/results"
                    "Grep" | "Glob" => {
                        let count = result.0.lines().count();
                        let unit = if tc.name == "Grep" {
                            "results"
                        } else {
                            "files"
                        };
                        eprintln!("\x1b[2m   ⎿  Found {count} {unit}\x1b[0m");
                    }
                    // Edit: summary + diff with colors
                    "Edit" => {
                        let mut lines_iter = result.0.lines();
                        if let Some(summary) = lines_iter.next() {
                            eprintln!("\x1b[2m   ⎿  {summary}\x1b[0m");
                        }
                        let remaining: Vec<&str> = lines_iter.collect();
                        for line in remaining.iter().take(12) {
                            let marker = line.get(6..8).unwrap_or("  ");
                            if marker == " +" {
                                eprintln!("\x1b[32m      {line}\x1b[0m");
                            } else if marker == " -" {
                                eprintln!("\x1b[31m      {line}\x1b[0m");
                            } else {
                                eprintln!("\x1b[2m      {line}\x1b[0m");
                            }
                        }
                        if remaining.len() > 12 {
                            eprintln!("\x1b[2m   ⎿  … +{} more lines\x1b[0m", remaining.len() - 12);
                        }
                    }
                    // Default
                    _ => {
                        let first = result.0.lines().next().unwrap_or("Done");
                        eprintln!("\x1b[2m   ⎿  {first}\x1b[0m");
                    }
                }

                tool_results.push(serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": tc.id,
                    "content": result.0,
                    "is_error": result.1,
                }));
            }

            msgs.push(one_core::provider::Message {
                role: one_core::provider::Role::Assistant,
                content: serde_json::to_string(&assistant_content).unwrap_or_default(),
            });
            msgs.push(one_core::provider::Message {
                role: one_core::provider::Role::User,
                content: serde_json::to_string(&tool_results).unwrap_or_default(),
            });
        }
        println!();
        return Ok(());
    }

    // Initialize core systems
    let event_bus = EventBus::default();
    let state = new_shared_state();

    // Open database for conversation persistence
    let db_path = one_db::Database::default_path();
    if let Some(parent) = std::path::Path::new(&db_path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let db = Arc::new(std::sync::Mutex::new(one_db::Database::open(&db_path)?));
    tracing::info!("Database opened at {db_path}");

    // Create tool registry and build executor
    let tool_registry = Arc::new(one_tools::create_default_registry());
    // Send only active (non-deferred) schemas to the API to save tokens
    let tool_schemas = tool_registry.active_schemas();

    // Create AI provider — all providers get tool schemas in their native format
    let ai_provider = one_ai::create_provider_with_tools(provider, api_key, tool_schemas.clone());

    let registry_for_executor = tool_registry.clone();
    let state_for_ctx = state.clone();
    let read_files: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
    let tool_executor: one_core::query_engine::ToolExecutor = Arc::new(
        move |name: String, input: serde_json::Value, working_dir: String| {
            let registry = registry_for_executor.clone();
            let read_files = read_files.clone();
            let state_ctx = state_for_ctx.clone();
            Box::pin(async move {
                // Resolve session_id and db_path from the currently active session.
                let (session_id, db_path) = {
                    let s = state_ctx.read().await;
                    let active_id = s.active_session_id.clone().unwrap_or_default();
                    let db_path = s
                        .sessions
                        .get(&active_id)
                        .map(|sess| sess.db_path.clone())
                        .filter(|p| !p.as_os_str().is_empty());
                    (active_id, db_path)
                };
                let ctx = one_tools::ToolContext {
                    working_dir,
                    session_id,
                    read_files,
                    db_path,
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

    // Default to current directory if no projects specified
    let projects = if cli.project.is_empty() {
        vec![std::env::current_dir()?.to_string_lossy().to_string()]
    } else {
        cli.project.clone()
    };

    // Show recent sessions for the first project before TUI launches.
    // Printed to stderr so it's visible briefly before the TUI takes over.
    {
        let first_project = projects.first().map(String::as_str).unwrap_or(".");
        let branch = one_core::worktree::get_current_branch(first_project)
            .unwrap_or_else(|| "main".to_string());
        if let Ok(sessions) = one_db::StoragePaths::list_sessions(first_project, &branch)
            && !sessions.is_empty()
        {
            eprintln!("Recent sessions on {}:", branch);
            for (i, s) in sessions.iter().take(5).enumerate() {
                let d = &s.opened_at; // YYYY_MM_DD_HH_MM_SS
                let dt_display = if d.len() >= 16 {
                    format!(
                        "{}-{}-{} {}:{}",
                        d.get(0..4).unwrap_or(""),
                        d.get(5..7).unwrap_or(""),
                        d.get(8..10).unwrap_or(""),
                        d.get(11..13).unwrap_or(""),
                        d.get(14..16).unwrap_or(""),
                    )
                } else {
                    d.to_string()
                };
                let label = if i == 0 { " ← most recent" } else { "" };
                eprintln!("  [{}]  {}{}", s.session_hash, dt_display, label);
            }
            eprintln!("Resume with: one --session <hash>");
            eprintln!();
        }
    }

    // Initialize sessions
    //   Default: new session (clean slate)
    //   --continue / -c: resume last session for each project
    //   --session <hash>: resume a specific session by its 6-char hash
    {
        let mut app_state = state.write().await;
        app_state.config = config.clone();

        // Helper: create a new session with filesystem storage paths attached.
        // Takes explicit parameters so it captures nothing from the outer scope.
        let make_new_session = |project_path: &str, mc: &ModelConfig| -> Session {
            let branch = one_core::worktree::get_current_branch(project_path)
                .unwrap_or_else(|| "main".to_string());
            match one_db::StoragePaths::for_new_session(project_path, &branch) {
                Ok(paths) => {
                    let _ = std::fs::create_dir_all(&paths.session_dir);
                    Session::new(project_path.to_string(), mc.clone()).with_storage_info(
                        paths.session_db,
                        paths.session_hash,
                        paths.branch,
                    )
                }
                Err(e) => {
                    tracing::warn!("StoragePaths init failed: {e}");
                    Session::new(project_path.to_string(), mc.clone())
                }
            }
        };

        // If --session was provided: 6-char hash → new-style lookup; longer → legacy UUID fallback
        if let Some(ref session_hash) = cli.session {
            let project_path = projects.first().cloned().unwrap_or_else(|| ".".to_string());
            let is_short_hash = session_hash.len() <= 8;

            if is_short_hash {
                // New-style: locate by 6-char hash under ~/.one/{project}/
                let found =
                    match one_db::StoragePaths::for_existing_session(&project_path, session_hash) {
                        Ok(Some(paths)) => Some(paths),
                        Ok(None) => {
                            eprintln!(
                                "No session found with hash '{session_hash}'. Starting fresh."
                            );
                            None
                        }
                        Err(e) => {
                            eprintln!("Error looking up session: {e}. Starting fresh.");
                            None
                        }
                    };

                let session = if let Some(ref paths) = found {
                    // Resume: load messages from per-session SQLite DB
                    let mut s = Session::new(project_path.clone(), model_config.clone())
                        .with_storage_info(
                            paths.session_db.clone(),
                            paths.session_hash.clone(),
                            paths.branch.clone(),
                        );
                    if let Ok(session_db) = one_db::SessionDb::open(&paths.session_db)
                        && let Ok(messages) = session_db.load_messages(None, None, true)
                    {
                        for msg in messages {
                            let role = match msg.role.as_str() {
                                "user" => one_core::conversation::TurnRole::User,
                                "assistant" => one_core::conversation::TurnRole::Assistant,
                                _ => continue,
                            };
                            s.conversation
                                .turns
                                .push(one_core::conversation::ConversationTurn {
                                    role,
                                    content: msg.content,
                                    timestamp: msg
                                        .created_at
                                        .parse()
                                        .unwrap_or_else(|_| chrono::Utc::now()),
                                    tool_calls: Vec::new(),
                                    is_streaming: false,
                                    tokens_used: None,
                                });
                        }
                    }
                    s
                } else {
                    make_new_session(&project_path, &model_config)
                };

                let turn_count = session.conversation.turns.len();
                let project_name = session.project_name.clone();
                let sid = session.id.clone();
                app_state.sessions.insert(sid.clone(), session);
                app_state.active_session_id = Some(sid.clone());
                let _ = event_bus.sender().send(Event::SessionCreated {
                    session_id: sid,
                    project: if turn_count > 0 {
                        format!("{project_name} ({turn_count} turns)")
                    } else {
                        project_name
                    },
                });
            } else {
                // Legacy: full UUID or external session ID — preserve existing behavior
                let backend = one_core::storage::StorageBackend::detect(session_hash);

                let mut session = Session::new(project_path, model_config.clone());
                session.id = session_hash.clone();

                match &backend {
                    one_core::storage::StorageBackend::ClaudeCode { .. }
                    | one_core::storage::StorageBackend::Codex { .. }
                    | one_core::storage::StorageBackend::Gemini { .. } => {
                        match backend.load(session_hash) {
                            Ok(conv) => {
                                session.conversation = conv;
                            }
                            Err(e) => {
                                eprintln!("Warning: failed to load session: {e}");
                            }
                        }
                    }
                    one_core::storage::StorageBackend::Native => {
                        if let Ok(messages) = db.lock().unwrap().load_messages(session_hash) {
                            for msg in messages {
                                let role = match msg.role.as_str() {
                                    "user" => one_core::conversation::TurnRole::User,
                                    "assistant" => one_core::conversation::TurnRole::Assistant,
                                    _ => continue,
                                };
                                session.conversation.turns.push(
                                    one_core::conversation::ConversationTurn {
                                        role,
                                        content: msg.content,
                                        timestamp: msg
                                            .created_at
                                            .parse()
                                            .unwrap_or_else(|_| chrono::Utc::now()),
                                        tool_calls: Vec::new(),
                                        is_streaming: false,
                                        tokens_used: None,
                                    },
                                );
                            }
                        }
                    }
                }

                let backend_name = match &backend {
                    one_core::storage::StorageBackend::ClaudeCode { .. } => " (Claude Code)",
                    one_core::storage::StorageBackend::Codex { .. } => " (Codex)",
                    one_core::storage::StorageBackend::Gemini { .. } => " (Gemini)",
                    one_core::storage::StorageBackend::Native => "",
                };

                let turn_count = session.conversation.turns.len();
                let project_name = session.project_name.clone();
                app_state.sessions.insert(session_hash.clone(), session);
                app_state.active_session_id = Some(session_hash.clone());
                let _ = event_bus.sender().send(Event::SessionCreated {
                    session_id: session_hash.clone(),
                    project: format!("{project_name}{backend_name} ({turn_count} turns)"),
                });
            }
        } else {
            // Normal mode: one session per project path
            for project_path in &projects {
                let (session, restored) = if cli.continue_session {
                    // --continue: try to restore last session from legacy DB
                    let existing = db
                        .lock()
                        .unwrap()
                        .find_session_by_project(project_path)
                        .ok()
                        .flatten();

                    if let Some(record) = existing {
                        let mut session = make_new_session(project_path, &model_config);
                        session.id = record.id;

                        if let Ok(messages) = db.lock().unwrap().load_messages(&session.id) {
                            for msg in messages {
                                let role = match msg.role.as_str() {
                                    "user" => one_core::conversation::TurnRole::User,
                                    "assistant" => one_core::conversation::TurnRole::Assistant,
                                    _ => continue,
                                };
                                session.conversation.turns.push(
                                    one_core::conversation::ConversationTurn {
                                        role,
                                        content: msg.content,
                                        timestamp: msg
                                            .created_at
                                            .parse()
                                            .unwrap_or_else(|_| chrono::Utc::now()),
                                        tool_calls: Vec::new(),
                                        is_streaming: false,
                                        tokens_used: None,
                                    },
                                );
                            }
                        }
                        (session, true)
                    } else {
                        (make_new_session(project_path, &model_config), false)
                    }
                } else {
                    // Default: always new session
                    (make_new_session(project_path, &model_config), false)
                };

                // Also create JSONL transcript file (dual-write for import round-trips)
                let _ = one_core::storage::create_claude_code_session(
                    &session.id,
                    &session.project_path,
                );

                // Save to legacy DB (strangler-fig: kept alive until full migration)
                let _ = db.lock().unwrap().save_session(&one_db::SessionRecord {
                    id: session.id.clone(),
                    project_path: session.project_path.clone(),
                    project_name: session.project_name.clone(),
                    model_provider: format!("{}", session.model_config.provider),
                    model_name: session.model_config.model.clone(),
                    created_at: session.created_at.to_rfc3339(),
                    cost_usd: 0.0,
                });

                let session_id = session.id.clone();
                let project_name = session.project_name.clone();
                let turn_count = session.conversation.turns.len();
                app_state.sessions.insert(session_id.clone(), session);

                if app_state.active_session_id.is_none() {
                    app_state.active_session_id = Some(session_id.clone());
                }

                let _ = event_bus.sender().send(Event::SessionCreated {
                    session_id,
                    project: if restored {
                        format!("{project_name} ({turn_count} turns)")
                    } else {
                        project_name
                    },
                });
            }
        }
    }

    // Spawn background persistence — saves new messages to DB as they arrive
    {
        let mut persist_rx = event_bus.sender().subscribe();
        let db_persist = db.clone();
        let state_persist = state.clone();

        tokio::spawn(async move {
            loop {
                match persist_rx.recv().await {
                    Ok(Event::UserMessage {
                        session_id,
                        content,
                    }) => {
                        let _ = db_persist.lock().unwrap().save_message(
                            &session_id,
                            "user",
                            &content,
                            &chrono::Utc::now().to_rfc3339(),
                        );

                        // Also write to JSONL transcript
                        let s = state_persist.read().await;
                        if let Some(session) = s.sessions.get(&session_id) {
                            // Dual-write to per-session SQLite DB (new-style sessions only)
                            if !session.db_path.as_os_str().is_empty() {
                                let db_path_w = session.db_path.clone();
                                let content_w = content.clone();
                                let now_w = chrono::Utc::now().to_rfc3339();
                                tokio::task::spawn_blocking(move || {
                                    if let Ok(db) = one_db::SessionDb::open(&db_path_w) {
                                        let _ = db.save_message("user", &content_w, &now_w, None);
                                    }
                                });
                            }
                            let backend = one_core::storage::StorageBackend::detect(&session_id);
                            if let one_core::storage::StorageBackend::ClaudeCode { .. } = backend {
                                let turn = one_core::conversation::ConversationTurn {
                                    role: one_core::conversation::TurnRole::User,
                                    content: content.clone(),
                                    timestamp: chrono::Utc::now(),
                                    tool_calls: Vec::new(),
                                    is_streaming: false,
                                    tokens_used: None,
                                };
                                let _ = backend.append_turn(&session_id, &turn);
                            } else {
                                // New session — create JSONL and write
                                if let Ok(path) = one_core::storage::create_claude_code_session(
                                    &session_id,
                                    &session.project_path,
                                ) {
                                    let backend = one_core::storage::StorageBackend::ClaudeCode {
                                        jsonl_path: path,
                                    };
                                    let turn = one_core::conversation::ConversationTurn {
                                        role: one_core::conversation::TurnRole::User,
                                        content: content.clone(),
                                        timestamp: chrono::Utc::now(),
                                        tool_calls: Vec::new(),
                                        is_streaming: false,
                                        tokens_used: None,
                                    };
                                    let _ = backend.append_turn(&session_id, &turn);
                                }
                            }
                        }
                    }
                    Ok(Event::AiResponseChunk {
                        session_id,
                        done: true,
                        ..
                    }) => {
                        // Save the completed assistant response
                        let s = state_persist.read().await;
                        if let Some(session) = s.sessions.get(&session_id)
                            && let Some(last) = session.conversation.turns.last()
                            && last.role == one_core::conversation::TurnRole::Assistant
                        {
                            let _ = db_persist.lock().unwrap().save_message(
                                &session_id,
                                "assistant",
                                &last.content,
                                &last.timestamp.to_rfc3339(),
                            );

                            // Dual-write to per-session SQLite DB (new-style sessions only)
                            if !session.db_path.as_os_str().is_empty() {
                                let db_path_w = session.db_path.clone();
                                let content_w = last.content.clone();
                                let ts_w = last.timestamp.to_rfc3339();
                                let tokens_w = last.tokens_used.map(|t| t as i64);
                                tokio::task::spawn_blocking(move || {
                                    if let Ok(db) = one_db::SessionDb::open(&db_path_w) {
                                        let _ = db.save_message(
                                            "assistant",
                                            &content_w,
                                            &ts_w,
                                            tokens_w,
                                        );
                                    }
                                });
                            }

                            // Also write to JSONL transcript
                            let backend = one_core::storage::StorageBackend::detect(&session_id);
                            if let one_core::storage::StorageBackend::ClaudeCode { .. } = backend {
                                let _ = backend.append_turn(&session_id, last);
                            }
                        }
                    }
                    Ok(Event::Quit) => break,
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(_) => {}
                }
            }
        });
    }

    // Start integrations
    if config.integrations.github.token.is_some() || std::env::var("GITHUB_TOKEN").is_ok() {
        let gh_token = config
            .integrations
            .github
            .token
            .clone()
            .or_else(|| std::env::var("GITHUB_TOKEN").ok())
            .unwrap_or_default();

        let gh_repos = config.integrations.github.repos.clone();

        let mut gh = one_integrations::github::GitHubIntegration::new(gh_token, gh_repos);
        let _ = gh.start(event_bus.sender()).await;
    }

    // Slack
    {
        let token = config
            .integrations
            .slack
            .token
            .clone()
            .or_else(|| std::env::var("SLACK_TOKEN").ok())
            .unwrap_or_default();
        if !token.is_empty() {
            let channels = config.integrations.slack.channels.clone();
            let mut slack = one_integrations::slack::SlackIntegration::new(token, channels);
            let _ = slack.start(event_bus.sender()).await;
        }
    }

    // Asana
    {
        let token = config
            .integrations
            .asana
            .token
            .clone()
            .or_else(|| std::env::var("ASANA_TOKEN").ok())
            .unwrap_or_default();
        if !token.is_empty() {
            let workspace = config.integrations.asana.workspace.clone();
            let mut asana = one_integrations::asana::AsanaIntegration::new(token, workspace);
            let _ = asana.start(event_bus.sender()).await;
        }
    }

    // Notion
    {
        let token = config
            .integrations
            .notion
            .token
            .clone()
            .or_else(|| std::env::var("NOTION_TOKEN").ok())
            .unwrap_or_default();
        if !token.is_empty() {
            let mut notion = one_integrations::notion::NotionIntegration::new(token);
            let _ = notion.start(event_bus.sender()).await;
        }
    }

    // Initialize MCP servers (if configured)
    let first_project = projects.first().cloned().unwrap_or_else(|| ".".to_string());
    let mut mcp_manager = one_core::mcp::client::McpManager::new();
    mcp_manager.connect_all(&first_project).await;

    let mcp_tool_count = mcp_manager.all_tools().len();
    if mcp_tool_count > 0 {
        tracing::info!("MCP: {mcp_tool_count} tools available from MCP servers");
    }

    // Merge MCP tool schemas with built-in tools
    let mut all_tool_schemas = tool_schemas;
    all_tool_schemas.extend(mcp_manager.tool_schemas());

    // Load permission settings from settings.json files (CLI flags override)
    let mut permission_engine = one_core::settings::load_permission_engine(&first_project);
    if cli.dangerously_skip_permissions {
        permission_engine.mode = one_core::permissions::PermissionMode::BypassPermissions;
    }
    for tool_name in &cli.allowed_tools {
        permission_engine.add_session_rule(
            tool_name,
            None,
            one_core::permissions::PermissionBehavior::Allow,
        );
    }
    tracing::debug!("Permission mode: {:?}", permission_engine.mode);

    // Collect deferred tool names so the model knows they exist via tool_search
    let deferred_names: Vec<String> = tool_registry
        .deferred_tool_names()
        .iter()
        .map(|s| s.to_string())
        .collect();

    // Load hooks from settings.json
    let hooks = one_core::settings::load_hooks(&first_project);

    // Clone for the Evergreen background task before the engine consumes these values.
    let evergreen_provider = ai_provider.clone();
    let evergreen_config = model_config.clone();

    // Spawn the query engine with tools + MCP + permissions + hooks
    let engine = QueryEngine::new(state.clone(), ai_provider, model_config, event_bus.sender())
        .with_tools(all_tool_schemas, tool_executor)
        .with_mcp_manager(mcp_manager)
        .with_permission_engine(permission_engine)
        .with_deferred_tool_names(deferred_names)
        .with_hooks(hooks);

    let _engine_handle = engine.spawn();

    // Spawn the Evergreen background compression task.
    // Wakes on each completed AI response turn, compresses eligible history spans,
    // and writes results to the per-session SQLite DB.
    let _evergreen_handle = tasks::evergreen::spawn(
        state.clone(),
        event_bus.sender(),
        evergreen_provider,
        evergreen_config,
    );

    // Launch TUI
    let mut app = App::new(state.clone(), event_bus.sender());
    app.run().await?;

    // Post-exit: session hashes (for --session resume) + token/cost summary
    {
        let s = state.read().await;
        let now = chrono::Utc::now();

        // Session hash(es) for resuming with `one --session <hash>`
        let hashes: Vec<String> = s
            .sessions
            .values()
            .filter(|sess| !sess.session_hash.is_empty())
            .map(|sess| sess.session_hash.clone())
            .collect();
        if !hashes.is_empty() {
            if hashes.len() == 1 {
                eprintln!(
                    "Session: [{}]  resume with: one --session {}",
                    hashes[0], hashes[0]
                );
            } else {
                eprintln!("Sessions ended:");
                for h in &hashes {
                    eprintln!("  [{}]  one --session {}", h, h);
                }
            }
        }

        // Token/cost summary per session
        let mut printed = false;
        for session in s.sessions.values() {
            if session.total_input_tokens == 0 {
                continue;
            }
            if !printed {
                eprintln!();
                printed = true;
            }
            let secs = now
                .signed_duration_since(session.created_at)
                .num_seconds()
                .max(0) as u64;
            let duration_str = if secs >= 3600 {
                format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
            } else if secs >= 60 {
                format!("{}m {}s", secs / 60, secs % 60)
            } else {
                format!("{}s", secs)
            };
            let turns = session
                .conversation
                .turns
                .iter()
                .filter(|t| t.role == one_core::conversation::TurnRole::User)
                .count();
            let turn_label = if turns == 1 { "turn" } else { "turns" };
            eprintln!(
                "\u{2812} {} \u{2014} {} \u{00b7} {} {} \u{00b7} \u{2191} {} \u{00b7} \u{2193} {} tokens \u{00b7} ~${:.4}",
                session.project_name,
                duration_str,
                turns,
                turn_label,
                fmt_tokens(session.total_input_tokens),
                fmt_tokens(session.total_output_tokens),
                session.cost_usd,
            );
        }
    }

    Ok(())
}

/// Format a token count with comma thousands separators (e.g. 12345 → "12,345").
fn fmt_tokens(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}
