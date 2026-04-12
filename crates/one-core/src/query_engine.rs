use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::broadcast;

use crate::agent::{AgentRegistry, AgentRole};
use crate::event::Event;
use crate::mcp::client::McpManager;
use crate::permissions::{PermissionBehavior, PermissionEngine, PermissionMode};
use crate::provider::{
    AiProvider, Message, MessageComplexity, ModelConfig, Role, estimate_message_complexity,
    model_capabilities, resolve_effort,
};
use crate::state::SharedState;

/// Type alias for tool executor functions passed in from outside.
pub type ToolExecutor = Arc<
    dyn Fn(
            String,
            serde_json::Value,
            String,
        ) -> Pin<Box<dyn Future<Output = ToolExecResult> + Send>>
        + Send
        + Sync,
>;

pub struct ToolExecResult {
    pub output: String,
    pub is_error: bool,
}

pub struct QueryEngine {
    pub(crate) state: SharedState,
    pub(crate) provider: Arc<dyn AiProvider>,
    pub(crate) model_config: ModelConfig,
    pub(crate) event_tx: broadcast::Sender<Event>,
    pub(crate) tool_schemas: Vec<serde_json::Value>,
    pub(crate) tool_executor: Option<ToolExecutor>,
    pub(crate) agent_registry: AgentRegistry,
    pub(crate) permission_engine: PermissionEngine,
    pub(crate) mcp_manager: Arc<tokio::sync::Mutex<McpManager>>,
    pub(crate) deferred_tool_names: Vec<String>,
    pub(crate) hooks: crate::settings::HooksConfig,
}

impl QueryEngine {
    pub fn new(
        state: SharedState,
        provider: Arc<dyn AiProvider>,
        model_config: ModelConfig,
        event_tx: broadcast::Sender<Event>,
    ) -> Self {
        Self {
            state,
            provider,
            model_config,
            event_tx,
            tool_schemas: Vec::new(),
            tool_executor: None,
            agent_registry: AgentRegistry::with_defaults(),
            permission_engine: PermissionEngine::new(PermissionMode::Default),
            mcp_manager: Arc::new(tokio::sync::Mutex::new(McpManager::new())),
            deferred_tool_names: Vec::new(),
            hooks: crate::settings::HooksConfig::default(),
        }
    }

    pub fn with_tools(mut self, schemas: Vec<serde_json::Value>, executor: ToolExecutor) -> Self {
        self.tool_schemas = schemas;
        self.tool_executor = Some(executor);
        self
    }

    pub fn with_permission_mode(mut self, mode: PermissionMode) -> Self {
        self.permission_engine = PermissionEngine::new(mode);
        self
    }

    pub fn with_permission_engine(mut self, engine: PermissionEngine) -> Self {
        self.permission_engine = engine;
        self
    }

    pub fn with_mcp_manager(mut self, manager: McpManager) -> Self {
        self.mcp_manager = Arc::new(tokio::sync::Mutex::new(manager));
        self
    }

    pub fn with_deferred_tool_names(mut self, names: Vec<String>) -> Self {
        self.deferred_tool_names = names;
        self
    }

    pub fn with_hooks(mut self, hooks: crate::settings::HooksConfig) -> Self {
        self.hooks = hooks;
        self
    }

    /// Merge MCP tool schemas into the engine's tool list.
    pub async fn load_mcp_tools(&mut self) {
        let mcp = self.mcp_manager.lock().await;
        let mcp_schemas = mcp.tool_schemas();
        let count = mcp_schemas.len();
        self.tool_schemas.extend(mcp_schemas);
        if count > 0 {
            tracing::info!("Added {count} MCP tools to query engine");
        }
    }

    /// Emit a debug log line for the given session. No-op if the send fails.
    fn debug(&self, session_id: &str, message: impl Into<String>) {
        let _ = self.event_tx.send(Event::DebugLog {
            session_id: session_id.to_string(),
            message: message.into(),
        });
    }

