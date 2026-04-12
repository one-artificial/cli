use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Global counter — incremented for every ToolRequest so consecutive tool
/// calls on the same tick get different spinner styles.
static STYLE_COUNTER: AtomicUsize = AtomicUsize::new(0);

use anyhow::Result;
use crossterm::{
    event::{
        self, Event as CrosstermEvent, KeyCode, KeyModifiers, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Tabs as RatatuiTabs, Wrap},
};
use tokio::sync::broadcast;

use one_core::conversation::TurnRole;
use one_core::event::Event;
use one_core::state::SharedState;

use crate::autocomplete::Autocomplete;
use crate::input::InputState;
use crate::pet::Pet;
use crate::tabs::TabManager;

/// Active permission prompt shown to the user.
struct PermissionPromptState {
    tool_name: String,
    input_summary: String,
    warning: Option<String>,
    /// Currently selected option (0=Yes, 1=Always, 2=No)
    selected: u8,
}

/// Provider picker shown when /new is used with multiple configured providers.
struct ProviderPickerState {
    /// Project path for the new session
    project_path: String,
    /// Available providers that have credentials configured
    providers: Vec<ConfiguredProvider>,
    /// Currently selected index
    selected: usize,
}

/// Session import picker — lists sessions from all CLI backends.
struct ImportPickerState {
    sessions: Vec<one_core::storage::SessionInfo>,
    selected: usize,
    /// Status line shown at the bottom of the dialog ("Imported 12 turns", etc.)
    status: Option<String>,
}

/// A provider that has been detected as configured (has credentials).
struct ConfiguredProvider {
    provider: one_core::provider::Provider,
    model: String,
    label: &'static str,
    status: String,
}

pub struct App {
    state: SharedState,
    event_tx: broadcast::Sender<Event>,
    event_rx: broadcast::Receiver<Event>,
    tabs: TabManager,
    input: InputState,
    autocomplete: Autocomplete,
    pet: Pet,
    messages_scroll: u16,
    /// Tracks when Ctrl+C was last pressed for double-tap exit
    last_ctrl_c: Option<std::time::Instant>,
    /// Active permission prompt waiting for user input
    permission_prompt: Option<PermissionPromptState>,
    /// Provider picker for /new when multiple providers configured
    provider_picker: Option<ProviderPickerState>,
    /// Session import picker (/import command)
    import_picker: Option<ImportPickerState>,
    /// Spinner animation tick (cycles through frames)
    spinner_tick: u8,
    /// Pending close-session confirmation (true = waiting for y/n)
    close_confirm: bool,
    /// When the current stream started (for elapsed time display)
    stream_started: Option<std::time::Instant>,
    /// Index into thinking verbs — rotates every ~4 seconds
    thinking_verb_idx: usize,
    /// Index into tips — rotates every ~8 seconds
    tip_idx: usize,
    /// When the current tool execution started (for elapsed time display)
    tool_exec_started: Option<std::time::Instant>,
    /// Which processing-dot animation style is active for the current tool (0–3).
    /// Picked from spinner_tick when ToolRequest fires so it varies between calls.
    processing_dot_style: usize,
    /// Bash mode — triggered by typing '!', input is a shell command
    bash_mode: bool,
    /// Show inline help when user types '?' alone
    help_open: bool,
    /// Transcript mode: full-screen conversation view (Ctrl+O)
    transcript_mode: bool,
    /// In transcript mode, show all messages (Ctrl+E) vs truncated
    transcript_show_all: bool,
    /// Scroll position within transcript mode
    transcript_scroll: u16,
    /// Sessions whose tab title has already been derived from first response
    named_sessions: std::collections::HashSet<String>,
    /// ONE.md sidebar open (Ctrl+B toggle)
    onemed_open: bool,
    /// Cached content of the active project's ONE.md
    onemed_content: Option<String>,
}