    pub fn spawn(mut self) -> tokio::task::JoinHandle<()> {
        let mut event_rx = self.event_tx.subscribe();

        tokio::spawn(async move {
            loop {
                match event_rx.recv().await {
                    Ok(Event::UserMessage {
                        session_id,
                        content,
                    }) => {
                        self.handle_user_message(&session_id, &content).await;
                    }
                    Ok(Event::Quit) => break,
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("QueryEngine lagged by {n} events");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            tracing::info!("QueryEngine shutting down");
        })
    }

    async fn handle_user_message(&mut self, session_id: &str, content: &str) {
        // Auto-memory: detect memory-worthy patterns in user messages
        if let Some(memory) = crate::memory::detect_memory_trigger(content) {
            let project_dir = {
                let s = self.state.read().await;
                s.sessions
                    .get(session_id)
                    .map(|s| s.project_path.clone())
                    .unwrap_or_default()
            };
            let store = crate::memory::MemoryStore::for_project(&project_dir);
            if let Err(e) = store.save(&memory) {
                tracing::warn!("Failed to auto-save memory: {e}");
            } else {
                tracing::info!("Auto-saved {} memory: {}", memory.memory_type, memory.name);
            }
        }

        // Early check: is the provider configured with valid credentials?
        if !self.provider.is_configured() {
            let provider_name = self.provider.provider_name();
            let error_msg = format!(
                "No API key configured. Use `/login {provider_name}` to sign in, \
                 or `/login {provider_name} <key>` to set your API key."
            );
            tracing::debug!(
                "Provider '{}' is not configured — no API key set",
                provider_name
            );

            // Show the error as an assistant message so the user can see it
            {
                let mut state = self.state.write().await;
                if let Some(session) = state.sessions.get_mut(session_id) {
                    session.conversation.start_assistant_response();
                    session.conversation.append_to_current(&error_msg);
                    session.conversation.finish_current(None);
                }
            }
            let _ = self.event_tx.send(Event::AiResponseChunk {
                session_id: session_id.to_string(),
                content: error_msg,
                done: true,
            });
            return;
        }

        // Classify intent to select the right agent
        let agent_role = self.classify_intent(content);

        if let Some(ref role) = agent_role {
            tracing::debug!("Routed to {:?} agent", role);
            self.debug(session_id, format!("agent → {:?}", role));
        }

        let messages = {
            let state = self.state.read().await;
            let session = match state.sessions.get(session_id) {
                Some(s) => s,
                None => {
                    tracing::error!("No session found for {session_id}");
                    return;
                }
            };

            let system_prompt = self.build_system_prompt(session, agent_role);
            let mut msgs = vec![Message {
                role: Role::System,
                content: system_prompt.clone(),
            }];

            // Build conversation messages from turns.
            // Skip empty assistant turns — they're either stuck streaming turns
            // from a previous failed request or tool-only turns with no prose.
            for turn in &session.conversation.turns {
                let role = match turn.role {
                    crate::conversation::TurnRole::User => Role::User,
                    crate::conversation::TurnRole::Assistant => {
                        if turn.content.is_empty() {
                            continue;
                        }
                        Role::Assistant
                    }
                    crate::conversation::TurnRole::System
                    | crate::conversation::TurnRole::ToolResult => continue,
                };
                msgs.push(Message {
                    role,
                    content: turn.content.clone(),
                });
            }

            // Anthropic (and most providers) require the conversation to end
            // with a user message. Trim any trailing assistant turns that
            // slipped through (e.g. a partial response from a prior error).
            while msgs
                .last()
                .map(|m| m.role == Role::Assistant)
                .unwrap_or(false)
            {
                msgs.pop();
            }

            msgs
        };

        // Don't pre-create an assistant turn — the on_chunk callback will
        // start one when the first text chunk arrives. This prevents empty
        // [one] headers in the TUI.

        let session_id_owned = session_id.to_string();
        let event_tx = self.event_tx.clone();
        let state_clone = self.state.clone();

        // Resolve effort once for this whole request cycle.
        // Reads session.effort, model capabilities, and message complexity to produce
        // concrete budget_tokens + max_tokens. min_ctx_pct is 0.0 until Evergreen is live.
        let request_config = {
            let state = self.state.read().await;
            let effort = state
                .sessions
                .get(session_id)
                .and_then(|s| s.effort.as_deref());
            let caps = model_capabilities(&self.model_config.model);
            let complexity = estimate_message_complexity(content);
            let resolved = resolve_effort(effort, &caps, 0.0, complexity);
            tracing::debug!(
                "Effort resolved: {} (budget_tokens={}, max_tokens={})",
                resolved.label,
                resolved.budget_tokens,
                resolved.max_tokens,
            );
            let mut config = self.model_config.clone();
            config.max_tokens = resolved.max_tokens;
            config.budget_tokens = (resolved.budget_tokens > 0).then_some(resolved.budget_tokens);
            config
        };

        // Tool execution loop: stream response, execute tools, feed results back.
        // No hard turn limit by default (matches CC). Guard against infinite loops
        // with a generous upper bound.
        const MAX_TOOL_TURNS: usize = 200;
        let mut current_messages = messages;

        for _turn in 0..MAX_TOOL_TURNS {
            let state_for_loop = state_clone.clone();
            let sid_for_loop = session_id_owned.clone();
            let tx_for_loop = event_tx.clone();

            let on_chunk_loop: Box<dyn Fn(String) + Send + Sync> = Box::new(move |text: String| {
                if let Ok(mut state) = state_for_loop.try_write()
                    && let Some(session) = state.sessions.get_mut(&sid_for_loop)
                {
                    // Start a new assistant turn if one isn't already streaming
                    if !session.conversation.last_is_streaming() {
                        session.conversation.start_assistant_response();
                    }
                    session.conversation.append_to_current(&text);
                }
                let _ = tx_for_loop.send(Event::AiResponseChunk {
                    session_id: sid_for_loop.clone(),
                    content: text,
                    done: false,
                });
            });

            // Auto-compact check before API call — uses the resolved config so the
            // token threshold matches what we're actually sending.
            if crate::compact::auto_compact::should_auto_compact(
                &current_messages,
                &request_config.model,
                request_config.max_tokens,
            ) {
                tracing::info!("Auto-compacting conversation (token limit approaching)");
                self.debug(
                    &session_id_owned,
                    "auto-compact → token limit approaching, summarising",
                );
                if let Some(result) = crate::compact::auto_compact::auto_compact_if_needed(
                    &current_messages,
                    &self.provider,
                    &request_config,
                    &mut crate::compact::auto_compact::AutoCompactTracking::default(),
                )
                .await
                {
                    // Replace messages with compacted summary + recent tail
                    current_messages = vec![
                        current_messages[0].clone(), // Keep system prompt
                        Message {
                            role: Role::User,
                            content: result.summary_message,
                        },
                    ];
                    // Re-add recent messages if any were preserved
                    current_messages.extend(result.messages_to_keep);
                }
            }

            // Signal TUI that we're starting a request — this activates the
            // thinking status (spinning verbs) immediately, before any text
            // arrives. Without this, the user sees nothing during the thinking
            // phase because thinking deltas are silently consumed.
            {
                let mut state = state_clone.write().await;
                if let Some(session) = state.sessions.get_mut(&session_id_owned)
                    && !session.conversation.last_is_streaming()
                {
                    session.conversation.start_assistant_response();
                }
            }
            let _ = event_tx.send(Event::AiResponseChunk {
                session_id: session_id_owned.clone(),
                content: String::new(),
                done: false,
            });

            self.debug(
                &session_id_owned,
                format!(
                    "api → {} ({} messages)",
                    self.model_config.model,
                    current_messages.len()
                ),
            );
            match self
                .provider
                .stream_message(&current_messages, &request_config, on_chunk_loop)
                .await
            {
                Ok(response) => {
                    self.debug(
                        &session_id_owned,
                        format!(
                            "api ← ↑ {} / ↓ {} tokens{}",
                            response.usage.input_tokens,
                            response.usage.output_tokens,
                            if response.tool_calls.is_empty() {
                                String::new()
                            } else {
                                format!(" · {} tool call(s)", response.tool_calls.len())
                            }
                        ),
                    );
                    // No tool calls → finalize and exit loop
                    if response.tool_calls.is_empty() {
                        {
                            let mut state = state_clone.write().await;
                            if let Some(session) = state.sessions.get_mut(&session_id_owned) {
                                let current_len = session
                                    .conversation
                                    .turns
                                    .last()
                                    .map(|t| t.content.len())
                                    .unwrap_or(0);
                                if current_len < response.content.len()
                                    && let Some(last) = session.conversation.turns.last_mut()
                                {
                                    last.content = response.content.clone();
                                }
                                session.record_usage(
                                    response.usage.input_tokens,
                                    response.usage.output_tokens,
                                );
                                session.conversation.finish_current(Some(
                                    response.usage.input_tokens + response.usage.output_tokens,
                                ));
                            }
                        }
                        let _ = event_tx.send(Event::AiResponseChunk {
                            session_id: session_id_owned,
                            content: String::new(),
                            done: true,
                        });
                        return;
                    }

                    // Check plan mode — describe tools without executing
                    let is_plan_mode = {
                        let s = state_clone.read().await;
                        s.plan_mode
                    };

                    if is_plan_mode {
                        // Plan mode: describe what would happen, don't execute
                        let mut tool_results: Vec<serde_json::Value> = Vec::new();
                        let mut assistant_content: Vec<serde_json::Value> = Vec::new();
                        if !response.content.is_empty() {
                            assistant_content.push(serde_json::json!({
                                "type": "text",
                                "text": response.content,
                            }));
                        }
                        for tc in &response.tool_calls {
                            assistant_content.push(serde_json::json!({
                                "type": "tool_use",
                                "id": tc.id,
                                "name": tc.name,
                                "input": tc.input,
                            }));
                            let desc = format!(
                                "[Plan mode] Would execute {} with input: {}",
                                tc.name,
                                serde_json::to_string(&tc.input).unwrap_or_default()
                            );
                            tool_results.push(serde_json::json!({
                                "type": "tool_result",
                                "tool_use_id": tc.id,
                                "content": desc,
                                "is_error": false,
                            }));
                            let _ = event_tx.send(Event::ToolRequest {
                                session_id: session_id_owned.clone(),
                                tool_name: tc.name.clone(),
                                input: tc.input.clone(),
                                call_id: tc.id.clone(),
                            });
                            let _ = event_tx.send(Event::ToolDenied {
                                session_id: session_id_owned.clone(),
                                call_id: tc.id.clone(),
                                tool_name: tc.name.clone(),
                                reason: "Plan mode active".to_string(),
                                warning: None,
                            });
                        }

                        {
                            let mut state = state_clone.write().await;
                            if let Some(session) = state.sessions.get_mut(&session_id_owned) {
                                session.record_usage(
                                    response.usage.input_tokens,
                                    response.usage.output_tokens,
                                );
                                session.conversation.finish_current(Some(
                                    response.usage.input_tokens + response.usage.output_tokens,
                                ));
                            }
                        }

                        current_messages.push(Message {
                            role: Role::Assistant,
                            content: serde_json::to_string(&assistant_content).unwrap_or_default(),
                        });
                        current_messages.push(Message {
                            role: Role::User,
                            content: serde_json::to_string(&tool_results).unwrap_or_default(),
                        });
                        continue;
                    }

                    // Tool calls present → execute tools and loop
                    if let Some(ref executor) = self.tool_executor {
                        // Ensure an assistant turn exists for this round's tool calls.
                        // If the model returned only tool_use blocks (no text),
                        // on_chunk never fired, so no turn was created.
                        {
                            let mut state = state_clone.write().await;
                            if let Some(session) = state.sessions.get_mut(&session_id_owned) {
                                if !session.conversation.last_is_streaming() {
                                    session.conversation.start_assistant_response();
                                    // Set any text content from the response
                                    if !response.content.is_empty() {
                                        session.conversation.append_to_current(&response.content);
                                    }
                                }
                                session.record_usage(
                                    response.usage.input_tokens,
                                    response.usage.output_tokens,
                                );
                                session.conversation.finish_current(Some(
                                    response.usage.input_tokens + response.usage.output_tokens,
                                ));
                            }
                        }

                        // Check permissions for each tool call
                        let working_dir = {
                            let s = state_clone.read().await;
                            s.sessions
                                .get(&session_id_owned)
                                .map(|s| s.project_path.clone())
                                .unwrap_or_default()
                        };

                        // Partition tool calls into allowed and denied
                        let mut allowed_calls = Vec::new();
                        let mut tool_results: Vec<serde_json::Value> = Vec::new();
                        // Track which indices in tool_results are placeholders for allowed calls
                        let mut allowed_indices = Vec::new();

                        for tool_call in &response.tool_calls {
                            // Intercept Agent tool calls — run sub-agent directly
                            if tool_call.name == "Agent" {
                                let prompt = tool_call.input["prompt"]
                                    .as_str()
                                    .unwrap_or("No prompt provided");
                                let description = tool_call.input["description"]
                                    .as_str()
                                    .unwrap_or("Sub-agent task");
                                let subagent_type = tool_call.input["subagent_type"].as_str();
                                let model_override = tool_call.input["model"].as_str();
                                let use_fork = tool_call.input["fork"].as_bool().unwrap_or(false);
                                let fork_msgs = if use_fork {
                                    Some(current_messages.clone())
                                } else {
                                    None
                                };
                                let run_in_background = tool_call.input["run_in_background"]
                                    .as_bool()
                                    .unwrap_or(false);

                                let _ = event_tx.send(Event::ToolRequest {
                                    session_id: session_id_owned.clone(),
                                    tool_name: "Agent".to_string(),
                                    input: tool_call.input.clone(),
                                    call_id: tool_call.id.clone(),
                                });

                                if run_in_background {
                                    // Background agent: spawn and return immediately
                                    let agent_id = format!(
                                        "agent_{}",
                                        uuid::Uuid::new_v4()
                                            .to_string()
                                            .split('-')
                                            .next()
                                            .unwrap_or("0")
                                    );
                                    let bg_prompt = prompt.to_string();
                                    let bg_desc = description.to_string();
                                    let bg_type = subagent_type.map(String::from);
                                    let bg_model = model_override.map(String::from);
                                    let bg_wd = working_dir.clone();
                                    let bg_provider = self.provider.clone();
                                    let bg_config = self.model_config.clone();
                                    let bg_executor = self.tool_executor.clone();
                                    let bg_schemas = self.tool_schemas.clone();
                                    let bg_agent_reg = self.agent_registry.clone();
                                    let bg_mcp = self.mcp_manager.clone();
                                    let bg_event_tx = event_tx.clone();
                                    let _bg_sid = session_id_owned.clone();
                                    let bg_agent_id = agent_id.clone();

                                    tokio::spawn(async move {
                                        // Create a mini query engine for the background agent
                                        let mut bg_engine = QueryEngine::new(
                                            crate::state::new_shared_state(),
                                            bg_provider,
                                            bg_config,
                                            bg_event_tx.clone(),
                                        );
                                        bg_engine.tool_schemas = bg_schemas;
                                        bg_engine.tool_executor = bg_executor;
                                        bg_engine.agent_registry = bg_agent_reg;
                                        bg_engine.mcp_manager = bg_mcp;

                                        let result = bg_engine
                                            .run_sub_agent(
                                                &bg_prompt,
                                                &bg_desc,
                                                bg_type.as_deref(),
                                                bg_model.as_deref(),
                                                None, // background agents don't support fork
                                                &bg_wd,
                                                &bg_wd,
                                            )
                                            .await;

                                        // Notify completion
                                        let _ = bg_event_tx.send(Event::Notification(
                                            crate::event::Notification {
                                                source: crate::event::NotificationSource::GitHub, // reuse existing variant
                                                title: format!("Agent complete: {bg_desc}"),
                                                body: if result.len() > 200 {
                                                    format!("{}...", &result[..200])
                                                } else {
                                                    result
                                                },
                                                url: None,
                                                timestamp: chrono::Utc::now(),
                                            },
                                        ));

                                        tracing::info!("Background agent {bg_agent_id} completed");
                                    });

                                    let msg = format!(
                                        "Background agent launched: {description} (ID: {agent_id}). \
                                         You'll be notified when it completes."
                                    );
                                    let _ = event_tx.send(Event::ToolResult {
                                        session_id: session_id_owned.clone(),
                                        call_id: tool_call.id.clone(),
                                        output: msg.clone(),
                                        is_error: false,
                                    });
                                    tool_results.push(serde_json::json!({
                                        "type": "tool_result",
                                        "tool_use_id": tool_call.id,
                                        "content": msg,
                                        "is_error": false,
                                    }));
                                } else {
                                    // Sync agent: optionally with worktree isolation
                                    let use_worktree =
                                        tool_call.input["isolation"].as_str() == Some("worktree");

                                    let (agent_wd, worktree_info) = if use_worktree {
                                        let agent_id = uuid::Uuid::new_v4()
                                            .to_string()
                                            .split('-')
                                            .next()
                                            .unwrap_or("0")
                                            .to_string();
                                        match crate::worktree::create_agent_worktree(
                                            &working_dir,
                                            &agent_id,
                                        )
                                        .await
                                        {
                                            Ok((wt_dir, branch)) => {
                                                tracing::info!(
                                                    "Agent worktree created: {wt_dir} ({branch})"
                                                );
                                                (wt_dir, Some((agent_id, branch)))
                                            }
                                            Err(e) => {
                                                tracing::warn!(
                                                    "Worktree creation failed, using main dir: {e}"
                                                );
                                                (working_dir.clone(), None)
                                            }
                                        }
                                    } else {
                                        (working_dir.clone(), None)
                                    };

                                    let result = self
                                        .run_sub_agent(
                                            prompt,
                                            description,
                                            subagent_type,
                                            model_override,
                                            fork_msgs.as_deref(),
                                            &working_dir,
                                            &agent_wd,
                                        )
                                        .await;

                                    // Handle worktree cleanup
                                    let final_result = if let Some((agent_id, branch)) =
                                        &worktree_info
                                    {
                                        let has_changes =
                                            crate::worktree::worktree_has_changes(&agent_wd).await;
                                        if has_changes {
                                            format!(
                                                "{result}\n\n---\nAgent made changes in worktree. \
                                                 Branch: {branch}, Path: {agent_wd}"
                                            )
                                        } else {
                                            // No changes — clean up
                                            let _ = crate::worktree::remove_agent_worktree(
                                                &working_dir,
                                                &agent_wd,
                                                branch,
                                                true,
                                            )
                                            .await;
                                            tracing::info!(
                                                "Agent {agent_id} worktree cleaned up (no changes)"
                                            );
                                            result
                                        }
                                    } else {
                                        result
                                    };

                                    let _ = event_tx.send(Event::ToolResult {
                                        session_id: session_id_owned.clone(),
                                        call_id: tool_call.id.clone(),
                                        output: final_result.clone(),
                                        is_error: false,
                                    });

                                    tool_results.push(serde_json::json!({
                                        "type": "tool_result",
                                        "tool_use_id": tool_call.id,
                                        "content": final_result,
                                        "is_error": false,
                                    }));
                                }
                                continue;
                            }

                            // Intercept plan mode tools
                            if tool_call.name == "enter_plan_mode" {
                                let mut s = state_clone.write().await;
                                s.plan_mode = true;
                                tool_results.push(serde_json::json!({
                                    "type": "tool_result",
                                    "tool_use_id": tool_call.id,
                                    "content": "Plan mode activated.",
                                    "is_error": false,
                                }));
                                continue;
                            }
                            if tool_call.name == "exit_plan_mode" {
                                let mut s = state_clone.write().await;
                                s.plan_mode = false;
                                let summary = tool_call.input["plan_summary"]
                                    .as_str()
                                    .unwrap_or("Plan complete.");
                                tool_results.push(serde_json::json!({
                                    "type": "tool_result",
                                    "tool_use_id": tool_call.id,
                                    "content": format!("Plan mode deactivated. {summary}"),
                                    "is_error": false,
                                }));
                                continue;
                            }

                            // Intercept MCP resource tools
                            if tool_call.name == "list_mcp_resources" {
                                let mcp = self.mcp_manager.lock().await;
                                let resources = mcp.list_resources().await;
                                let output = if resources.is_empty() {
                                    "No MCP resources available.".to_string()
                                } else {
                                    let mut lines = vec![format!("{} resources:", resources.len())];
                                    for r in &resources {
                                        lines.push(format!(
                                            "  {} — {} [{}] ({})",
                                            r.uri, r.name, r.mime_type, r.server_name
                                        ));
                                        if !r.description.is_empty() {
                                            lines.push(format!("    {}", r.description));
                                        }
                                    }
                                    lines.join("\n")
                                };
                                tool_results.push(serde_json::json!({
                                    "type": "tool_result",
                                    "tool_use_id": tool_call.id,
                                    "content": output,
                                    "is_error": false,
                                }));
                                continue;
                            }
                            if tool_call.name == "read_mcp_resource" {
                                let uri = tool_call.input["uri"].as_str().unwrap_or("");
                                let mcp = self.mcp_manager.lock().await;
                                let result = mcp.read_resource(uri).await;
                                let (content, is_error) = match result {
                                    Ok(text) => (text, false),
                                    Err(e) => (format!("Error reading resource: {e}"), true),
                                };
                                tool_results.push(serde_json::json!({
                                    "type": "tool_result",
                                    "tool_use_id": tool_call.id,
                                    "content": content,
                                    "is_error": is_error,
                                }));
                                continue;
                            }

                            // Intercept cron tools
                            if tool_call.name == "cron_create" {
                                let cron_expr =
                                    tool_call.input["cron"].as_str().unwrap_or("*/5 * * * *");
                                let prompt = tool_call.input["prompt"].as_str().unwrap_or("");
                                let recurring =
                                    tool_call.input["recurring"].as_bool().unwrap_or(true);

                                let mut s = state_clone.write().await;
                                let id = s.cron.create(cron_expr, prompt, recurring);
                                let kind = if recurring { "recurring" } else { "one-shot" };
                                let msg = format!(
                                    "Scheduled {kind} job {id} ({cron_expr}): \"{prompt}\""
                                );

                                tool_results.push(serde_json::json!({
                                    "type": "tool_result",
                                    "tool_use_id": tool_call.id,
                                    "content": msg,
                                    "is_error": false,
                                }));
                                continue;
                            }
                            if tool_call.name == "cron_delete" {
                                let job_id = tool_call.input["job_id"].as_str().unwrap_or("");
                                let mut s = state_clone.write().await;
                                let deleted = s.cron.delete(job_id);
                                let msg = if deleted {
                                    format!("Deleted job {job_id}")
                                } else {
                                    format!("Job not found: {job_id}")
                                };
                                tool_results.push(serde_json::json!({
                                    "type": "tool_result",
                                    "tool_use_id": tool_call.id,
                                    "content": msg,
                                    "is_error": !deleted,
                                }));
                                continue;
                            }
                            if tool_call.name == "cron_list" {
                                let s = state_clone.read().await;
                                let summary = s.cron.summary();
                                tool_results.push(serde_json::json!({
                                    "type": "tool_result",
                                    "tool_use_id": tool_call.id,
                                    "content": summary,
                                    "is_error": false,
                                }));
                                continue;
                            }

                            // Intercept Skill tool — invoke a skill (slash command) by name
                            if tool_call.name == "Skill" {
                                let skill_name = tool_call.input["skill"].as_str().unwrap_or("");
                                let args = tool_call.input["args"].as_str().unwrap_or("");

                                let project_dir = {
                                    let s = state_clone.read().await;
                                    s.active_session()
                                        .map(|s| s.project_path.clone())
                                        .unwrap_or_else(|| ".".to_string())
                                };

                                let skills = crate::skills::load_skills(&project_dir);
                                if let Some(skill) = skills.iter().find(|s| s.name == skill_name) {
                                    let prompt = crate::skills::prepare_skill_prompt(
                                        skill,
                                        args,
                                        &project_dir,
                                    );
                                    tool_results.push(serde_json::json!({
                                        "type": "tool_result",
                                        "tool_use_id": tool_call.id,
                                        "content": format!("Skill /{skill_name} loaded. Follow these instructions:\n\n{prompt}"),
                                        "is_error": false,
                                    }));
                                } else {
                                    tool_results.push(serde_json::json!({
                                        "type": "tool_result",
                                        "tool_use_id": tool_call.id,
                                        "content": format!("Skill '{skill_name}' not found. Available skills can be listed with /skills."),
                                        "is_error": true,
                                    }));
                                }
                                continue;
                            }

                            let decision = self
                                .permission_engine
                                .check(&tool_call.name, &tool_call.input);

                            // Log warning if present (even for allowed tools)
                            if let Some(ref warning) = decision.warning {
                                tracing::warn!("{}", warning);
                            }

                            match decision.behavior {
                                PermissionBehavior::Deny => {
                                    let deny_msg = format!(
                                        "Tool use denied: {}. {}",
                                        tool_call.name, decision.reason
                                    );
                                    let _ = event_tx.send(Event::ToolDenied {
                                        session_id: session_id_owned.clone(),
                                        call_id: tool_call.id.clone(),
                                        tool_name: tool_call.name.clone(),
                                        reason: decision.reason.clone(),
                                        warning: decision.warning,
                                    });
                                    tool_results.push(serde_json::json!({
                                        "type": "tool_result",
                                        "tool_use_id": tool_call.id,
                                        "content": deny_msg,
                                        "is_error": true,
                                    }));
                                }
                                PermissionBehavior::Ask => {
                                    // Build input summary for the prompt
                                    let input_summary =
                                        crate::permissions::PermissionEngine::extract_input_context(
                                            &tool_call.name,
                                            &tool_call.input,
                                        );

                                    // Create oneshot channel for the response
                                    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();

                                    // Store the response channel in shared state
                                    {
                                        let s = state_clone.read().await;
                                        *s.pending_permission.lock().unwrap() =
                                            Some((tool_call.id.clone(), resp_tx));
                                    }

                                    // Emit permission prompt for TUI
                                    let _ = event_tx.send(Event::PermissionPrompt {
                                        session_id: session_id_owned.clone(),
                                        call_id: tool_call.id.clone(),
                                        tool_name: tool_call.name.clone(),
                                        input_summary: input_summary.clone(),
                                        warning: decision.warning.clone(),
                                    });

                                    // Wait for user response (with timeout)
                                    let response = tokio::time::timeout(
                                        std::time::Duration::from_secs(120),
                                        resp_rx,
                                    )
                                    .await;

                                    // Clear pending state
                                    {
                                        let s = state_clone.read().await;
                                        let _ = s.pending_permission.lock().unwrap().take();
                                    }

                                    // Resolve the response
                                    let user_decision = match response {
                                        Ok(Ok(r)) => Some(r),
                                        _ => None, // Timeout or channel error
                                    };

                                    match user_decision {
                                        Some(crate::event::PermissionResponse::Allow) => {
                                            let _ = event_tx.send(Event::ToolRequest {
                                                session_id: session_id_owned.clone(),
                                                tool_name: tool_call.name.clone(),
                                                input: tool_call.input.clone(),
                                                call_id: tool_call.id.clone(),
                                            });
                                            allowed_indices.push(tool_results.len());
                                            tool_results.push(serde_json::Value::Null);
                                            allowed_calls.push(tool_call);
                                        }
                                        Some(crate::event::PermissionResponse::AlwaysAllow) => {
                                            self.permission_engine.add_session_rule(
                                                &tool_call.name,
                                                None,
                                                PermissionBehavior::Allow,
                                            );
                                            let _ = event_tx.send(Event::ToolRequest {
                                                session_id: session_id_owned.clone(),
                                                tool_name: tool_call.name.clone(),
                                                input: tool_call.input.clone(),
                                                call_id: tool_call.id.clone(),
                                            });
                                            allowed_indices.push(tool_results.len());
                                            tool_results.push(serde_json::Value::Null);
                                            allowed_calls.push(tool_call);
                                        }
                                        Some(crate::event::PermissionResponse::Deny) => {
                                            let deny_msg = format!(
                                                "Tool use denied by user: {}",
                                                tool_call.name
                                            );
                                            let _ = event_tx.send(Event::ToolDenied {
                                                session_id: session_id_owned.clone(),
                                                call_id: tool_call.id.clone(),
                                                tool_name: tool_call.name.clone(),
                                                reason: "Denied by user".to_string(),
                                                warning: None,
                                            });
                                            tool_results.push(serde_json::json!({
                                                "type": "tool_result",
                                                "tool_use_id": tool_call.id,
                                                "content": deny_msg,
                                                "is_error": true,
                                            }));
                                        }
                                        Some(crate::event::PermissionResponse::AlwaysDeny) => {
                                            self.permission_engine.add_session_rule(
                                                &tool_call.name,
                                                None,
                                                PermissionBehavior::Deny,
                                            );
                                            let deny_msg = format!(
                                                "Tool use denied by user: {}",
                                                tool_call.name
                                            );
                                            let _ = event_tx.send(Event::ToolDenied {
                                                session_id: session_id_owned.clone(),
                                                call_id: tool_call.id.clone(),
                                                tool_name: tool_call.name.clone(),
                                                reason: "Always denied by user".to_string(),
                                                warning: None,
                                            });
                                            tool_results.push(serde_json::json!({
                                                "type": "tool_result",
                                                "tool_use_id": tool_call.id,
                                                "content": deny_msg,
                                                "is_error": true,
                                            }));
                                        }
                                        None => {
                                            // Timeout or channel error — treat as deny
                                            let deny_msg =
                                                "Tool use timed out waiting for approval"
                                                    .to_string();
                                            tool_results.push(serde_json::json!({
                                                "type": "tool_result",
                                                "tool_use_id": tool_call.id,
                                                "content": deny_msg,
                                                "is_error": true,
                                            }));
                                        }
                                    }
                                }
                                PermissionBehavior::Allow => {
                                    let _ = event_tx.send(Event::ToolRequest {
                                        session_id: session_id_owned.clone(),
                                        tool_name: tool_call.name.clone(),
                                        input: tool_call.input.clone(),
                                        call_id: tool_call.id.clone(),
                                    });
                                    allowed_indices.push(tool_results.len());
                                    tool_results.push(serde_json::Value::Null); // placeholder
                                    allowed_calls.push(tool_call);
                                }
                            }
                        }

                        // Execute with smart concurrency (matches CC):
                        // - Read-only tools (file_read, grep, glob) run in parallel
                        // - Write tools (file_write, file_edit, bash) run exclusively
                        // - Bash errors cascade and abort remaining sibling tools
                        // - Max concurrency: 10
                        const MAX_CONCURRENCY: usize = 10;
                        let read_only = ["file_read", "grep", "glob"];

                        // Partition into batches: consecutive read-only → parallel batch,
                        // any write tool → exclusive batch of 1
                        struct Batch<'a> {
                            calls: Vec<&'a crate::provider::ToolCall>,
                            indices: Vec<usize>,
                            concurrent: bool,
                        }

                        let mut batches: Vec<Batch<'_>> = Vec::new();
                        for (tc, &idx) in allowed_calls.iter().zip(allowed_indices.iter()) {
                            let is_read_only = read_only.contains(&tc.name.as_str());
                            if is_read_only {
                                // Append to current concurrent batch, or start new one
                                if let Some(last) = batches.last_mut()
                                    && last.concurrent
                                    && last.calls.len() < MAX_CONCURRENCY
                                {
                                    last.calls.push(tc);
                                    last.indices.push(idx);
                                    continue;
                                }
                                batches.push(Batch {
                                    calls: vec![tc],
                                    indices: vec![idx],
                                    concurrent: true,
                                });
                            } else {
                                // Write/bash tool → exclusive batch
                                batches.push(Batch {
                                    calls: vec![tc],
                                    indices: vec![idx],
                                    concurrent: false,
                                });
                            }
                        }

                        let mut bash_error_cascade = false;

                        for batch in &batches {
                            if bash_error_cascade {
                                // Abort remaining tools after a bash error
                                for (&tc, &idx) in batch.calls.iter().zip(batch.indices.iter()) {
                                    let abort_msg =
                                        "Aborted: previous bash command failed".to_string();
                                    let _ = event_tx.send(Event::ToolResult {
                                        session_id: session_id_owned.clone(),
                                        call_id: tc.id.clone(),
                                        output: abort_msg.clone(),
                                        is_error: true,
                                    });
                                    tool_results[idx] = serde_json::json!({
                                        "type": "tool_result",
                                        "tool_use_id": tc.id,
                                        "content": abort_msg,
                                        "is_error": true,
                                    });
                                }
                                continue;
                            }

                            // Helper: execute a single tool (built-in or MCP)
                            let execute_one =
                                |tc: &crate::provider::ToolCall,
                                 exec: ToolExecutor,
                                 mcp: Arc<tokio::sync::Mutex<McpManager>>,
                                 wd: String| {
                                    let name = tc.name.clone();
                                    let input = tc.input.clone();
                                    async move {
                                        if McpManager::is_mcp_tool(&name) {
                                            let mgr = mcp.lock().await;
                                            match mgr.call_tool(&name, input).await {
                                                Ok(output) => ToolExecResult {
                                                    output,
                                                    is_error: false,
                                                },
                                                Err(e) => ToolExecResult {
                                                    output: format!("MCP error: {e}"),
                                                    is_error: true,
                                                },
                                            }
                                        } else {
                                            exec(name, input, wd).await
                                        }
                                    }
                                };

                            let mcp_ref = self.mcp_manager.clone();

                            if batch.concurrent && batch.calls.len() > 1 {
                                // Execute read-only batch in parallel
                                let futs: Vec<_> = batch
                                    .calls
                                    .iter()
                                    .map(|tc| {
                                        execute_one(
                                            tc,
                                            executor.clone(),
                                            mcp_ref.clone(),
                                            working_dir.clone(),
                                        )
                                    })
                                    .collect();
                                let results = futures_util::future::join_all(futs).await;

                                for ((tc, result), &idx) in batch
                                    .calls
                                    .iter()
                                    .zip(results.iter())
                                    .zip(batch.indices.iter())
                                {
                                    let _ = event_tx.send(Event::ToolResult {
                                        session_id: session_id_owned.clone(),
                                        call_id: tc.id.clone(),
                                        output: result.output.clone(),
                                        is_error: result.is_error,
                                    });
                                    tool_results[idx] = serde_json::json!({
                                        "type": "tool_result",
                                        "tool_use_id": tc.id,
                                        "content": result.output,
                                        "is_error": result.is_error,
                                    });
                                }
                            } else {
                                // Execute exclusively (single tool or single read-only)
                                for (&tc, &idx) in batch.calls.iter().zip(batch.indices.iter()) {
                                    // Run PreToolUse hooks
                                    let input_str =
                                        serde_json::to_string(&tc.input).unwrap_or_default();
                                    if !self.hooks.pre_tool_use.is_empty() {
                                        let hook_results = crate::settings::execute_hooks(
                                            &self.hooks.pre_tool_use,
                                            Some(&tc.name),
                                            Some(&input_str),
                                            &working_dir,
                                        )
                                        .await;
                                        for (output, is_err) in &hook_results {
                                            if *is_err && !output.is_empty() {
                                                tracing::warn!("PreToolUse hook error: {output}");
                                            }
                                        }
                                    }

                                    let result = execute_one(
                                        tc,
                                        executor.clone(),
                                        mcp_ref.clone(),
                                        working_dir.clone(),
                                    )
                                    .await;

                                    // Run PostToolUse hooks
                                    if !self.hooks.post_tool_use.is_empty() {
                                        let _ = crate::settings::execute_hooks(
                                            &self.hooks.post_tool_use,
                                            Some(&tc.name),
                                            Some(&input_str),
                                            &working_dir,
                                        )
                                        .await;
                                    }

                                    // Bash errors cascade to abort siblings
                                    if tc.name == "bash" && result.is_error {
                                        bash_error_cascade = true;
                                    }

                                    let _ = event_tx.send(Event::ToolResult {
                                        session_id: session_id_owned.clone(),
                                        call_id: tc.id.clone(),
                                        output: result.output.clone(),
                                        is_error: result.is_error,
                                    });
                                    tool_results[idx] = serde_json::json!({
                                        "type": "tool_result",
                                        "tool_use_id": tc.id,
                                        "content": result.output,
                                        "is_error": result.is_error,
                                    });
                                }
                            }
                        }

                        // Build the assistant message (with tool_use blocks)
                        let mut assistant_content: Vec<serde_json::Value> = Vec::new();
                        if !response.content.is_empty() {
                            assistant_content.push(serde_json::json!({
                                "type": "text",
                                "text": response.content,
                            }));
                        }
                        for tc in &response.tool_calls {
                            assistant_content.push(serde_json::json!({
                                "type": "tool_use",
                                "id": tc.id,
                                "name": tc.name,
                                "input": tc.input,
                            }));
                        }

                        // Append assistant + tool_result messages for next turn
                        current_messages.push(Message {
                            role: Role::Assistant,
                            content: serde_json::to_string(&assistant_content).unwrap_or_default(),
                        });
                        current_messages.push(Message {
                            role: Role::User,
                            content: serde_json::to_string(&tool_results).unwrap_or_default(),
                        });

                        // Don't start a new assistant turn here — the next
                        // iteration's streaming callback will start one when
                        // text actually arrives. This prevents empty [one]
                        // headers in the TUI for tool-only rounds.

                        continue; // Loop back for next model call
                    }

                    // No executor → finalize
                    {
                        let mut state = state_clone.write().await;
                        if let Some(session) = state.sessions.get_mut(&session_id_owned) {
                            session.record_usage(
                                response.usage.input_tokens,
                                response.usage.output_tokens,
                            );
                            session.conversation.finish_current(Some(
                                response.usage.input_tokens + response.usage.output_tokens,
                            ));
                        }
                    }
                    let _ = event_tx.send(Event::AiResponseChunk {
                        session_id: session_id_owned,
                        content: String::new(),
                        done: true,
                    });
                    return;
                }
                Err(e) => {
                    let raw_error = format!("{e}");
                    tracing::debug!("AI provider error: {raw_error}");

                    let provider_name = self.provider.provider_name();
                    let error_msg = if raw_error.contains("401")
                        || raw_error.contains("403")
                        || raw_error.contains("authentication")
                        || raw_error.contains("Unauthorized")
                        || raw_error.contains("invalid_api_key")
                        || raw_error.contains("invalid x-api-key")
                    {
                        format!(
                            "Authentication failed for {provider_name}. Your API key may be \
                             invalid or expired.\n\n\
                             Use `/login {provider_name}` to sign in, or \
                             `/login {provider_name} <key>` to set a new API key."
                        )
                    } else if raw_error.contains("connect")
                        || raw_error.contains("dns")
                        || raw_error.contains("timed out")
                        || raw_error.contains("Connection refused")
                    {
                        format!(
                            "Network error: could not reach the {provider_name} API.\n\n\
                             {raw_error}"
                        )
                    } else {
                        format!("Error from {provider_name}: {raw_error}")
                    };

                    {
                        let mut state = state_clone.write().await;
                        if let Some(session) = state.sessions.get_mut(&session_id_owned) {
                            if let Some(last) = session.conversation.turns.last_mut() {
                                if last.role == crate::conversation::TurnRole::Assistant {
                                    last.content = error_msg.clone();
                                    last.is_streaming = false;
                                }
                            } else {
                                session.conversation.start_assistant_response();
                                session.conversation.append_to_current(&error_msg);
                                session.conversation.finish_current(None);
                            }
                        }
                    }

                    let _ = event_tx.send(Event::AiResponseChunk {
                        session_id: session_id_owned,
                        content: error_msg,
                        done: true,
                    });
                    return; // Exit on error
                }
            }
        }

        // Max tool turns reached — finalize
        let _ = event_tx.send(Event::AiResponseChunk {
            session_id: session_id_owned,
            content: String::new(),
            done: true,
        });
    }

    /// Classify user intent to select the right agent and tool subset.
    fn classify_intent(&self, message: &str) -> Option<AgentRole> {
        let lower = message.to_lowercase();

        // Writer signals
        let writer_keywords = [
            "change",
            "edit",
            "modify",
            "update",
            "write",
            "create",
            "add",
            "fix",
            "refactor",
            "rename",
            "replace",
            "implement",
            "delete line",
        ];
        if writer_keywords.iter().any(|k| lower.contains(k)) {
            return Some(AgentRole::Writer);
        }

        // Executor signals
        let exec_keywords = [
            "run", "execute", "test", "build", "install", "compile", "deploy", "npm", "cargo",
            "pip", "make", "docker",
        ];
        if exec_keywords.iter().any(|k| lower.contains(k)) {
            return Some(AgentRole::Executor);
        }

        // Explorer signals
        let explorer_keywords = [
            "find",
            "search",
            "where",
            "locate",
            "which file",
            "grep",
            "how many",
            "list all",
            "show all",
        ];
        if explorer_keywords.iter().any(|k| lower.contains(k)) {
            return Some(AgentRole::Explorer);
        }

        // Reader signals
        let reader_keywords = [
            "read",
            "show",
            "what does",
            "explain",
            "how does",
            "look at",
            "print",
            "display",
            "cat",
            "open",
        ];
        if reader_keywords.iter().any(|k| lower.contains(k)) {
            return Some(AgentRole::Reader);
        }

        // Ambiguous — use all tools
        None
    }

    fn build_system_prompt(
        &self,
        session: &crate::session::Session,
        agent_role: Option<AgentRole>,
    ) -> String {
        // Start with CLAUDE.md-based system prompt
        // Include deferred tool names and model context
        let deferred_refs: Vec<&str> = self
            .deferred_tool_names
            .iter()
            .map(|s| s.as_str())
            .collect();
        let model_name = if self.model_config.model.is_empty() {
            None
        } else {
            Some(self.model_config.model.as_str())
        };
        let mut prompt = crate::system_prompt::build_with_context(
            &session.project_path,
            &deferred_refs,
            model_name,
        );

        // Inject evergreen recall context if available
        if let Some(ref recall) = session.evergreen_context {
            prompt.push_str(&format!("\n\n{recall}"));
            // Count tiers present in the recall block for debug visibility.
            let hot = recall.matches("--- HOT").count();
            let warm = recall.matches("--- WARM").count();
            let cold = recall.matches("--- COLD").count();
            let _ = self.event_tx.send(Event::DebugLog {
                session_id: session.id.clone(),
                message: format!(
                    "recall: injecting {} hot · {} warm · {} cold chunks into system prompt",
                    hot, warm, cold,
                ),
            });
        }

        // Add agent-specific role instructions if routed
        if let Some(role) = agent_role {
            prompt.push_str(&format!("\n\n# Agent Role\n\n{}", role.system_prompt()));
        }

        // Filter tool schemas based on agent role
        let schemas = match agent_role {
            Some(role) => {
                let role_name = match role {
                    AgentRole::Reader => "reader",
                    AgentRole::Writer => "writer",
                    AgentRole::Executor => "executor",
                    AgentRole::Explorer => "explorer",
                    AgentRole::Coordinator => "coordinator",
                };
                self.agent_registry
                    .filter_schemas(role_name, &self.tool_schemas)
            }
            None => self.tool_schemas.clone(),
        };

        if !schemas.is_empty() {
            prompt.push_str("\n\n## Available Tools\n\n");
            for schema in &schemas {
                if let Some(name) = schema["name"].as_str()
                    && let Some(desc) = schema["description"].as_str()
                {
                    prompt.push_str(&format!("- **{name}**: {desc}\n"));
                }
            }
        }

        prompt
    }

    /// Run a sub-agent: creates a fresh conversation with filtered tools,
    /// executes a tool loop, and returns the result as a string.
    /// This is the v1 implementation — sync only, no fork/background/worktree.
    #[allow(clippy::too_many_arguments)]
    async fn run_sub_agent(
        &self,
        prompt: &str,
        description: &str,
        subagent_type: Option<&str>,
        model_override: Option<&str>,
        fork_messages: Option<&[Message]>,
        _project_path: &str,
        working_dir: &str,
    ) -> String {
        const MAX_AGENT_TURNS: usize = 50;

        // Determine agent role from subagent_type or description
        let agent_role = subagent_type.and_then(|t| match t {
            "Explore" | "explorer" => Some(AgentRole::Explorer),
            "Plan" | "plan" => Some(AgentRole::Reader),
            "general-purpose" | "general" => None,
            _ => None,
        });

        // Build a minimal system prompt for the sub-agent
        let role_desc = match agent_role {
            Some(role) => format!(
                "\n\nYou are a sub-agent with role: {:?}. {}",
                role,
                role.system_prompt()
            ),
            None => String::new(),
        };

        let system_prompt = format!(
            "You are a focused sub-agent. Complete the task described below and return a concise result. \
             Do not ask questions — make your best judgment. \
             Working directory: {working_dir}{role_desc}"
        );

        // Filter tool schemas based on agent role
        let schemas = match agent_role {
            Some(role) => {
                let role_name = match role {
                    AgentRole::Reader => "reader",
                    AgentRole::Writer => "writer",
                    AgentRole::Executor => "executor",
                    AgentRole::Explorer => "explorer",
                    AgentRole::Coordinator => "coordinator",
                };
                self.agent_registry
                    .filter_schemas(role_name, &self.tool_schemas)
            }
            None => self.tool_schemas.clone(),
        };

        // Remove the Agent tool from sub-agent schemas to prevent recursion
        // (Used when creating per-agent providers in future versions)
        let _schemas: Vec<serde_json::Value> = schemas
            .into_iter()
            .filter(|s| s["name"].as_str() != Some("Agent"))
            .collect();

        // Build initial messages
        let mut messages = if let Some(parent_msgs) = fork_messages {
            // Fork mode: inherit parent conversation + add child directive
            let mut msgs = parent_msgs.to_vec();
            msgs.push(Message {
                role: Role::User,
                content: format!(
                    "[Sub-agent directive] You are now a focused sub-agent. \
                     Complete this specific task using the conversation above as context:\n\n{prompt}"
                ),
            });
            msgs
        } else {
            // Normal mode: fresh conversation
            vec![
                Message {
                    role: Role::System,
                    content: system_prompt,
                },
                Message {
                    role: Role::User,
                    content: prompt.to_string(),
                },
            ]
        };

        // Apply model override if specified, then resolve effort for the sub-agent.
        // Sub-agents always use at least Medium complexity — they're never trivial queries.
        let agent_config = {
            let mut config = if let Some(model_shortcut) = model_override {
                let resolved = crate::provider::resolve_model_shortcut(model_shortcut);
                let mut c = self.model_config.clone();
                c.model = resolved.to_string();
                tracing::info!("Sub-agent using model override: {resolved}");
                c
            } else {
                self.model_config.clone()
            };
            // Read effort from the active session (if any), default to auto/medium for agents
            let effort = {
                let state = self.state.read().await;
                state
                    .active_session()
                    .and_then(|s| s.effort.as_deref())
                    .map(str::to_string)
            };
            let caps = model_capabilities(&config.model);
            let resolved = resolve_effort(effort.as_deref(), &caps, 0.0, MessageComplexity::Medium);
            config.max_tokens = resolved.max_tokens;
            config.budget_tokens = (resolved.budget_tokens > 0).then_some(resolved.budget_tokens);
            config
        };

        tracing::info!(
            "Sub-agent started: {} (role: {:?}, model: {})",
            description,
            agent_role,
            agent_config.model
        );

        // Mini tool execution loop
        for _turn in 0..MAX_AGENT_TURNS {
            let response = match self.provider.send_message(&messages, &agent_config).await {
                Ok(r) => r,
                Err(e) => {
                    return format!("Sub-agent error: {e}");
                }
            };

            // No tool calls → return the text response
            if response.tool_calls.is_empty() {
                return response.content;
            }

            // Execute tool calls
            let mut assistant_content: Vec<serde_json::Value> = Vec::new();
            if !response.content.is_empty() {
                assistant_content.push(serde_json::json!({
                    "type": "text",
                    "text": response.content,
                }));
            }

            let mut tool_result_blocks: Vec<serde_json::Value> = Vec::new();

            for tc in &response.tool_calls {
                assistant_content.push(serde_json::json!({
                    "type": "tool_use",
                    "id": tc.id,
                    "name": tc.name,
                    "input": tc.input,
                }));

                // Execute tool
                let result = if let Some(ref executor) = self.tool_executor {
                    if McpManager::is_mcp_tool(&tc.name) {
                        let mgr = self.mcp_manager.lock().await;
                        match mgr.call_tool(&tc.name, tc.input.clone()).await {
                            Ok(output) => ToolExecResult {
                                output,
                                is_error: false,
                            },
                            Err(e) => ToolExecResult {
                                output: format!("MCP error: {e}"),
                                is_error: true,
                            },
                        }
                    } else {
                        executor(tc.name.clone(), tc.input.clone(), working_dir.to_string()).await
                    }
                } else {
                    ToolExecResult {
                        output: "No tool executor configured".to_string(),
                        is_error: true,
                    }
                };

                tool_result_blocks.push(serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": tc.id,
                    "content": result.output,
                    "is_error": result.is_error,
                }));
            }

            // Append messages for next turn
            messages.push(Message {
                role: Role::Assistant,
                content: serde_json::to_string(&assistant_content).unwrap_or_default(),
            });
            messages.push(Message {
                role: Role::User,
                content: serde_json::to_string(&tool_result_blocks).unwrap_or_default(),
            });
        }

        "Sub-agent reached maximum turn limit.".to_string()
    }
}