impl App {
    pub fn new(state: SharedState, event_tx: broadcast::Sender<Event>) -> Self {
        let event_rx = event_tx.subscribe();
        Self {
            state,
            event_tx,
            event_rx,
            tabs: TabManager::new(),
            input: InputState::new(),
            autocomplete: Autocomplete::new(),
            pet: Pet::new("Pixel".to_string(), "duck", true),
            messages_scroll: 0,
            last_ctrl_c: None,
            permission_prompt: None,
            provider_picker: None,
            import_picker: None,
            spinner_tick: 0,
            close_confirm: false,
            stream_started: None,
            tool_exec_started: None,
            processing_dot_style: 0,
            thinking_verb_idx: 0,
            tip_idx: 0,
            bash_mode: false,
            help_open: false,
            transcript_mode: false,
            transcript_show_all: false,
            transcript_scroll: 0,
            named_sessions: std::collections::HashSet::new(),
            onemed_open: false,
            onemed_content: None,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        // Initialize tabs from sessions that already exist in state.
        // SessionCreated events fire before App::new subscribes, so we
        // can't rely on events for the initial session list.
        {
            let state = self.state.read().await;
            for (id, session) in &state.sessions {
                self.tabs
                    .add_session(session.project_name.clone(), id.clone());
            }
            // Select the tab matching active_session_id
            if let Some(ref active_id) = state.active_session_id {
                while self.tabs.active_session_id() != Some(active_id.as_str()) {
                    self.tabs.select_next();
                }
            }
            self.pet = Pet::from_config(&state.config.pet);
        }

        enable_raw_mode()?;
        let mut stdout = io::stdout();
        crossterm::execute!(
            stdout,
            EnterAlternateScreen,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let result = self.event_loop(&mut terminal).await;

        disable_raw_mode()?;
        crossterm::execute!(
            terminal.backend_mut(),
            PopKeyboardEnhancementFlags,
            LeaveAlternateScreen
        )?;
        terminal.show_cursor()?;

        result
    }

    async fn event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> Result<()> {
        // Init pet from config if available
        {
            let state = self.state.read().await;
            self.pet = Pet::from_config(&state.config.pet);
        }

        loop {
            self.pet.tick();
            self.spinner_tick = self.spinner_tick.wrapping_add(1);

            // Rotate thinking verb every ~4s (80 ticks) and tip every ~8s (160 ticks)
            if self.stream_started.is_some() {
                if self.spinner_tick.is_multiple_of(80) {
                    self.thinking_verb_idx = self.thinking_verb_idx.wrapping_add(1);
                }
                if self.spinner_tick.is_multiple_of(160) {
                    self.tip_idx = self.tip_idx.wrapping_add(1);
                }
            }

            // Drain broadcast events FIRST so state is up-to-date before drawing
            while let Ok(evt) = self.event_rx.try_recv() {
                match evt {
                    Event::Quit => return Ok(()),
                    Event::AiResponseChunk {
                        ref session_id,
                        done,
                        ..
                    } => {
                        if done {
                            self.pet.on_response_complete();
                            self.messages_scroll = 0;
                            self.stream_started = None;
                            // Derive tab title from first AI response
                            let sid = session_id.clone();
                            if !self.named_sessions.contains(&sid) {
                                let maybe_derive = {
                                    let s = self.state.read().await;
                                    s.sessions.get(&sid).and_then(|sess| {
                                        if sess.conversation.turns.len() == 2 {
                                            let content = sess
                                                .conversation
                                                .turns
                                                .last()
                                                .map(|t| t.content.as_str())
                                                .unwrap_or("");
                                            let title = derive_tab_title(content);
                                            if !title.is_empty() {
                                                Some((title, sess.db_path.clone()))
                                            } else {
                                                None
                                            }
                                        } else {
                                            None
                                        }
                                    })
                                };
                                if let Some((title, db_path)) = maybe_derive {
                                    self.tabs.set_title(&sid, title.clone());
                                    self.named_sessions.insert(sid);
                                    if !db_path.as_os_str().is_empty() {
                                        drop(tokio::task::spawn_blocking(move || {
                                            if let Ok(db) = one_db::SessionDb::open(&db_path) {
                                                let _ = db.set_meta("tab_name", &title);
                                            }
                                        }));
                                    }
                                }
                            }
                        } else {
                            self.pet.on_response_start();
                            if self.stream_started.is_none() {
                                self.stream_started = Some(std::time::Instant::now());
                                // Pick new random-ish verb, tip, and dot style for this stream
                                self.thinking_verb_idx = self.spinner_tick as usize;
                                self.tip_idx = (self.spinner_tick as usize).wrapping_mul(7);
                                self.processing_dot_style =
                                    STYLE_COUNTER.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    Event::ToolRequest {
                        ref session_id,
                        ref tool_name,
                        ref input,
                        ref call_id,
                    } => {
                        self.pet.on_tool_call(tool_name);
                        let detail = match tool_name.as_str() {
                            "Read" => input["file_path"].as_str().unwrap_or("").to_string(),
                            "Write" => input["file_path"].as_str().unwrap_or("").to_string(),
                            "Edit" => input["file_path"].as_str().unwrap_or("").to_string(),
                            "Bash" => {
                                let cmd = input["command"].as_str().unwrap_or("");
                                if cmd.len() > 60 {
                                    format!("{}...", &cmd[..60])
                                } else {
                                    cmd.to_string()
                                }
                            }
                            "Grep" => input["pattern"].as_str().unwrap_or("").to_string(),
                            "Glob" => input["pattern"].as_str().unwrap_or("").to_string(),
                            _ => String::new(),
                        };
                        // Detect Bash cd commands and update session cwd
                        if tool_name == "Bash" {
                            let cmd = input["command"].as_str().unwrap_or("");
                            let trimmed = cmd.trim();
                            if trimmed == "cd"
                                || trimmed.starts_with("cd ")
                                || trimmed.starts_with("cd\t")
                            {
                                let target = trimmed.strip_prefix("cd").unwrap_or("").trim();
                                let mut state = self.state.write().await;
                                if let Some(session) = state.sessions.get_mut(session_id) {
                                    let base = session.cwd.clone();
                                    let new_dir = if target.is_empty() {
                                        dirs_next::home_dir()
                                            .map(|h| h.to_string_lossy().to_string())
                                            .unwrap_or(base)
                                    } else {
                                        let expanded = if target.starts_with('~') {
                                            dirs_next::home_dir()
                                                .map(|h| {
                                                    let rest = target
                                                        .strip_prefix("~/")
                                                        .unwrap_or(&target[1..]);
                                                    h.join(rest).to_string_lossy().to_string()
                                                })
                                                .unwrap_or(target.to_string())
                                        } else if std::path::Path::new(target).is_absolute() {
                                            target.to_string()
                                        } else {
                                            std::path::Path::new(&base)
                                                .join(target)
                                                .to_string_lossy()
                                                .to_string()
                                        };
                                        std::fs::canonicalize(&expanded)
                                            .map(|p| p.to_string_lossy().to_string())
                                            .unwrap_or(expanded)
                                    };
                                    if std::path::Path::new(&new_dir).is_dir() {
                                        session.cwd = new_dir;
                                    }
                                }
                                drop(state);
                            }
                        }

                        // Store on the LAST assistant turn so it renders per-turn
                        self.tool_exec_started = Some(std::time::Instant::now());
                        self.processing_dot_style = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.subsec_nanos() as usize)
                            .unwrap_or(self.spinner_tick as usize);
                        let mut state = self.state.write().await;
                        if let Some(session) = state.sessions.get_mut(session_id) {
                            // Set active tool for status bar
                            session.active_tool = Some(tool_name.clone());
                            if let Some(last) = session.conversation.turns.last_mut()
                                && last.role == TurnRole::Assistant
                            {
                                // Increment a global counter so consecutive tool
                                // calls always get distinct spinner styles.
                                let dot_style = STYLE_COUNTER.fetch_add(1, Ordering::Relaxed);
                                last.tool_calls
                                    .push(one_core::conversation::ToolCallRecord {
                                        id: call_id.clone(),
                                        tool_name: tool_name.clone(),
                                        input_summary: detail,
                                        output: None,
                                        is_error: false,
                                        duration_ms: None,
                                        dot_style,
                                    });
                            }
                        }
                    }
                    Event::ToolResult {
                        ref session_id,
                        ref call_id,
                        ref output,
                        is_error,
                    } => {
                        if is_error {
                            self.pet.on_error();
                        }
                        // Clear active tool in status bar
                        self.tool_exec_started = None;
                        let mut state = self.state.write().await;
                        if let Some(session) = state.sessions.get_mut(session_id) {
                            session.active_tool = None;
                        }
                        // Search all turns (not just the last) — the next assistant turn
                        // may already exist by the time this event fires.
                        if let Some(session) = state.sessions.get_mut(session_id) {
                            'find_tc: for turn in session.conversation.turns.iter_mut().rev() {
                                for tc in turn.tool_calls.iter_mut() {
                                    if tc.id == *call_id {
                                        tc.is_error = is_error;
                                        tc.output = Some(output.clone());
                                        break 'find_tc;
                                    }
                                }
                            }
                        }
                    }
                    Event::DebugLog {
                        ref session_id,
                        ref message,
                    } => {
                        let mut state = self.state.write().await;
                        // Empty session_id → route to whichever session is active.
                        let target_id = if session_id.is_empty() {
                            state.active_session_id.clone()
                        } else {
                            Some(session_id.clone())
                        };
                        if let Some(id) = target_id
                            && let Some(session) = state.sessions.get_mut(&id)
                        {
                            session
                                .debug_events
                                .push((chrono::Utc::now(), message.clone()));
                        }
                    }
                    Event::Notification(notif) => {
                        let mut state = self.state.write().await;
                        state.notifications.push(notif);
                    }
                    Event::SessionCreated {
                        ref session_id,
                        ref project,
                    } => {
                        self.tabs.add_session(project.clone(), session_id.clone());
                    }
                    Event::PermissionPrompt {
                        tool_name,
                        input_summary,
                        warning,
                        ..
                    } => {
                        self.permission_prompt = Some(PermissionPromptState {
                            tool_name,
                            input_summary,
                            warning,
                            selected: 0,
                        });
                    }
                    _ => {}
                }
            }

            // Now draw with fresh state
            let snapshot = self.state.read().await.clone();
            terminal.draw(|f| self.draw(f, &snapshot))?;

            if event::poll(std::time::Duration::from_millis(50))? {
                let crossterm_event = event::read()?;

                // Handle permission prompt input before anything else
                if self.permission_prompt.is_some() {
                    if let CrosstermEvent::Key(key) = crossterm_event {
                        let response = match key.code {
                            // Arrow / j/k navigation
                            KeyCode::Up | KeyCode::Char('k') => {
                                if let Some(ref mut p) = self.permission_prompt {
                                    p.selected = p.selected.saturating_sub(1);
                                }
                                None
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                if let Some(ref mut p) = self.permission_prompt {
                                    p.selected = (p.selected + 1).min(2);
                                }
                                None
                            }
                            // Number keys for direct selection
                            KeyCode::Char('1') => Some(one_core::event::PermissionResponse::Allow),
                            KeyCode::Char('2') => {
                                Some(one_core::event::PermissionResponse::AlwaysAllow)
                            }
                            KeyCode::Char('3') => Some(one_core::event::PermissionResponse::Deny),
                            // Letter shortcuts (CC compat)
                            KeyCode::Char('y') => Some(one_core::event::PermissionResponse::Allow),
                            KeyCode::Char('a') => {
                                Some(one_core::event::PermissionResponse::AlwaysAllow)
                            }
                            KeyCode::Char('n') => Some(one_core::event::PermissionResponse::Deny),
                            // Enter confirms selected option
                            KeyCode::Enter => {
                                let sel = self
                                    .permission_prompt
                                    .as_ref()
                                    .map(|p| p.selected)
                                    .unwrap_or(0);
                                Some(match sel {
                                    0 => one_core::event::PermissionResponse::Allow,
                                    1 => one_core::event::PermissionResponse::AlwaysAllow,
                                    _ => one_core::event::PermissionResponse::Deny,
                                })
                            }
                            // Esc = deny
                            KeyCode::Esc => Some(one_core::event::PermissionResponse::Deny),
                            _ => None,
                        };

                        if let Some(resp) = response {
                            let state = self.state.read().await;
                            if let Some((_, sender)) =
                                state.pending_permission.lock().unwrap().take()
                            {
                                let _ = sender.send(resp);
                            }
                            self.permission_prompt = None;
                        }
                    }
                    continue;
                }

                // Handle provider picker input
                if self.provider_picker.is_some() {
                    if let CrosstermEvent::Key(key) = crossterm_event {
                        match key.code {
                            KeyCode::Up | KeyCode::Char('k') => {
                                if let Some(ref mut p) = self.provider_picker {
                                    p.selected = p.selected.saturating_sub(1);
                                }
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                if let Some(ref mut p) = self.provider_picker {
                                    let max = p.providers.len().saturating_sub(1);
                                    if p.selected < max {
                                        p.selected += 1;
                                    }
                                }
                            }
                            KeyCode::Enter => {
                                if let Some(picker) = self.provider_picker.take() {
                                    let chosen = &picker.providers[picker.selected];
                                    let model_config = one_core::provider::ModelConfig {
                                        provider: chosen.provider,
                                        model: chosen.model.clone(),
                                        max_tokens: 8000,
                                        temperature: None,
                                        budget_tokens: None,
                                    };
                                    let session = one_core::session::Session::new(
                                        picker.project_path,
                                        model_config,
                                    );
                                    let sid = session.id.clone();
                                    let name = session.project_name.clone();
                                    {
                                        let mut s = self.state.write().await;
                                        s.sessions.insert(sid.clone(), session);
                                        s.active_session_id = Some(sid.clone());
                                    }
                                    self.tabs.add_session(name.clone(), sid.clone());
                                    while self.tabs.active_session_id() != Some(&sid) {
                                        self.tabs.select_next();
                                    }
                                    let _ = self.event_tx.send(Event::SessionCreated {
                                        session_id: sid,
                                        project: name,
                                    });
                                }
                            }
                            KeyCode::Esc => {
                                self.provider_picker = None;
                            }
                            _ => {}
                        }
                    }
                    continue;
                }

                // Handle import picker input
                if self.import_picker.is_some() {
                    if let CrosstermEvent::Key(key) = crossterm_event {
                        match key.code {
                            KeyCode::Up | KeyCode::Char('k') => {
                                if let Some(ref mut p) = self.import_picker {
                                    p.selected = p.selected.saturating_sub(1);
                                }
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                if let Some(ref mut p) = self.import_picker {
                                    let max = p.sessions.len().saturating_sub(1);
                                    if p.selected < max {
                                        p.selected += 1;
                                    }
                                }
                            }
                            KeyCode::Enter => {
                                if let Some(picker) = self.import_picker.take()
                                    && !picker.sessions.is_empty()
                                {
                                    let info = &picker.sessions[picker.selected];
                                    let session_id = info.session_id.clone();
                                    let backend =
                                        one_core::storage::StorageBackend::detect(&session_id);
                                    let result = tokio::task::spawn_blocking(move || {
                                        backend.load(&session_id)
                                    })
                                    .await;
                                    match result {
                                        Ok(Ok(imported)) => {
                                            let count = imported.turns.len();
                                            let source = info.backend.clone();
                                            let mut state = self.state.write().await;
                                            if let Some(session) = state.active_session_mut() {
                                                // Prepend imported turns before existing ones
                                                let existing =
                                                    std::mem::take(&mut session.conversation.turns);
                                                session.conversation.turns = imported.turns;
                                                session.conversation.turns.extend(existing);
                                                // Persist each imported turn to DB
                                                let db_path = session.db_path.clone();
                                                if !db_path.as_os_str().is_empty() {
                                                    let turns_snap = session
                                                            .conversation
                                                            .turns
                                                            .iter()
                                                            .take(count)
                                                            .map(|t| {
                                                                (
                                                                    match t.role {
                                                                        one_core::conversation::TurnRole::User => "user",
                                                                        _ => "assistant",
                                                                    },
                                                                    t.content.clone(),
                                                                    t.timestamp.to_rfc3339(),
                                                                )
                                                            })
                                                            .collect::<Vec<_>>();
                                                    let src = source.clone();
                                                    tokio::task::spawn_blocking(move || {
                                                        if let Ok(db) =
                                                            one_db::SessionDb::open(&db_path)
                                                        {
                                                            for (role, content, ts) in &turns_snap {
                                                                let _ = db.save_message(
                                                                    role, content, ts, None,
                                                                );
                                                            }
                                                            let _ = db.set_meta(
                                                                "imported_from",
                                                                &src.to_lowercase(),
                                                            );
                                                        }
                                                    });
                                                }
                                                // Show confirmation turn
                                                session.conversation.start_assistant_response();
                                                session.conversation.append_to_current(&format!(
                                                    "Imported {count} turns from {source}."
                                                ));
                                                session.conversation.finish_current(None);
                                            }
                                        }
                                        _ => {
                                            let mut state = self.state.write().await;
                                            if let Some(session) = state.active_session_mut() {
                                                session.conversation.start_assistant_response();
                                                session
                                                    .conversation
                                                    .append_to_current("Failed to load session.");
                                                session.conversation.finish_current(None);
                                            }
                                        }
                                    }
                                }
                            }
                            KeyCode::Esc => {
                                self.import_picker = None;
                            }
                            _ => {}
                        }
                    }
                    continue;
                }

                // Handle close-session confirmation (y/n)
                if self.close_confirm {
                    if let CrosstermEvent::Key(key) = crossterm_event {
                        match key.code {
                            KeyCode::Char('y') | KeyCode::Enter => {
                                self.close_confirm = false;
                                self.input.clear_placeholder();
                                let mut s = self.state.write().await;
                                if let Some(sid) = s.active_session_id.clone() {
                                    s.sessions.remove(&sid);
                                    s.active_session_id = s.sessions.keys().next().cloned();
                                }
                                self.tabs = crate::tabs::TabManager::new();
                                let s = self.state.read().await;
                                for (id, session) in &s.sessions {
                                    self.tabs
                                        .add_session(session.project_name.clone(), id.clone());
                                }
                            }
                            _ => {
                                // Any other key cancels
                                self.close_confirm = false;
                                self.input.clear_placeholder();
                            }
                        }
                    }
                    continue;
                }

                // Transcript mode input — intercepts all keys
                if self.transcript_mode {
                    if let CrosstermEvent::Key(key) = crossterm_event {
                        match (key.modifiers, key.code) {
                            // Exit transcript
                            (_, KeyCode::Esc)
                            | (_, KeyCode::Char('q'))
                            | (KeyModifiers::CONTROL, KeyCode::Char('o'))
                            | (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                                self.transcript_mode = false;
                                self.transcript_scroll = 0;
                            }
                            // Toggle show all
                            (KeyModifiers::CONTROL, KeyCode::Char('e')) => {
                                self.transcript_show_all = !self.transcript_show_all;
                                self.transcript_scroll = 0;
                            }
                            // Scroll
                            (_, KeyCode::Up) | (_, KeyCode::Char('k')) => {
                                self.transcript_scroll = self.transcript_scroll.saturating_add(1);
                            }
                            (_, KeyCode::Down) | (_, KeyCode::Char('j')) => {
                                self.transcript_scroll = self.transcript_scroll.saturating_sub(1);
                            }
                            (_, KeyCode::PageUp) => {
                                self.transcript_scroll = self.transcript_scroll.saturating_add(20);
                            }
                            (_, KeyCode::PageDown) => {
                                self.transcript_scroll = self.transcript_scroll.saturating_sub(20);
                            }
                            // Home/End (g/G in less-style)
                            (_, KeyCode::Home) | (_, KeyCode::Char('g')) => {
                                self.transcript_scroll = u16::MAX; // top
                            }
                            (_, KeyCode::End) | (_, KeyCode::Char('G')) => {
                                self.transcript_scroll = 0; // bottom
                            }
                            _ => {}
                        }
                    }
                    continue;
                }

                // Resize: reset scroll so auto-scroll recalculates with new dimensions
                if let CrosstermEvent::Resize(_, _) = crossterm_event {
                    self.messages_scroll = 0;
                }

                let CrosstermEvent::Key(key) = crossterm_event else {
                    continue; // Resize/Mouse/etc — just redraw next iteration
                };
                // Clear Ctrl+C placeholder on any other keypress
                if key.code != KeyCode::Char('c') || key.modifiers != KeyModifiers::CONTROL {
                    self.input.clear_placeholder();
                    self.last_ctrl_c = None;
                }

                match (key.modifiers, key.code) {
                    (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                        // Priority: clear input / exit bash mode → cancel stream → double-tap exit
                        if self.bash_mode || !self.input.value().is_empty() {
                            self.bash_mode = false;
                            self.input.delete_line();
                        } else {
                            // Check if streaming — cancel it
                            let was_streaming = {
                                let mut state = self.state.write().await;
                                if let Some(session) = state.active_session_mut() {
                                    if session.conversation.last_is_streaming() {
                                        session.conversation.finish_current(None);
                                        session.active_tool = None;
                                        self.stream_started = None;
                                        true
                                    } else {
                                        false
                                    }
                                } else {
                                    false
                                }
                            };
                            if !was_streaming {
                                let now = std::time::Instant::now();
                                if let Some(last) = self.last_ctrl_c
                                    && now.duration_since(last).as_millis() < 2000
                                {
                                    let _ = self.event_tx.send(Event::Quit);
                                    return Ok(());
                                }
                                self.last_ctrl_c = Some(now);
                                self.input.set_placeholder("Press Ctrl+C again to exit");
                            }
                        }
                    }
                    // Tab navigation: Ctrl+N or Ctrl+Shift+]
                    (KeyModifiers::CONTROL, KeyCode::Char('n')) => {
                        let new_sid = self.tabs.select_next().map(String::from);
                        let mut state = self.state.write().await;
                        state.active_session_id = new_sid;
                        self.messages_scroll = 0;
                    }
                    (m, KeyCode::Char(']'))
                        if m.contains(KeyModifiers::CONTROL) && m.contains(KeyModifiers::SHIFT) =>
                    {
                        let new_sid = self.tabs.select_next().map(String::from);
                        let mut state = self.state.write().await;
                        state.active_session_id = new_sid;
                        self.messages_scroll = 0;
                    }
                    // Tab navigation: Ctrl+P or Ctrl+Shift+[
                    (KeyModifiers::CONTROL, KeyCode::Char('p')) => {
                        let new_sid = self.tabs.previous().map(String::from);
                        let mut state = self.state.write().await;
                        state.active_session_id = new_sid;
                        self.messages_scroll = 0;
                    }
                    (m, KeyCode::Char('['))
                        if m.contains(KeyModifiers::CONTROL) && m.contains(KeyModifiers::SHIFT) =>
                    {
                        let new_sid = self.tabs.previous().map(String::from);
                        let mut state = self.state.write().await;
                        state.active_session_id = new_sid;
                        self.messages_scroll = 0;
                    }
                    // New session: Ctrl+T
                    (KeyModifiers::CONTROL, KeyCode::Char('t')) => {
                        let project_path = {
                            let s = self.state.read().await;
                            s.active_session()
                                .map(|s| s.project_path.clone())
                                .unwrap_or_else(|| {
                                    std::env::current_dir()
                                        .map(|p| p.to_string_lossy().to_string())
                                        .unwrap_or_else(|_| ".".to_string())
                                })
                        };
                        let configured = detect_configured_providers();
                        if configured.len() > 1 {
                            self.provider_picker = Some(ProviderPickerState {
                                project_path,
                                providers: configured,
                                selected: 0,
                            });
                        } else {
                            let model_config = if let Some(cp) = configured.first() {
                                one_core::provider::ModelConfig {
                                    provider: cp.provider,
                                    model: cp.model.clone(),
                                    max_tokens: 8000,
                                    temperature: None,
                                    budget_tokens: None,
                                }
                            } else {
                                let s = self.state.read().await;
                                s.active_session()
                                    .map(|s| s.model_config.clone())
                                    .unwrap_or_default()
                            };
                            let session =
                                one_core::session::Session::new(project_path, model_config);
                            let sid = session.id.clone();
                            let name = session.project_name.clone();
                            {
                                let mut s = self.state.write().await;
                                s.sessions.insert(sid.clone(), session);
                                s.active_session_id = Some(sid.clone());
                            }
                            self.tabs.add_session(name.clone(), sid.clone());
                            while self.tabs.active_session_id() != Some(&sid) {
                                self.tabs.select_next();
                            }
                            let _ = self.event_tx.send(Event::SessionCreated {
                                session_id: sid,
                                project: name,
                            });
                        }
                    }
                    // Transcript mode: Ctrl+O
                    (KeyModifiers::CONTROL, KeyCode::Char('o')) => {
                        self.transcript_mode = true;
                        self.transcript_scroll = 0;
                    }
                    // ONE.md sidebar toggle: Ctrl+B
                    (KeyModifiers::CONTROL, KeyCode::Char('b')) => {
                        self.onemed_open = !self.onemed_open;
                        if self.onemed_open {
                            // Resolve path: root ONE.md → .one/ONE.md shim → none
                            let path = {
                                let s = self.state.read().await;
                                s.active_session().and_then(|sess| {
                                    let root =
                                        std::path::PathBuf::from(&sess.project_path).join("ONE.md");
                                    let dotone = std::path::PathBuf::from(&sess.project_path)
                                        .join(".one")
                                        .join("ONE.md");
                                    if root.exists() {
                                        Some(root)
                                    } else if dotone.exists() {
                                        Some(dotone)
                                    } else {
                                        None
                                    }
                                })
                            };
                            self.onemed_content =
                                path.and_then(|p| std::fs::read_to_string(&p).ok());
                        } else {
                            self.onemed_content = None;
                        }
                    }
                    // Close session: Ctrl+W (with confirmation)
                    (KeyModifiers::CONTROL, KeyCode::Char('w')) => {
                        let has_session = {
                            let s = self.state.read().await;
                            s.active_session_id.is_some()
                        };
                        if has_session {
                            self.close_confirm = true;
                            self.input.set_placeholder("Close this session? (y/n)");
                        }
                    }
                    // Readline/emacs keybindings
                    (KeyModifiers::CONTROL, KeyCode::Char('l')) => {
                        // Clear screen — reset scroll and clear conversation display
                        self.messages_scroll = 0;
                        let mut state = self.state.write().await;
                        if let Some(session) = state.active_session_mut() {
                            session.conversation.turns.clear();
                        }
                    }
                    (KeyModifiers::CONTROL, KeyCode::Char('u')) => {
                        self.input.delete_line();
                    }
                    (KeyModifiers::CONTROL, KeyCode::Char('k')) => {
                        self.input.kill_to_end();
                    }
                    (KeyModifiers::CONTROL, KeyCode::Char('a')) => {
                        self.input.move_to_start();
                    }
                    (KeyModifiers::CONTROL, KeyCode::Char('e')) => {
                        self.input.move_to_end();
                    }
                    // Alt+Backspace: delete word backward
                    (KeyModifiers::ALT, KeyCode::Backspace) => {
                        self.input.delete_word_backward();
                    }
                    // Ctrl+Backspace: delete entire line
                    (KeyModifiers::CONTROL, KeyCode::Backspace) => {
                        self.input.delete_line();
                    }
                    (_, KeyCode::Esc) => {
                        if self.bash_mode {
                            self.bash_mode = false;
                            self.input.delete_line();
                        } else if self.help_open {
                            self.help_open = false;
                        } else if self.onemed_open {
                            self.onemed_open = false;
                            self.onemed_content = None;
                        } else if self.autocomplete.visible {
                            self.autocomplete.visible = false;
                        } else {
                            // Check if AI is currently streaming — Escape aborts
                            let mut state = self.state.write().await;
                            if let Some(session) = state.active_session_mut()
                                && session.conversation.last_is_streaming()
                            {
                                session.conversation.finish_current(None);
                                session.active_tool = None;
                            }
                            drop(state);
                            // Also handle vim escape
                            self.input.handle_vim_escape();
                        }
                    }
                    (_, KeyCode::Tab) => {
                        if self.bash_mode {
                            // Bash mode: shell-aware tab completion
                            let working_dir = {
                                let s = self.state.read().await;
                                s.active_session()
                                    .map(|s| s.cwd.clone())
                                    .unwrap_or_else(|| ".".to_string())
                            };
                            let raw = self.input.value().to_string();
                            if let Some(completed) = shell_tab_complete(&raw, &working_dir) {
                                self.input.set_value(completed);
                            }
                        } else if let Some(val) =
                            self.autocomplete.accept_with_input(self.input.value())
                        {
                            self.input.set_value(val);
                            self.autocomplete
                                .update_with_context(self.input.value(), ".", &{
                                    let s = self.state.read().await;
                                    s.active_session()
                                        .map(|s| s.cwd.clone())
                                        .unwrap_or_else(|| ".".into())
                                });
                        }
                    }
                    (KeyModifiers::SHIFT, KeyCode::Enter) => {
                        self.input.insert('\n');
                    }
                    (_, KeyCode::Enter) => {
                        if self.autocomplete.visible {
                            let current = self.input.value().to_string();
                            if let Some(val) = self.autocomplete.accept_with_input(&current) {
                                if val == current {
                                    // Exact match — nothing to complete, submit directly
                                    self.autocomplete.visible = false;
                                    self.autocomplete.suggestions.clear();
                                } else {
                                    self.input.set_value(val);
                                    self.autocomplete.update_with_context(
                                        self.input.value(),
                                        ".",
                                        &{
                                            let s = self.state.read().await;
                                            s.active_session()
                                                .map(|s| s.cwd.clone())
                                                .unwrap_or_else(|| ".".into())
                                        },
                                    );
                                    // Accepted a completion — don't submit yet
                                    continue;
                                }
                            }
                        }
                        if let Some(raw_msg) = self.input.submit() {
                            // If bash mode, prefix the command back with !
                            let (msg, is_bash_cmd) = if self.bash_mode {
                                self.bash_mode = false;
                                (format!("!{raw_msg}"), true)
                            } else {
                                (raw_msg, false)
                            };
                            // Check for ! prefix (inline shell command)
                            if is_bash_cmd
                                || msg.starts_with("! ")
                                || msg.starts_with("!") && msg.len() > 1 && !msg.starts_with("!!")
                            {
                                let cmd = msg
                                    .strip_prefix("! ")
                                    .or_else(|| msg.strip_prefix('!'))
                                    .unwrap_or("");
                                if !cmd.trim().is_empty() {
                                    let working_dir = {
                                        let s = self.state.read().await;
                                        s.active_session()
                                            .map(|s| s.cwd.clone())
                                            .unwrap_or_else(|| ".".to_string())
                                    };

                                    // Handle cd: resolve new directory, update cwd
                                    let trimmed_cmd = cmd.trim();
                                    if trimmed_cmd == "cd"
                                        || trimmed_cmd.starts_with("cd ")
                                        || trimmed_cmd.starts_with("cd\t")
                                    {
                                        let target =
                                            trimmed_cmd.strip_prefix("cd").unwrap_or("").trim();
                                        let new_dir = if target.is_empty() {
                                            dirs_next::home_dir()
                                                .map(|h| h.to_string_lossy().to_string())
                                                .unwrap_or(working_dir.clone())
                                        } else {
                                            let expanded = if target.starts_with('~') {
                                                dirs_next::home_dir()
                                                    .map(|h| {
                                                        let rest = target
                                                            .strip_prefix("~/")
                                                            .unwrap_or(&target[1..]);
                                                        h.join(rest).to_string_lossy().to_string()
                                                    })
                                                    .unwrap_or(target.to_string())
                                            } else if std::path::Path::new(target).is_absolute() {
                                                target.to_string()
                                            } else {
                                                std::path::Path::new(&working_dir)
                                                    .join(target)
                                                    .to_string_lossy()
                                                    .to_string()
                                            };
                                            // Canonicalize to resolve .. and .
                                            std::fs::canonicalize(&expanded)
                                                .map(|p| p.to_string_lossy().to_string())
                                                .unwrap_or(expanded)
                                        };

                                        let mut state = self.state.write().await;
                                        if let Some(session) = state.active_session_mut() {
                                            if std::path::Path::new(&new_dir).is_dir() {
                                                session.cwd = new_dir.clone();
                                                session
                                                    .conversation
                                                    .push_user_message(format!("$ {trimmed_cmd}"));
                                                session.conversation.start_assistant_response();
                                                session.conversation.append_to_current(&format!(
                                                    "Changed directory to `{new_dir}`"
                                                ));
                                                session.conversation.finish_current(None);
                                            } else {
                                                session
                                                    .conversation
                                                    .push_user_message(format!("$ {trimmed_cmd}"));
                                                session.conversation.start_assistant_response();
                                                session.conversation.append_to_current(&format!(
                                                    "cd: no such directory: {target}"
                                                ));
                                                session.conversation.finish_current(None);
                                            }
                                        }
                                    } else {
                                        let output = tokio::process::Command::new("sh")
                                            .args(["-c", trimmed_cmd])
                                            .current_dir(&working_dir)
                                            .output()
                                            .await;
                                        let text = match output {
                                            Ok(o) => {
                                                let stdout = String::from_utf8_lossy(&o.stdout);
                                                let stderr = String::from_utf8_lossy(&o.stderr);
                                                let mut result = String::new();
                                                if !stdout.is_empty() {
                                                    result.push_str(&stdout);
                                                }
                                                if !stderr.is_empty() {
                                                    if !result.is_empty() {
                                                        result.push('\n');
                                                    }
                                                    result.push_str(&stderr);
                                                }
                                                if result.is_empty() {
                                                    format!(
                                                        "(command exited with code {})",
                                                        o.status.code().unwrap_or(-1)
                                                    )
                                                } else {
                                                    format!("```\n{}\n```", result.trim())
                                                }
                                            }
                                            Err(e) => format!("Error running command: {e}"),
                                        };
                                        let mut state = self.state.write().await;
                                        if let Some(session) = state.active_session_mut() {
                                            session
                                                .conversation
                                                .push_user_message(format!("$ {trimmed_cmd}"));
                                            session.conversation.start_assistant_response();
                                            session.conversation.append_to_current(&text);
                                            session.conversation.finish_current(None);
                                        }
                                    }
                                }
                            }
                            // Check for slash commands
                            else if msg.starts_with('/') {
                                let result = crate::commands::handle_command(
                                    &msg,
                                    &self.state,
                                    &mut self.pet,
                                )
                                .await;

                                match result {
                                    crate::commands::CommandResult::Message(text) => {
                                        let mut state = self.state.write().await;
                                        if let Some(session) = state.active_session_mut() {
                                            session.conversation.push_user_message(msg);
                                            session.conversation.start_assistant_response();
                                            session.conversation.append_to_current(&text);
                                            session.conversation.finish_current(None);
                                        }
                                    }
                                    crate::commands::CommandResult::ClearConversation => {
                                        let mut state = self.state.write().await;
                                        if let Some(session) = state.active_session_mut() {
                                            session.conversation.turns.clear();
                                        }
                                    }
                                    crate::commands::CommandResult::NewSession { project_path } => {
                                        let configured = detect_configured_providers();
                                        if configured.len() > 1 {
                                            // Multiple providers — show picker
                                            self.provider_picker = Some(ProviderPickerState {
                                                project_path,
                                                providers: configured,
                                                selected: 0,
                                            });
                                        } else {
                                            // Single provider or none — use it directly
                                            let model_config = if let Some(cp) = configured.first()
                                            {
                                                one_core::provider::ModelConfig {
                                                    provider: cp.provider,
                                                    model: cp.model.clone(),
                                                    max_tokens: 8000,
                                                    temperature: None,
                                                    budget_tokens: None,
                                                }
                                            } else {
                                                let s = self.state.read().await;
                                                s.active_session()
                                                    .map(|s| s.model_config.clone())
                                                    .unwrap_or_default()
                                            };
                                            let session = one_core::session::Session::new(
                                                project_path,
                                                model_config,
                                            );
                                            let sid = session.id.clone();
                                            let name = session.project_name.clone();
                                            {
                                                let mut s = self.state.write().await;
                                                s.sessions.insert(sid.clone(), session);
                                                s.active_session_id = Some(sid.clone());
                                            }
                                            self.tabs.add_session(name.clone(), sid.clone());
                                            while self.tabs.active_session_id() != Some(&sid) {
                                                self.tabs.select_next();
                                            }
                                            let _ = self.event_tx.send(Event::SessionCreated {
                                                session_id: sid,
                                                project: name,
                                            });
                                        }
                                    }
                                    crate::commands::CommandResult::CloseSession => {
                                        let mut s = self.state.write().await;
                                        if let Some(sid) = s.active_session_id.clone() {
                                            s.sessions.remove(&sid);
                                            // Switch to another session or inbox
                                            s.active_session_id = s.sessions.keys().next().cloned();
                                        }
                                        // Reset tabs — rebuild from state
                                        self.tabs = crate::tabs::TabManager::new();
                                        let s = self.state.read().await;
                                        for (id, session) in &s.sessions {
                                            self.tabs.add_session(
                                                session.project_name.clone(),
                                                id.clone(),
                                            );
                                        }
                                    }
                                    crate::commands::CommandResult::SwitchSession { name: sid } => {
                                        let mut s = self.state.write().await;
                                        s.active_session_id = Some(sid.clone());
                                        drop(s);
                                        // Move tab selection to match
                                        while self.tabs.active_session_id() != Some(sid.as_str()) {
                                            self.tabs.select_next();
                                        }
                                    }
                                    crate::commands::CommandResult::OAuthLogin { provider } => {
                                        // Show "opening browser" message
                                        {
                                            let mut s = self.state.write().await;
                                            if let Some(session) = s.active_session_mut() {
                                                session.conversation.push_user_message(msg);
                                                session.conversation.start_assistant_response();
                                                session.conversation.append_to_current(&format!(
                                                    "Opening browser for {provider} login..."
                                                ));
                                                session.conversation.finish_current(None);
                                            }
                                        }

                                        // Run OAuth flow in background
                                        let state_clone = self.state.clone();
                                        let provider_clone = provider.clone();
                                        tokio::spawn(async move {
                                            match one_core::oauth::browser_login(&provider_clone)
                                                .await
                                            {
                                                Ok(result) => {
                                                    let mut s = state_clone.write().await;
                                                    if let Some(session) = s.active_session_mut() {
                                                        let all_msgs = result.messages.join("\n");
                                                        session
                                                            .conversation
                                                            .start_assistant_response();
                                                        session
                                                            .conversation
                                                            .append_to_current(&all_msgs);
                                                        session.conversation.finish_current(None);
                                                    }
                                                }
                                                Err(e) => {
                                                    let mut s = state_clone.write().await;
                                                    if let Some(session) = s.active_session_mut() {
                                                        session
                                                            .conversation
                                                            .start_assistant_response();
                                                        session.conversation.append_to_current(
                                                                &format!("Login failed: {e}\n\nYou can also use: /login {provider_clone} <api_key>")
                                                            );
                                                        session.conversation.finish_current(None);
                                                    }
                                                }
                                            }
                                        });
                                    }
                                    crate::commands::CommandResult::SendToAi(prompt) => {
                                        // Send the prompt as a user message to the AI
                                        self.pet.on_user_message();
                                        let mut state = self.state.write().await;
                                        if let Some(session) = state.active_session_mut() {
                                            session.conversation.push_user_message(prompt.clone());
                                            let sid = session.id.clone();
                                            drop(state);
                                            let _ = self.event_tx.send(Event::UserMessage {
                                                session_id: sid,
                                                content: prompt,
                                            });
                                        }
                                    }
                                    crate::commands::CommandResult::EmitEvent(evt) => {
                                        let _ = self.event_tx.send(evt);
                                    }
                                    crate::commands::CommandResult::Silent => {}
                                    crate::commands::CommandResult::Quit => {
                                        let _ = self.event_tx.send(Event::Quit);
                                        return Ok(());
                                    }
                                    crate::commands::CommandResult::NotACommand => {}
                                    crate::commands::CommandResult::OpenImportPicker => {
                                        let sessions = tokio::task::spawn_blocking(
                                            one_core::storage::list_all_importable_sessions,
                                        )
                                        .await
                                        .unwrap_or_else(|_| Ok(Vec::new()))
                                        .unwrap_or_default();

                                        if sessions.is_empty() {
                                            let mut state = self.state.write().await;
                                            if let Some(session) = state.active_session_mut() {
                                                session.conversation.push_user_message(msg);
                                                session.conversation.start_assistant_response();
                                                session.conversation.append_to_current(
                                                    "No sessions found.\n\nSearched:\n  ~/.claude/projects/  (Claude Code)\n  ~/.codex/            (Codex)\n  ~/.gemini/tmp/       (Gemini CLI)"
                                                );
                                                session.conversation.finish_current(None);
                                            }
                                        } else {
                                            self.import_picker = Some(ImportPickerState {
                                                sessions,
                                                selected: 0,
                                                status: None,
                                            });
                                        }
                                    }
                                }
                            } else {
                                self.pet.on_user_message();
                                // Expand @file references in user messages
                                let working_dir = {
                                    let s = self.state.read().await;
                                    s.active_session()
                                        .map(|s| s.project_path.clone())
                                        .unwrap_or_else(|| ".".to_string())
                                };
                                let msg = one_core::skills::expand_at_mentions(&msg, &working_dir);
                                let mut state = self.state.write().await;
                                if let Some(session) = state.active_session_mut() {
                                    session.conversation.push_user_message(msg.clone());
                                    let sid = session.id.clone();
                                    drop(state);
                                    let _ = self.event_tx.send(Event::UserMessage {
                                        session_id: sid,
                                        content: msg,
                                    });
                                } else {
                                    // No active session — create one on the fly
                                    let project_path = std::env::current_dir()
                                        .map(|p| p.to_string_lossy().to_string())
                                        .unwrap_or_else(|_| ".".to_string());
                                    let session = one_core::session::Session::new(
                                        project_path,
                                        one_core::provider::ModelConfig::default(),
                                    );
                                    let sid = session.id.clone();
                                    let name = session.project_name.clone();
                                    state.sessions.insert(sid.clone(), session);
                                    state.active_session_id = Some(sid.clone());

                                    // Now add the message
                                    if let Some(session) = state.active_session_mut() {
                                        session.conversation.push_user_message(msg.clone());
                                    }

                                    self.tabs.add_session(name, sid.clone());
                                    drop(state);

                                    let _ = self.event_tx.send(Event::UserMessage {
                                        session_id: sid,
                                        content: msg,
                                    });
                                }
                            }
                            self.messages_scroll = 0;
                        }
                    }
                    (_, KeyCode::Char(c)) => {
                        if !self.bash_mode
                            && self.input.value().is_empty()
                            && (c == '!' || c == '1' && key.modifiers.contains(KeyModifiers::SHIFT))
                        {
                            // '!' on empty input → enter bash mode (consume the character)
                            self.bash_mode = true;
                            self.help_open = false;
                        } else if !self.bash_mode && self.input.value().is_empty() && c == '?' {
                            // '?' on empty input → toggle help
                            self.help_open = !self.help_open;
                        } else {
                            self.input.insert(c);
                            self.help_open = false;
                        }
                        {
                            let cwd = {
                                let s = self.state.read().await;
                                s.active_session()
                                    .map(|s| s.cwd.clone())
                                    .unwrap_or_else(|| ".".into())
                            };
                            self.autocomplete
                                .update_with_context(self.input.value(), ".", &cwd);
                        }
                    }
                    (_, KeyCode::Backspace) => {
                        if self.bash_mode && self.input.value().is_empty() {
                            // Backspace on empty bash input → exit bash mode
                            self.bash_mode = false;
                        } else {
                            self.input.backspace();
                        }
                        if self.help_open && self.input.value().is_empty() {
                            self.help_open = false;
                        }
                        {
                            let cwd = {
                                let s = self.state.read().await;
                                s.active_session()
                                    .map(|s| s.cwd.clone())
                                    .unwrap_or_else(|| ".".into())
                            };
                            self.autocomplete
                                .update_with_context(self.input.value(), ".", &cwd);
                        }
                    }
                    (_, KeyCode::Left) => self.input.move_left(),
                    (_, KeyCode::Right) => self.input.move_right(),
                    // Up/Down: input history (always)
                    (_, KeyCode::Up) => {
                        if self.autocomplete.visible {
                            self.autocomplete.select_prev();
                        } else {
                            self.input.history_up();
                        }
                    }
                    (_, KeyCode::Down) => {
                        if self.autocomplete.visible {
                            self.autocomplete.select_next();
                        } else {
                            self.input.history_down();
                        }
                    }
                    // PageUp/PageDown: scroll conversation
                    (_, KeyCode::PageUp) => {
                        self.messages_scroll = self.messages_scroll.saturating_add(10);
                    }
                    (_, KeyCode::PageDown) => {
                        self.messages_scroll = self.messages_scroll.saturating_sub(10);
                    }
                    _ => {}
                }
            }
        }
    }

    fn draw(&self, f: &mut Frame, snapshot: &one_core::state::AppState) {
        // Transcript mode: full-screen conversation view
        if self.transcript_mode {
            self.draw_transcript(f, snapshot);
            return;
        }

        if self.permission_prompt.is_some() {
            // Permission prompt: compact banner → tabs → messages → permission overlay
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),  // Compact banner
                    Constraint::Length(3),  // Tab bar
                    Constraint::Min(1),     // Main content
                    Constraint::Length(10), // Permission prompt (overlays input + status)
                ])
                .split(f.area());

            self.draw_banner_compact(f, chunks[0], snapshot);
            self.draw_tabs(f, chunks[1]);
            let msg_area = self.split_for_sidebar(f, chunks[2]);
            if self.tabs.active_session_id().is_none() {
                self.draw_inbox(f, msg_area, snapshot);
            } else {
                self.draw_messages(f, msg_area, snapshot);
            }
            if let Some(ref prompt) = self.permission_prompt {
                self.draw_permission_inline(f, chunks[3], prompt);
            }
        } else {
            // Show full welcome banner only if every session is still empty.
            // Once any tab gets its first message, collapse globally — including inbox.
            let show_banner = snapshot
                .sessions
                .values()
                .all(|s| s.conversation.turns.is_empty());

            if show_banner {
                // Welcome layout: full banner → tabs → input (all at top)
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(12), // Welcome banner
                        Constraint::Length(3),  // Tab bar
                        Constraint::Length(3),  // Input (right under tabs)
                        Constraint::Min(0),     // Empty space below
                    ])
                    .split(f.area());

                self.draw_banner(f, chunks[0], snapshot);
                self.draw_tabs(f, chunks[1]);
                self.draw_input(f, chunks[2], snapshot);
                self.autocomplete.render(f, chunks[2]);
            } else if self.help_open {
                // Help layout: compact banner → tabs → messages → help overlay
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(1),  // Compact banner
                        Constraint::Length(3),  // Tab bar
                        Constraint::Min(1),     // Messages (compressed)
                        Constraint::Length(22), // Help overlay
                    ])
                    .split(f.area());

                self.draw_banner_compact(f, chunks[0], snapshot);
                self.draw_tabs(f, chunks[1]);
                let msg_area = self.split_for_sidebar(f, chunks[2]);
                if self.tabs.active_session_id().is_none() {
                    self.draw_inbox(f, msg_area, snapshot);
                } else {
                    self.draw_messages(f, msg_area, snapshot);
                }
                self.draw_help_overlay(f, chunks[3]);
            } else {
                // Conversation layout: compact banner → tabs → messages → input
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(1), // Compact banner
                        Constraint::Length(3), // Tab bar
                        Constraint::Min(1),    // Messages
                        Constraint::Length(3), // Input
                    ])
                    .split(f.area());

                self.draw_banner_compact(f, chunks[0], snapshot);
                self.draw_tabs(f, chunks[1]);
                let msg_area = self.split_for_sidebar(f, chunks[2]);
                if self.tabs.active_session_id().is_none() {
                    self.draw_inbox(f, msg_area, snapshot);
                } else {
                    self.draw_messages(f, msg_area, snapshot);
                }
                self.draw_input(f, chunks[3], snapshot);
                self.autocomplete.render(f, chunks[3]);
            }
        }

        // Provider picker overlay (drawn on top of everything)
        if let Some(ref picker) = self.provider_picker {
            self.draw_provider_picker(f, picker);
        }

        // Import picker overlay (drawn on top of everything)
        if let Some(ref picker) = self.import_picker {
            self.draw_import_picker(f, picker);
        }
    }

    /// Build the thinking status components: (verb, elapsed, tokens, tip).
    fn thinking_status(
        &self,
        session: &one_core::session::Session,
    ) -> (&str, String, String, &str) {
        const VERBS: &[&str] = &[
            "Accomplishing",
            "Actioning",
            "Actualizing",
            "Architecting",
            "Baking",
            "Beaming",
            "Beboppin'",
            "Befuddling",
            "Billowing",
            "Blanching",
            "Bloviating",
            "Boogieing",
            "Boondoggling",
            "Booping",
            "Bootstrapping",
            "Brewing",
            "Bunning",
            "Burrowing",
            "Calculating",
            "Canoodling",
            "Caramelizing",
            "Cascading",
            "Catapulting",
            "Cerebrating",
            "Channeling",
            "Choreographing",
            "Churning",
            "Coalescing",
            "Cogitating",
            "Combobulating",
            "Composing",
            "Computing",
            "Concocting",
            "Considering",
            "Contemplating",
            "Cooking",
            "Crafting",
            "Creating",
            "Crunching",
            "Crystallizing",
            "Cultivating",
            "Deciphering",
            "Deliberating",
            "Determining",
            "Dilly-dallying",
            "Discombobulating",
            "Doing",
            "Doodling",
            "Drizzling",
            "Ebbing",
            "Effecting",
            "Elucidating",
            "Embellishing",
            "Enchanting",
            "Envisioning",
            "Evaporating",
            "Fermenting",
            "Fiddle-faddling",
            "Finagling",
            "Flowing",
            "Flummoxing",
            "Fluttering",
            "Forging",
            "Forming",
            "Frolicking",
            "Frosting",
            "Gallivanting",
            "Galloping",
            "Garnishing",
            "Generating",
            "Gesticulating",
            "Germinating",
            "Grooving",
            "Gusting",
            "Harmonizing",
            "Hashing",
            "Hatching",
            "Herding",
            "Honking",
            "Hyperspacing",
            "Ideating",
            "Imagining",
            "Improvising",
            "Incubating",
            "Inferring",
            "Infusing",
            "Ionizing",
            "Jitterbugging",
            "Julienning",
            "Kneading",
            "Leavening",
            "Levitating",
            "Lollygagging",
            "Manifesting",
            "Marinating",
            "Meandering",
            "Metamorphosing",
            "Misting",
            "Moonwalking",
            "Moseying",
            "Mulling",
            "Mustering",
            "Musing",
            "Nebulizing",
            "Nesting",
            "Noodling",
            "Nucleating",
            "Orbiting",
            "Orchestrating",
            "Osmosing",
            "Perambulating",
            "Percolating",
            "Perusing",
            "Philosophising",
            "Photosynthesizing",
            "Pollinating",
            "Pondering",
            "Pontificating",
            "Pouncing",
            "Precipitating",
            "Prestidigitating",
            "Processing",
            "Proofing",
            "Propagating",
            "Puttering",
            "Puzzling",
            "Quantumizing",
            "Razzle-dazzling",
            "Razzmatazzing",
            "Recombobulating",
            "Reticulating",
            "Roosting",
            "Ruminating",
            "Scampering",
            "Schlepping",
            "Scurrying",
            "Seasoning",
            "Shimmying",
            "Simmering",
            "Skedaddling",
            "Sketching",
            "Slithering",
            "Smooshing",
            "Spelunking",
            "Spinning",
            "Sprouting",
            "Stewing",
            "Sublimating",
            "Swirling",
            "Swooping",
            "Synthesizing",
            "Tempering",
            "Thinking",
            "Thundering",
            "Tinkering",
            "Transmuting",
            "Twisting",
            "Undulating",
            "Unfurling",
            "Unravelling",
            "Vibing",
            "Waddling",
            "Wandering",
            "Warping",
            "Whirlpooling",
            "Whirring",
            "Whisking",
            "Wibbling",
            "Working",
            "Wrangling",
            "Zesting",
            "Zigzagging",
        ];
        const TIPS: &[&str] = &[
            "Use /model to switch AI models mid-conversation.",
            "Use /new <path> to open a project in a new tab.",
            "Ctrl+Shift+[ and ] to cycle between tabs.",
            "Prefix a message with ! to run a shell command inline.",
            "Use @path/to/file to include file contents in your message.",
            "Use /session to see all your active sessions.",
            "Press Escape to abort a streaming response.",
            "Use /provider to check which providers are configured.",
            "Shift+Enter inserts a newline for multi-line input.",
            "Use /cost to check your session token usage and estimated cost.",
            "Use /compact to summarize the conversation and save context.",
            "The /switch command lets you jump to another project by name.",
            "Ctrl+T opens a new session. Ctrl+W closes the current one.",
            "Up/Down arrows browse your input history.",
            "Ctrl+C clears input, cancels streams, or double-tap to exit.",
            "Use /effort to control reasoning depth (low/medium/high/max).",
        ];

        let verb = VERBS[self.thinking_verb_idx % VERBS.len()];
        let tip = TIPS[self.tip_idx % TIPS.len()];

        // Elapsed time
        let elapsed = self.stream_started.map(|s| s.elapsed()).unwrap_or_default();
        let secs = elapsed.as_secs();
        let elapsed_str = if secs >= 60 {
            format!("{}m {:02}s", secs / 60, secs % 60)
        } else {
            format!("{}s", secs)
        };

        // Token count
        let total = session.total_input_tokens + session.total_output_tokens;
        let tokens_str = if total > 0 {
            format!("\u{2193} {} tokens", format_token_count(total))
        } else {
            "\u{2193} 0 tokens".to_string()
        };

        (verb, elapsed_str, tokens_str, tip)
    }

    /// Render full-screen transcript view (Ctrl+O).
    fn draw_transcript(&self, f: &mut Frame, snapshot: &one_core::state::AppState) {
        let area = f.area();
        let dim = Style::default().fg(Color::DarkGray);

        // Header bar
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Header
                Constraint::Min(1),    // Content
                Constraint::Length(1), // Footer
            ])
            .split(area);

        // Header
        let show_label = if self.transcript_show_all {
            "all"
        } else {
            "recent"
        };
        let header = Paragraph::new(Line::from(vec![
            Span::styled(
                " TRANSCRIPT ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  showing: {show_label}"), dim),
        ]));
        f.render_widget(header, chunks[0]);

        // Build transcript lines — reuse the same message format as draw_messages
        let mut lines = Vec::new();
        if let Some(session) = snapshot.active_session() {
            let turns = &session.conversation.turns;
            let max_recent = 30;
            let show_turns = if self.transcript_show_all || turns.len() <= max_recent {
                turns.as_slice()
            } else {
                let hidden = turns.len() - max_recent;
                lines.push(Line::from(Span::styled(
                    format!("  --- {hidden} earlier messages hidden (Ctrl+E to show all) ---"),
                    dim,
                )));
                lines.push(Line::from(""));
                &turns[turns.len() - max_recent..]
            };

            for turn in show_turns {
                if turn.role == one_core::conversation::TurnRole::Assistant
                    && turn.content.trim().is_empty()
                    && !turn.is_streaming
                {
                    continue;
                }

                match turn.role {
                    one_core::conversation::TurnRole::User => {
                        lines.push(Line::from(""));
                        for line in turn.content.lines() {
                            lines.push(Line::from(Span::styled(
                                format!("> {line}"),
                                Style::default()
                                    .fg(Color::White)
                                    .add_modifier(Modifier::BOLD),
                            )));
                        }
                        lines.push(Line::from(""));
                    }
                    one_core::conversation::TurnRole::Assistant => {
                        if !turn.content.is_empty() {
                            let md_lines = crate::markdown::render_markdown(&turn.content);
                            lines.extend(md_lines);
                        }
                        for tc in &turn.tool_calls {
                            let face_name = match tc.tool_name.as_str() {
                                "Edit" => "Update",
                                "Glob" => "Search",
                                _ => &tc.tool_name,
                            };
                            let display = if tc.input_summary.is_empty() {
                                face_name.to_string()
                            } else {
                                format!("{face_name}({})", tc.input_summary)
                            };
                            lines.push(Line::from(Span::styled(
                                format!("\u{23fa} {display}"),
                                Style::default().add_modifier(Modifier::BOLD),
                            )));
                            if let Some(ref output) = tc.output {
                                for l in output.lines().take(3) {
                                    lines.push(Line::from(Span::styled(
                                        format!("  \u{23bf}  {l}"),
                                        dim,
                                    )));
                                }
                                if output.lines().count() > 3 {
                                    lines.push(Line::from(Span::styled(
                                        format!(
                                            "  \u{23bf}  \u{2026} +{} more",
                                            output.lines().count() - 3
                                        ),
                                        dim,
                                    )));
                                }
                            }
                        }
                    }
                    _ => {
                        for line in turn.content.lines() {
                            lines.push(Line::from(Span::styled(line.to_string(), dim)));
                        }
                    }
                }
            }
        } else {
            lines.push(Line::from(Span::styled("  No active session.", dim)));
        }

        // Render scrollable content
        let text = Text::from(lines);
        let inner_width = chunks[1].width.saturating_sub(2) as usize;
        let content_height: u16 = text
            .lines
            .iter()
            .map(|line| {
                let w: usize = line.spans.iter().map(|s| s.content.len()).sum();
                if w == 0 {
                    1
                } else {
                    w.div_ceil(inner_width) as u16
                }
            })
            .sum();
        let visible = chunks[1].height;
        let max_scroll = content_height.saturating_sub(visible);
        let scroll_offset = max_scroll.saturating_sub(self.transcript_scroll.min(max_scroll));

        let content = Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .scroll((scroll_offset, 0));
        f.render_widget(content, chunks[1]);

        // Footer
        let footer_text = if self.transcript_show_all {
            " Ctrl+E hide old  |  j/k scroll  |  g/G top/bottom  |  Esc/q exit"
        } else {
            " Ctrl+E show all  |  j/k scroll  |  g/G top/bottom  |  Esc/q exit"
        };
        let footer = Paragraph::new(Line::from(Span::styled(footer_text, dim)));
        f.render_widget(footer, chunks[2]);
    }

    /// Render the provider picker as a centered dialog overlay.
    fn draw_provider_picker(&self, f: &mut Frame, picker: &ProviderPickerState) {
        use ratatui::widgets::Clear;

        let area = f.area();
        let height = (picker.providers.len() as u16 * 2) + 6; // 2 lines per provider + header/footer
        let width = 50u16.min(area.width.saturating_sub(4));
        let x = (area.width.saturating_sub(width)) / 2;
        let y = (area.height.saturating_sub(height)) / 2;
        let dialog = Rect::new(x, y, width, height);

        // Clear area behind the dialog
        f.render_widget(Clear, dialog);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(" choose a provider ")
            .title_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            );
        let inner = block.inner(dialog);
        f.render_widget(block, dialog);

        let mut lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                " Which provider for this session?",
                Style::default().fg(Color::White),
            )),
            Line::from(""),
        ];

        for (i, cp) in picker.providers.iter().enumerate() {
            if i == picker.selected {
                lines.push(Line::from(Span::styled(
                    format!(" > {}  ({})", cp.label, cp.status),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    format!("   {}  ({})", cp.label, cp.status),
                    Style::default().fg(Color::White),
                )));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " ↑↓ navigate  Enter select  Esc cancel",
            Style::default().fg(Color::DarkGray),
        )));

        let content = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
        f.render_widget(content, inner);
    }

    fn draw_import_picker(&self, f: &mut Frame, picker: &ImportPickerState) {
        use ratatui::widgets::Clear;

        let area = f.area();
        let height = (picker.sessions.len() as u16 * 3 + 6).min(area.height.saturating_sub(4));
        let width = 72u16.min(area.width.saturating_sub(4));
        let x = (area.width.saturating_sub(width)) / 2;
        let y = (area.height.saturating_sub(height)) / 2;
        let dialog = Rect::new(x, y, width, height);

        f.render_widget(Clear, dialog);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(" import session ")
            .title_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            );
        let inner = block.inner(dialog);
        f.render_widget(block, dialog);

        let dim = Style::default().fg(Color::DarkGray);
        let mut lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                " Select a session to import as context:",
                Style::default().fg(Color::White),
            )),
            Line::from(""),
        ];

        for (i, s) in picker.sessions.iter().enumerate() {
            let date = if s.timestamp.len() >= 10 {
                &s.timestamp[..10]
            } else {
                &s.timestamp
            };
            let label = format!(" {} | {} | {}", date, s.backend, s.project_path);
            let preview = format!(
                "   \"{}\"",
                if s.first_message.len() > 55 {
                    format!("{}...", &s.first_message[..55])
                } else {
                    s.first_message.clone()
                }
            );
            if i == picker.selected {
                lines.push(Line::from(Span::styled(
                    format!(" ❯{}", &label[1..]),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(Span::styled(
                    preview,
                    Style::default().fg(Color::White),
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    label,
                    Style::default().fg(Color::White),
                )));
                lines.push(Line::from(Span::styled(preview, dim)));
            }
            lines.push(Line::from(""));
        }

        if let Some(ref status) = picker.status {
            lines.push(Line::from(Span::styled(
                format!(" {status}"),
                Style::default().fg(Color::Green),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                " ↑↓/jk navigate  Enter import  Esc cancel",
                dim,
            )));
        }

        f.render_widget(
            Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
            inner,
        );
    }

    /// Render the permission prompt inline.
    /// Tool category header, command display, numbered menu, footer hints.
    /// Overlays the input + status bar area.
    fn draw_permission_inline(&self, f: &mut Frame, area: Rect, prompt: &PermissionPromptState) {
        let dim = Style::default().fg(Color::DarkGray);
        let white = Style::default().fg(Color::White);

        // Tool category header (bold, colored — like CC's "Bash command")
        let (category, category_color) = match prompt.tool_name.as_str() {
            "Bash" => ("Bash command", Color::Yellow),
            "Edit" => ("Edit file", Color::Cyan),
            "Write" => ("Write file", Color::Cyan),
            "Read" => ("Read file", Color::Green),
            "Grep" => ("Search content", Color::Green),
            "Glob" => ("Search files", Color::Green),
            "Agent" => ("Launch agent", Color::Magenta),
            _ => (&*prompt.tool_name, Color::White),
        };

        // Build the "don't ask again" pattern for option 2
        let always_pattern = match prompt.tool_name.as_str() {
            "Bash" => {
                // Extract first word of command for the pattern
                let first_word = prompt
                    .input_summary
                    .split_whitespace()
                    .next()
                    .unwrap_or("*");
                format!("{}:{}*", prompt.tool_name.to_lowercase(), first_word)
            }
            _ => format!("{}:*", prompt.tool_name.to_lowercase()),
        };

        let sel = prompt.selected;
        let marker = |idx: u8| -> &'static str { if idx == sel { "\u{276f}" } else { " " } };
        let opt_style = |idx: u8| -> Style {
            if idx == sel {
                Style::default().fg(Color::Cyan)
            } else {
                white
            }
        };

        let mut lines = vec![];

        // Header: tool category
        lines.push(Line::from(Span::styled(
            format!(" {category}"),
            Style::default()
                .fg(category_color)
                .add_modifier(Modifier::BOLD),
        )));

        // Input display (indented, like a code block)
        if !prompt.input_summary.is_empty() {
            let max_width = area.width as usize - 6;
            let display = if prompt.input_summary.len() > max_width {
                format!(
                    "{}...",
                    &prompt.input_summary[..max_width.saturating_sub(3)]
                )
            } else {
                prompt.input_summary.clone()
            };
            lines.push(Line::from(Span::styled(format!("    {display}"), white)));
        }

        // Warning if present
        if let Some(ref warning) = prompt.warning {
            lines.push(Line::from(Span::styled(
                format!("    {warning}"),
                Style::default().fg(Color::Yellow),
            )));
        }

        lines.push(Line::from(""));

        // Question
        lines.push(Line::from(Span::styled(" Do you want to proceed?", white)));

        // Numbered options with > selector
        lines.push(Line::from(vec![
            Span::styled(format!(" {} ", marker(0)), Style::default().fg(Color::Cyan)),
            Span::styled("1. ", dim),
            Span::styled("Yes", opt_style(0)),
        ]));
        lines.push(Line::from(vec![
            Span::styled(format!(" {} ", marker(1)), Style::default().fg(Color::Cyan)),
            Span::styled("2. ", dim),
            Span::styled(
                format!("Yes, and don't ask again for: {always_pattern}"),
                opt_style(1),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled(format!(" {} ", marker(2)), Style::default().fg(Color::Cyan)),
            Span::styled("3. ", dim),
            Span::styled("No", opt_style(2)),
        ]));

        // Footer hints
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(" Esc to cancel", dim)));

        let block = Block::default().borders(Borders::TOP).border_style(dim);

        let paragraph = Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: false });

        f.render_widget(paragraph, area);
    }

    /// Welcome banner shown before the first message — mirrors CC's welcome screen.
    /// Two-column layout: left = welcome + pet + provider, right = tips.
    fn draw_banner(&self, f: &mut Frame, area: Rect, snapshot: &one_core::state::AppState) {
        let version = env!("CARGO_PKG_VERSION");
        let dim = Style::default().fg(Color::DarkGray);
        let white = Style::default().fg(Color::White);

        // Split into left (55%) and right (45%) columns inside the border
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .title(format!(" One v{version} "))
            .border_style(Style::default().fg(Color::DarkGray));

        let inner = block.inner(area);
        f.render_widget(block, area);

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(inner);

        // ── Left column: welcome + pet + provider info ──
        let mut left_lines = vec![Line::from("")];

        // Welcome message
        left_lines.push(Line::from(Span::styled(
            "              Welcome!",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )));
        left_lines.push(Line::from(""));

        // Pet in the center
        let pet_art = self.pet.ascii_art();
        let pet_color = self.pet.mood_color();
        left_lines.push(Line::from(Span::styled(
            format!("                 {pet_art}"),
            Style::default().fg(pet_color),
        )));
        left_lines.push(Line::from(Span::styled(
            format!("                 {}", self.pet.name),
            dim,
        )));
        left_lines.push(Line::from(""));

        // Provider + model info
        if let Some(session) = snapshot.active_session() {
            let model = &session.model_config.model;
            let short_model = model.replace("claude-", "").replace("-20250514", "");
            left_lines.push(Line::from(Span::styled(format!(" {short_model}"), white)));
        }

        // Working directory
        let cwd = snapshot
            .active_session()
            .map(|s| {
                let p = &s.project_path;
                // Shorten home dir to ~
                if let Some(home) = dirs_next::home_dir()
                    && let Ok(rel) = std::path::Path::new(p).strip_prefix(&home)
                {
                    return format!("~/{}", rel.display());
                }
                p.clone()
            })
            .unwrap_or_else(|| ".".to_string());
        left_lines.push(Line::from(Span::styled(format!(" {cwd}"), dim)));

        f.render_widget(
            Paragraph::new(Text::from(left_lines)).wrap(Wrap { trim: false }),
            cols[0],
        );

        // ── Right column: tips ──
        let right_lines = vec![
            Line::from(Span::styled(" Tips for getting started", white)),
            Line::from(Span::styled(" Type a message to start chatting", dim)),
            Line::from(Span::styled(" /help to see all commands", dim)),
            Line::from(Span::styled(" /login to sign in with Hugging Face", dim)),
            Line::from(Span::styled(" @file to include file context", dim)),
            Line::from(Span::styled(" ! command to run shell inline", dim)),
            Line::from(""),
            Line::from(Span::styled(" Recent activity", white)),
            Line::from(Span::styled(" No recent activity", dim)),
        ];

        f.render_widget(
            Paragraph::new(Text::from(right_lines)).wrap(Wrap { trim: false }),
            cols[1],
        );
    }

    /// Compact single-line banner shown when conversation has messages.
    fn draw_banner_compact(&self, f: &mut Frame, area: Rect, snapshot: &one_core::state::AppState) {
        let version = env!("CARGO_PKG_VERSION");
        let dim = Style::default().fg(Color::DarkGray);

        let mut spans = vec![
            Span::styled(
                format!(" One v{version} "),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" \u{2502} ", dim),
        ];

        // Model
        if let Some(session) = snapshot.active_session() {
            let short_model = session
                .model_config
                .model
                .replace("claude-", "")
                .replace("-20250514", "");
            spans.push(Span::styled(short_model, Style::default().fg(Color::White)));
            spans.push(Span::styled(" \u{2502} ", dim));
        }

        // Token count + cost + context %
        if let Some(session) = snapshot.active_session() {
            let total = session.total_input_tokens + session.total_output_tokens;
            if total > 0 {
                let tokens = format_token_count(total);
                spans.push(Span::styled(format!("{tokens} tokens"), dim));
                if session.cost_usd > 0.001 {
                    spans.push(Span::styled(format!("  ${:.2}", session.cost_usd), dim));
                }
                // Context window usage %
                let context_window = context_window_for_model(&session.model_config.model);
                if context_window > 0 {
                    let pct = ((session.total_input_tokens as f64 / context_window as f64) * 100.0)
                        .round()
                        .min(100.0) as u32;
                    let pct_color = match pct {
                        0..=50 => Color::Green,
                        51..=75 => Color::Yellow,
                        _ => Color::Red,
                    };
                    let filled = ((pct as usize * 5 + 50) / 100).min(5);
                    let bar: String = "▓".repeat(filled) + &"░".repeat(5 - filled);
                    spans.push(Span::styled(
                        format!("  {bar} {pct}%"),
                        Style::default().fg(pct_color),
                    ));
                }
                spans.push(Span::styled(" \u{2502} ", dim));
            }
        }

        // Active tool
        if let Some(session) = snapshot.active_session()
            && let Some(ref tool) = session.active_tool
        {
            spans.push(Span::styled(
                format!("\u{21BB} {tool}"),
                Style::default().fg(Color::Yellow),
            ));
            spans.push(Span::styled(" \u{2502} ", dim));
        }

        // Pet
        if self.pet.enabled {
            spans.push(Span::styled(
                self.pet.ascii_art(),
                Style::default().fg(self.pet.mood_color()),
            ));
        }

        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    /// Split `area` horizontally for the ONE.md sidebar.
    /// Renders the sidebar in the right 30% and returns the left 70% for messages.
    /// When the sidebar is closed returns `area` unchanged.
    fn split_for_sidebar(&self, f: &mut Frame, area: Rect) -> Rect {
        if !self.onemed_open {
            return area;
        }
        let h_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
            .split(area);
        self.draw_onemed_sidebar(f, h_chunks[1]);
        h_chunks[0]
    }

    fn draw_onemed_sidebar(&self, f: &mut Frame, area: Rect) {
        let dim = Style::default().fg(Color::DarkGray);
        let placeholder = "No ONE.md found in this project.\n\nCreate ONE.md to document\nproject context for the AI.";
        let content = self.onemed_content.as_deref().unwrap_or(placeholder);

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" ONE.md ")
            .title_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .border_style(dim);
        let inner = block.inner(area);
        f.render_widget(block, area);

        let paragraph = Paragraph::new(content)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(Color::White));
        f.render_widget(paragraph, inner);
    }

    fn draw_tabs(&self, f: &mut Frame, area: Rect) {
        const BRAILLE: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        let spinning = self.stream_started.is_some();
        let frame = BRAILLE[self.spinner_tick as usize % BRAILLE.len()];
        let selected = self.tabs.selected();
        let titles: Vec<Line> = self
            .tabs
            .titles()
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let label = if spinning && i == selected {
                    format!("{frame} {t}")
                } else {
                    t.clone()
                };
                Line::from(Span::styled(label, Style::default().fg(Color::White)))
            })
            .collect();

        let tabs = RatatuiTabs::new(titles)
            .block(Block::default().borders(Borders::ALL).title(" one "))
            .select(self.tabs.selected())
            .style(Style::default().fg(Color::DarkGray))
            .highlight_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            );

        f.render_widget(tabs, area);
    }

    fn draw_messages(&self, f: &mut Frame, area: Rect, snapshot: &one_core::state::AppState) {
        let block = Block::default();

        let session = snapshot.active_session();

        let lines: Vec<Line> = if let Some(session) = session {
            if session.conversation.turns.is_empty() {
                vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        format!("  Project: {}", session.project_name),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from(""),
                    Line::from(Span::styled(
                        "  Type a message below and press Enter to ask anything.",
                        Style::default().fg(Color::White),
                    )),
                    Line::from(Span::styled(
                        "  Try: \"What does this project do?\" or \"Show me the main files\"",
                        Style::default().fg(Color::DarkGray),
                    )),
                ]
            } else {
                let mut result = Vec::new();

                // Effort dot color — matches the effort symbol system
                let dot_color = match session
                    .effort
                    .as_deref()
                    .and_then(one_core::effort::parse_effort)
                    .unwrap_or(one_core::effort::EFFORT_HIGH)
                {
                    0 => Color::DarkGray,
                    1 => Color::White,
                    2 => Color::Yellow,
                    3 => Color::Cyan,
                    4 => Color::Magenta,
                    _ => Color::Cyan,
                };
                let dot = Span::styled(
                    "\u{23fa} ", // ⏺
                    Style::default().fg(dot_color),
                );

                let debug_mode = snapshot.debug_mode;
                let debug_dim = Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM);
                let mut dbg_idx = 0usize;

                for turn in &session.conversation.turns {
                    // Interleave debug events that occurred before this turn
                    if debug_mode {
                        while dbg_idx < session.debug_events.len()
                            && session.debug_events[dbg_idx].0 <= turn.timestamp
                        {
                            result.push(Line::from(Span::styled(
                                format!("  \u{2812} {}", session.debug_events[dbg_idx].1),
                                debug_dim,
                            )));
                            dbg_idx += 1;
                        }
                    }

                    // Skip empty assistant turns — but keep turns that have tool calls,
                    // even when they have no prose content.
                    if turn.role == TurnRole::Assistant
                        && turn.content.trim().is_empty()
                        && !turn.is_streaming
                        && turn.tool_calls.is_empty()
                    {
                        continue;
                    }

                    match turn.role {
                        TurnRole::User => {
                            // User: "> message"
                            result.push(Line::from(""));
                            for line in turn.content.lines() {
                                result.push(Line::from(Span::styled(
                                    format!("> {line}"),
                                    Style::default()
                                        .fg(Color::White)
                                        .add_modifier(Modifier::BOLD),
                                )));
                            }
                            result.push(Line::from(""));
                        }
                        TurnRole::Assistant => {
                            // Assistant: "⏺ content" (markdown rendered)
                            if !turn.content.is_empty() {
                                let md_lines = crate::markdown::render_markdown(&turn.content);
                                for (i, line) in md_lines.into_iter().enumerate() {
                                    if i == 0 {
                                        // Prepend dot to first line
                                        let mut spans = vec![dot.clone()];
                                        spans.extend(line.spans);
                                        result.push(Line::from(spans));
                                    } else {
                                        result.push(line);
                                    }
                                }
                            }
                        }
                        TurnRole::System => {
                            for line in turn.content.lines() {
                                result.push(Line::from(Span::styled(
                                    line.to_string(),
                                    Style::default().fg(Color::DarkGray),
                                )));
                            }
                        }
                        TurnRole::ToolResult => {
                            if !turn.content.is_empty() {
                                let dim = Style::default().add_modifier(Modifier::DIM);
                                let total = turn.content.lines().count();
                                for line in turn.content.lines().take(5) {
                                    let truncated = truncate_line(line, 120);
                                    result.push(Line::from(Span::styled(
                                        format!("  \u{23bf}  {truncated}"),
                                        dim,
                                    )));
                                }
                                if total > 5 {
                                    result.push(Line::from(Span::styled(
                                        format!("  \u{23bf}  \u{2026} +{} more lines", total - 5),
                                        dim,
                                    )));
                                }
                            }
                        }
                    }

                    // Tool calls: "⏺ ToolName(args)" with output below
                    if turn.role == TurnRole::Assistant && !turn.tool_calls.is_empty() {
                        for tc in &turn.tool_calls {
                            let face_name = match tc.tool_name.as_str() {
                                "Edit" => "Update",
                                "Glob" => "Search",
                                _ => &tc.tool_name,
                            };
                            let display = if tc.input_summary.is_empty() {
                                face_name.to_string()
                            } else {
                                format!("{face_name}({})", tc.input_summary)
                            };
                            let dim = Style::default().add_modifier(Modifier::DIM);
                            // Tool header: animated while running, ⏺ once complete.
                            // Style is picked per-tool from spinner_tick at ToolRequest time.
                            let header_dot = if tc.output.is_none() {
                                const GROWING: &[&str] =
                                    &["\u{00b7}", "\u{2022}", "\u{25cf}", "\u{2022}", "\u{00b7}"];
                                const FALLING_SAND: &[&str] = &[
                                    "⠁", "⠂", "⠄", "⡀", "⡈", "⡐", "⡠", "⣀", "⣁", "⣂", "⣄", "⣌",
                                    "⣔", "⣤", "⣥", "⣦", "⣮", "⣶", "⣷", "⣿", "⡿", "⠿", "⢟", "⠟",
                                    "⡛", "⠛", "⠫", "⢋", "⠋", "⠍", "⡉", "⠉", "⠑", "⠡", "⢁",
                                ];
                                const FOLD: &[&str] = &["-", "≻", "›", "⟩", "|", "⟨", "‹", "≺"];
                                const BOX_BOUNCE: &[&str] = &["▖", "▘", "▝", "▗"];
                                const BRAILLE: &[&str] =
                                    &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                                let frames: &[&str] = match tc.dot_style % 5 {
                                    0 => GROWING,
                                    1 => FALLING_SAND,
                                    2 => FOLD,
                                    3 => BOX_BOUNCE,
                                    _ => BRAILLE,
                                };
                                let d = frames[self.spinner_tick as usize % frames.len()];
                                Span::styled(
                                    format!("{d} "),
                                    Style::default().fg(dot_color).add_modifier(Modifier::DIM),
                                )
                            } else {
                                dot.clone()
                            };
                            result.push(Line::from(vec![
                                header_dot,
                                Span::styled(
                                    display,
                                    Style::default().add_modifier(Modifier::BOLD),
                                ),
                            ]));
                            // Tool output: "  ⎿  content"
                            if tc.output.is_none() {
                                // Pending — tool is still running
                                result.push(Line::from(Span::styled(
                                    "  \u{23bf}  running\u{2026}",
                                    Style::default()
                                        .fg(Color::Yellow)
                                        .add_modifier(Modifier::DIM),
                                )));
                            } else if let Some(ref output) = tc.output
                                && output.trim().is_empty()
                            {
                                // Completed with no output
                                result.push(Line::from(vec![
                                    Span::styled("  \u{23bf}  ", dim),
                                    Span::styled("(No output)", dim),
                                ]));
                            } else if let Some(ref output) = tc.output
                                && !output.is_empty()
                            {
                                let out_line = |text: &str| {
                                    Line::from(vec![
                                        Span::styled("  \u{23bf}  ", dim),
                                        Span::styled(text.to_string(), dim),
                                    ])
                                };

                                match tc.tool_name.as_str() {
                                    "Read" => {
                                        let count = output.lines().count();
                                        result.push(out_line(&format!("{count} lines")));
                                    }
                                    "Write" => {
                                        let first = output.lines().next().unwrap_or("Done");
                                        result.push(out_line(first));
                                    }
                                    "Bash" => {
                                        let lines: Vec<&str> = output.lines().collect();
                                        if lines.is_empty() {
                                            result.push(out_line("(No output)"));
                                        } else {
                                            for l in lines.iter().take(5) {
                                                result.push(out_line(&truncate_line(l, 120)));
                                            }
                                            if lines.len() > 5 {
                                                result.push(out_line(&format!(
                                                    "\u{2026} +{} more lines",
                                                    lines.len() - 5
                                                )));
                                            }
                                        }
                                    }
                                    "Grep" | "Glob" => {
                                        let count = output.lines().count();
                                        let unit = if tc.tool_name == "Grep" {
                                            "results"
                                        } else {
                                            "files"
                                        };
                                        result.push(out_line(&format!("Found {count} {unit}")));
                                    }
                                    "Edit" => {
                                        let mut lines_iter = output.lines();
                                        if let Some(s) = lines_iter.next() {
                                            result.push(out_line(s));
                                        }
                                        let remaining: Vec<&str> = lines_iter.collect();
                                        for line in remaining.iter().take(5) {
                                            let marker = line.get(6..8).unwrap_or("  ");
                                            let style = match marker {
                                                " +" => Style::default().fg(Color::Green),
                                                " -" => Style::default().fg(Color::Red),
                                                _ => dim,
                                            };
                                            result.push(Line::from(Span::styled(
                                                format!("      {line}"),
                                                style,
                                            )));
                                        }
                                        if remaining.len() > 5 {
                                            result.push(out_line(&format!(
                                                "\u{2026} +{} more lines",
                                                remaining.len() - 5
                                            )));
                                        }
                                    }
                                    "web_fetch" => {
                                        let chars = output.chars().count();
                                        result.push(out_line(&format!(
                                            "Fetched {} chars",
                                            fmt_count(chars)
                                        )));
                                    }
                                    "web_search" => {
                                        // Count result entries (each starts with a URL line)
                                        let count = output
                                            .lines()
                                            .filter(|l| l.starts_with("URL:"))
                                            .count();
                                        if count > 0 {
                                            result.push(out_line(&format!(
                                                "Found {count} result(s)"
                                            )));
                                        } else {
                                            result.push(out_line(
                                                output.lines().next().unwrap_or("No results"),
                                            ));
                                        }
                                    }
                                    _ => {
                                        if tc.is_error {
                                            let first = output.lines().next().unwrap_or("Error");
                                            result.push(Line::from(vec![
                                                Span::styled("  \u{23bf}  ", dim),
                                                Span::styled(
                                                    truncate_line(first, 120),
                                                    Style::default().fg(Color::Red),
                                                ),
                                            ]));
                                        } else {
                                            let first = output.lines().next().unwrap_or("Done");
                                            result.push(out_line(&truncate_line(first, 120)));
                                        }
                                    }
                                }
                            }
                        }
                        result.push(Line::from(""));
                    }
                }

                // Debug events that arrived after all turns (most recent activity)
                if debug_mode {
                    while dbg_idx < session.debug_events.len() {
                        result.push(Line::from(Span::styled(
                            format!("  \u{2812} {}", session.debug_events[dbg_idx].1),
                            debug_dim,
                        )));
                        dbg_idx += 1;
                    }
                }

                // ── Streaming / tool-execution status line ────────────
                let is_streaming = session.conversation.last_is_streaming();
                let tool_running = session.active_tool.is_some();
                let is_processing = is_streaming && self.stream_started.is_none();

                if is_streaming || tool_running {
                    let (verb, elapsed_str, tokens_str, tip) = self.thinking_status(session);
                    let effort_suffix = session
                        .effort
                        .as_deref()
                        .map(|e| format!(" \u{00b7} {e} effort"))
                        .unwrap_or_default();
                    const GROWING: &[&str] =
                        &["\u{00b7}", "\u{2022}", "\u{25cf}", "\u{2022}", "\u{00b7}"];
                    const FALLING_SAND: &[&str] = &[
                        "⠁", "⠂", "⠄", "⡀", "⡈", "⡐", "⡠", "⣀", "⣁", "⣂", "⣄", "⣌", "⣔", "⣤", "⣥",
                        "⣦", "⣮", "⣶", "⣷", "⣿", "⡿", "⠿", "⢟", "⠟", "⡛", "⠛", "⠫", "⢋", "⠋", "⠍",
                        "⡉", "⠉", "⠑", "⠡", "⢁",
                    ];
                    const FOLD: &[&str] = &["-", "≻", "›", "⟩", "|", "⟨", "‹", "≺"];
                    const BOX_BOUNCE: &[&str] = &["▖", "▘", "▝", "▗"];
                    const BRAILLE: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                    let frames: &[&str] = match self.processing_dot_style % 5 {
                        0 => GROWING,
                        1 => FALLING_SAND,
                        2 => FOLD,
                        3 => BOX_BOUNCE,
                        _ => BRAILLE,
                    };
                    let spinner = frames[self.spinner_tick as usize % frames.len()];
                    let spinner_span = Span::styled(
                        format!("{spinner} "),
                        Style::default().fg(dot_color).add_modifier(Modifier::DIM),
                    );
                    let verb_span = Span::styled(
                        format!("{verb}\u{2026}"),
                        Style::default().fg(dot_color).add_modifier(Modifier::BOLD),
                    );

                    if is_processing {
                        // Waiting for first token — model is processing the request
                        result.push(Line::from(vec![spinner_span, verb_span]));
                    } else {
                        // Receiving tokens or tool running — show elapsed + token stats
                        let elapsed_str = if tool_running && !is_streaming {
                            // Tool execution: use per-tool timer
                            self.tool_exec_started
                                .map(|s| {
                                    let secs = s.elapsed().as_secs();
                                    if secs >= 60 {
                                        format!("{}m {:02}s", secs / 60, secs % 60)
                                    } else {
                                        format!("{}s", secs)
                                    }
                                })
                                .unwrap_or_else(|| "0s".to_string())
                        } else {
                            elapsed_str
                        };
                        result.push(Line::from(vec![
                            spinner_span,
                            verb_span,
                            Span::styled(
                                format!(" ({elapsed_str} \u{00b7} {tokens_str}{effort_suffix})"),
                                Style::default().fg(Color::DarkGray),
                            ),
                        ]));
                        result.push(Line::from(vec![
                            Span::styled("  \u{23bf}  ", Style::default().fg(Color::DarkGray)),
                            Span::styled(
                                format!("Tip: {tip}"),
                                Style::default().fg(Color::DarkGray),
                            ),
                        ]));
                    }
                    result.push(Line::from(""));
                }

                result
            }
        } else {
            vec![
                Line::from(""),
                Line::from(Span::styled(
                    "  Welcome to One.",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "  Type a message below and press Enter to start chatting.",
                    Style::default().fg(Color::White),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "  Quick start:",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    "    Type /         to see available commands",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(Span::styled(
                    "    Ctrl+N / P     to switch between project tabs",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(Span::styled(
                    "    Ctrl+C         to quit",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "  Open more projects with /new <path>",
                    Style::default().fg(Color::DarkGray),
                )),
            ]
        };

        let text = Text::from(lines);
        // Available width inside borders
        let inner_width = area.width.saturating_sub(2) as usize;
        let visible_height = area.height.saturating_sub(2);

        // Calculate WRAPPED content height — each line may wrap to multiple rows.
        // This is critical: without accounting for wrapping, scroll calculation
        // is wrong and content extends below the visible area.
        let content_height: u16 = text
            .lines
            .iter()
            .map(|line| {
                if inner_width == 0 {
                    return 1;
                }
                let line_width: usize = line.spans.iter().map(|s| s.content.len()).sum();
                if line_width == 0 {
                    1 // empty lines still take 1 row
                } else {
                    line_width.div_ceil(inner_width) as u16
                }
            })
            .sum();

        // Auto-scroll to bottom — newest messages always visible.
        // messages_scroll lets the user scroll UP from the bottom to see history.
        let max_scroll = content_height.saturating_sub(visible_height);
        let scroll_offset = max_scroll.saturating_sub(self.messages_scroll.min(max_scroll));

        let content = Paragraph::new(text)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((scroll_offset, 0));

        f.render_widget(content, area);
    }

    fn draw_inbox(&self, f: &mut Frame, area: Rect, snapshot: &one_core::state::AppState) {
        let block = Block::default()
            .borders(Borders::LEFT | Borders::RIGHT)
            .title(" inbox ");

        let lines: Vec<Line> = if snapshot.notifications.is_empty() {
            vec![
                Line::from(""),
                Line::from(Span::styled(
                    "  No notifications yet.",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "  Configure integrations in ~/.one/config.toml:",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(Span::styled(
                    "    [integrations.github]",
                    Style::default().fg(Color::White),
                )),
                Line::from(Span::styled(
                    "    token = \"ghp_...\"",
                    Style::default().fg(Color::White),
                )),
                Line::from(Span::styled(
                    "    repos = [\"owner/repo\"]",
                    Style::default().fg(Color::White),
                )),
            ]
        } else {
            let mut result = Vec::new();
            result.push(Line::from(Span::styled(
                format!("  {} notifications", snapshot.notifications.len()),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
            result.push(Line::from(""));

            for notif in snapshot.notifications.iter().rev().take(50) {
                let (icon, color) = match notif.source {
                    one_core::event::NotificationSource::GitHub => ("", Color::White),
                    one_core::event::NotificationSource::Slack => ("#", Color::Magenta),
                    one_core::event::NotificationSource::Asana => ("*", Color::Red),
                    one_core::event::NotificationSource::Notion => ("N", Color::Blue),
                };

                let time = notif.timestamp.format("%H:%M").to_string();

                result.push(Line::from(vec![
                    Span::styled(format!("[{icon}]"), Style::default().fg(color)),
                    Span::raw(" "),
                    Span::styled(time, Style::default().fg(Color::DarkGray)),
                    Span::raw(" "),
                    Span::styled(&notif.title, Style::default().fg(Color::White)),
                ]));

                if !notif.body.is_empty() {
                    result.push(Line::from(Span::styled(
                        format!("       {}", notif.body),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
            result
        };

        let content = Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((self.messages_scroll, 0));

        f.render_widget(content, area);
    }
}

impl App {
    fn draw_input(&self, f: &mut Frame, area: Rect, snapshot: &one_core::state::AppState) {
        let border_color = if self.bash_mode {
            Color::Yellow
        } else {
            Color::Cyan
        };

        // Get current branch if available
        let branch = snapshot
            .active_session()
            .and_then(|s| one_core::worktree::get_current_branch(&s.cwd));

        // Build title differently for normal mode vs bash mode
        let title = if self.bash_mode {
            // Bash mode: PS1-like format with working directory and branch
            let pwd = snapshot
                .active_session()
                .map(|s| {
                    let path = &s.cwd;
                    if let Some(home) = dirs_next::home_dir() {
                        let home_str = home.to_string_lossy();
                        if path.starts_with(home_str.as_ref()) {
                            return format!("~{}", &path[home_str.len()..]);
                        }
                    }
                    path.clone()
                })
                .unwrap_or_default();

            // Build the full prompt: "~/path (branch) $"
            let branch_part = branch
                .as_ref()
                .map(|b| format!(" ({})", b))
                .unwrap_or_default();
            let mut full_title = format!("{}{} $", pwd, branch_part);

            // Truncate to max 80 characters, preserving the branch at the end
            if full_title.len() > 80 {
                let max_path_len = 80_usize.saturating_sub(branch_part.len() + 2); // 2 for " $"
                if max_path_len > 10 {
                    let keep = (max_path_len - 3) / 2; // 3 for "..."
                    let path_truncated = format!("{}...{}", &pwd[..keep], &pwd[pwd.len() - keep..]);
                    full_title = format!("{}{} $", path_truncated, branch_part);
                }
            }
            format!(" {} ", full_title)
        } else {
            // Normal mode: just show branch
            let branch_name = branch.as_deref().unwrap_or("unknown");
            format!(" > ({}) ", branch_name)
        };

        let (display_text, text_style) = if let Some(ref placeholder) = self.input.placeholder {
            (placeholder.as_str(), Style::default().fg(Color::Yellow))
        } else {
            let color = if self.bash_mode {
                Color::Yellow
            } else {
                Color::White
            };
            (self.input.value(), Style::default().fg(color))
        };

        let input = Paragraph::new(display_text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .border_style(Style::default().fg(border_color)),
            )
            .style(text_style);

        f.render_widget(input, area);
        f.set_cursor_position((area.x + self.input.cursor_position() as u16 + 1, area.y + 1));
    }

    /// Render inline help overlay (triggered by typing '?').
    fn draw_help_overlay(&self, f: &mut Frame, area: Rect) {
        let dim = Style::default().fg(Color::DarkGray);
        let key_style = Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD);

        let lines = vec![
            Line::from(""),
            Line::from(vec![Span::styled(
                "  Keyboard shortcuts",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )]),
            Line::from(""),
            Line::from(vec![
                Span::styled("  Enter        ", key_style),
                Span::styled("Send message", dim),
            ]),
            Line::from(vec![
                Span::styled("  Shift+Enter  ", key_style),
                Span::styled("New line", dim),
            ]),
            Line::from(vec![
                Span::styled("  Up/Down      ", key_style),
                Span::styled("Input history", dim),
            ]),
            Line::from(vec![
                Span::styled("  PageUp/Down  ", key_style),
                Span::styled("Scroll conversation", dim),
            ]),
            Line::from(vec![
                Span::styled("  Ctrl+O       ", key_style),
                Span::styled("Open transcript", dim),
            ]),
            Line::from(vec![
                Span::styled("  Ctrl+T       ", key_style),
                Span::styled("New session", dim),
            ]),
            Line::from(vec![
                Span::styled("  Ctrl+W       ", key_style),
                Span::styled("Close session", dim),
            ]),
            Line::from(vec![
                Span::styled("  Ctrl+Shift+] ", key_style),
                Span::styled("Next tab", dim),
            ]),
            Line::from(vec![
                Span::styled("  Ctrl+Shift+[ ", key_style),
                Span::styled("Previous tab", dim),
            ]),
            Line::from(vec![
                Span::styled("  Ctrl+C       ", key_style),
                Span::styled("Clear / cancel / exit", dim),
            ]),
            Line::from(vec![
                Span::styled("  Escape       ", key_style),
                Span::styled("Abort / dismiss", dim),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled("  ! command    ", key_style),
                Span::styled("Run shell command", dim),
            ]),
            Line::from(vec![
                Span::styled("  /command     ", key_style),
                Span::styled("Slash command (Tab to complete)", dim),
            ]),
            Line::from(vec![
                Span::styled("  @file        ", key_style),
                Span::styled("Include file contents", dim),
            ]),
            Line::from(""),
            Line::from(Span::styled("  Press Escape or any key to dismiss", dim)),
        ];

        let content = Paragraph::new(Text::from(lines))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" ? help ")
                    .border_style(Style::default().fg(Color::Cyan)),
            )
            .wrap(Wrap { trim: false });
        f.render_widget(content, area);
    }
}

/// Detect which AI providers have credentials configured.
/// Returns a list of providers with their auth status.
fn detect_configured_providers() -> Vec<ConfiguredProvider> {
    use one_core::provider::Provider;

    let checks: &[(&str, &str, Provider, &str, &str)] = &[
        (
            "anthropic",
            "ANTHROPIC_API_KEY",
            Provider::Anthropic,
            "Anthropic (Claude)",
            "claude-sonnet-4-20250514",
        ),
        (
            "openai",
            "OPENAI_API_KEY",
            Provider::OpenAI,
            "OpenAI (GPT)",
            "gpt-4o",
        ),
        (
            "google",
            "GOOGLE_API_KEY",
            Provider::Google,
            "Google (Gemini)",
            "gemini-2.0-flash",
        ),
        (
            "huggingface",
            "HF_TOKEN",
            Provider::HuggingFace,
            "Hugging Face",
            "meta-llama/Llama-3.3-70B-Instruct",
        ),
    ];

    let mut configured = Vec::new();

    for &(name, env_var, provider, label, default_model) in checks {
        // Check OAuth tokens
        let oauth = one_core::credentials::CredentialStore::get(&format!("{name}_oauth"))
            .ok()
            .flatten()
            .and_then(|json| serde_json::from_str::<one_core::oauth::OAuthTokens>(&json).ok())
            .filter(|t| !t.is_expired());

        let keyring = one_core::credentials::CredentialStore::get(name)
            .ok()
            .flatten()
            .is_some();
        let env = !env_var.is_empty() && std::env::var(env_var).is_ok();

        if oauth.is_some() || keyring || env {
            let status = if oauth.is_some() {
                "oauth".to_string()
            } else if keyring {
                "api key".to_string()
            } else {
                "env var".to_string()
            };
            configured.push(ConfiguredProvider {
                provider,
                model: default_model.to_string(),
                label,
                status,
            });
        }
    }

    // Ollama is always available (local, no auth)
    configured.push(ConfiguredProvider {
        provider: Provider::Ollama,
        model: "llama3".to_string(),
        label: "Ollama (local)",
        status: "no auth needed".to_string(),
    });

    configured
}

/// Format a token count for display: 1234 → "1.2k", 12345 → "12k", etc.
/// Get the context window (input token limit) for a model.
fn context_window_for_model(model: &str) -> u64 {
    let lower = model.to_lowercase();
    // Anthropic
    if lower.contains("opus-4-6") {
        return 1_000_000;
    }
    if lower.contains("sonnet-4-6") {
        return 1_000_000;
    }
    if lower.contains("sonnet-4") {
        return 200_000;
    }
    if lower.contains("haiku") {
        return 200_000;
    }
    if lower.starts_with("claude-") {
        return 200_000;
    }
    // OpenAI
    if lower.starts_with("gpt-4.1") {
        return 1_047_576;
    }
    if lower.starts_with("gpt-4o") {
        return 128_000;
    }
    if lower.starts_with("o3") || lower.starts_with("o4") {
        return 200_000;
    }
    if lower.starts_with("o1") {
        return 200_000;
    }
    // Google
    if lower.starts_with("gemini-2.5") {
        return 1_048_576;
    }
    if lower.starts_with("gemini") {
        return 1_048_576;
    }
    // Default
    128_000
}

/// Shell-aware tab completion for bash mode.
/// Delegates to bash's `compgen` for both command and file completion,
/// which handles tilde expansion, PATH lookup, and all shell semantics.
fn shell_tab_complete(input: &str, working_dir: &str) -> Option<String> {
    let trimmed = input.trim_end();
    if trimmed.is_empty() {
        return None;
    }

    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    let partial = parts.last().copied().unwrap_or("");
    if partial.is_empty() {
        return None;
    }
    let is_first_token = parts.len() <= 1;

    // Use bash compgen for file + directory completion (handles ~, $HOME, etc.)
    let compgen_type = if is_first_token { "-c -d -f" } else { "-d -f" };
    let script = format!(
        "cd {working_dir:?} 2>/dev/null; compgen {compgen_type} -- {partial:?} 2>/dev/null | head -30"
    );
    let output = std::process::Command::new("bash")
        .args(["-c", &script])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let completions: Vec<String> = std::str::from_utf8(&output.stdout)
        .ok()?
        .lines()
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();

    if completions.is_empty() {
        return None;
    }

    let build_result = |completed: &str| -> String {
        let prefix_parts = &parts[..parts.len() - 1];
        let mut result = prefix_parts.join(" ");
        if !result.is_empty() {
            result.push(' ');
        }
        result.push_str(completed);
        // Add trailing slash for directories
        let expanded = if completed.starts_with('~') {
            // Expand ~ for the is_dir check
            dirs_next::home_dir()
                .map(|h| completed.replacen('~', &h.to_string_lossy(), 1))
                .unwrap_or_else(|| completed.to_string())
        } else if std::path::Path::new(completed).is_absolute() {
            completed.to_string()
        } else {
            std::path::Path::new(working_dir)
                .join(completed)
                .to_string_lossy()
                .to_string()
        };
        if std::path::Path::new(&expanded).is_dir() && !result.ends_with('/') {
            result.push('/');
        }
        result
    };

    if completions.len() == 1 {
        return Some(build_result(&completions[0]));
    }

    // Multiple matches — extend to longest common prefix
    if let Some(common) = longest_common_prefix(&completions)
        && common.len() > partial.len()
    {
        return Some(build_result(&common));
    }

    None
}

/// Find the longest common prefix among a set of strings.
fn longest_common_prefix(strings: &[String]) -> Option<String> {
    if strings.is_empty() {
        return None;
    }
    let first = &strings[0];
    let mut len = first.len();
    for s in &strings[1..] {
        len = first
            .chars()
            .zip(s.chars())
            .take_while(|(a, b)| a == b)
            .count()
            .min(len);
    }
    if len > 0 {
        Some(
            first[..first
                .char_indices()
                .nth(len)
                .map(|(i, _)| i)
                .unwrap_or(first.len())]
                .to_string(),
        )
    } else {
        None
    }
}

/// Format a character/byte count as "1,234" or "12.3k".
fn fmt_count(n: usize) -> String {
    if n < 10_000 {
        format!("{n}")
    } else if n < 1_000_000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}

/// Truncate a single line to `max_chars` characters, appending `…` if cut.
fn truncate_line(line: &str, max_chars: usize) -> String {
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= max_chars {
        line.to_string()
    } else {
        chars[..max_chars].iter().collect::<String>() + "…"
    }
}

fn format_token_count(tokens: u64) -> String {
    if tokens < 1000 {
        tokens.to_string()
    } else if tokens < 10_000 {
        format!("{:.1}k", tokens as f64 / 1000.0)
    } else if tokens < 1_000_000 {
        format!("{}k", tokens / 1000)
    } else {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    }
}

/// Derive a short (1-2 word) display title from the first AI response.
/// Strips markdown, filters stop-words, title-cases the result.
fn derive_tab_title(content: &str) -> String {
    const STOP_WORDS: &[&str] = &[
        "i", "a", "an", "the", "is", "are", "was", "were", "to", "of", "for", "in", "on", "at",
        "by", "as", "be", "it", "do", "so", "or", "if", "no", "my", "we", "me", "he", "she",
        "they", "you", "your", "this", "that", "with", "from", "not", "can", "will", "have", "has",
        "had", "but", "and", "here", "let", "ll", "ve", "re", "s", "d", "m",
    ];

    // Strip markdown characters; keep alphanumeric, hyphens, spaces
    let cleaned: String = content
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c.is_whitespace() {
                c
            } else {
                ' '
            }
        })
        .collect();

    let words: Vec<String> = cleaned
        .split_whitespace()
        .filter(|w| {
            let lw = w.to_lowercase();
            w.len() > 2 && !STOP_WORDS.contains(&lw.as_str())
        })
        .take(2)
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect();

    let title = words.join(" ");
    if title.is_empty() {
        // Fallback: truncate raw content
        content
            .chars()
            .take(12)
            .collect::<String>()
            .trim()
            .to_string()
    } else if title.len() > 20 {
        title[..20].trim_end().to_string()
    } else {
        title
    }
}
