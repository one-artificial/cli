//! Palimpsest — living document maintenance.
//!
//! Watches for markdown files containing `<!-- one:autodoc -->` being read by
//! the AI. When detected, spawns a background pass that updates the file with
//! learnings from the current session.
//!
//! Design:
//! - `ToolRequest` for Read → record call_id → file_path
//! - `ToolResult` for known call_id → check for autodoc marker
//! - If found → sequential per-file update (Mutex prevents torn writes)
//! - Update: read file + recent session messages, call model, write result
//! - Deregisters if file deleted or marker removed

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use one_core::event::Event;
use one_core::provider::{AiProvider, Message, ModelConfig, Role};
use one_core::state::SharedState;
use tokio::sync::{Mutex, broadcast};

const AUTODOC_MARKER: &str = "<!-- one:autodoc -->";
const MAX_CONTEXT_TURNS: usize = 20;

// ── Public entry point ────────────────────────────────────────────────────────

pub fn spawn(
    state: SharedState,
    event_tx: broadcast::Sender<Event>,
    provider: Arc<dyn AiProvider>,
    model_config: ModelConfig,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(run(state, event_tx, provider, model_config))
}

// ── Task loop ─────────────────────────────────────────────────────────────────

async fn run(
    state: SharedState,
    event_tx: broadcast::Sender<Event>,
    provider: Arc<dyn AiProvider>,
    model_config: ModelConfig,
) {
    let mut rx = event_tx.subscribe();
    // call_id → (session_id, file_path) for pending Read calls
    let mut pending_reads: HashMap<String, (String, String)> = HashMap::new();
    // Per-file sequential lock — prevents concurrent writes to the same doc
    let file_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let update_config = ModelConfig {
        max_tokens: 2048,
        temperature: Some(0.3),
        budget_tokens: None,
        ..model_config
    };

    loop {
        let evt = match rx.recv().await {
            Ok(e) => e,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        };

        match evt {
            // Track Read tool calls so we know which file each ToolResult relates to.
            Event::ToolRequest {
                ref session_id,
                ref tool_name,
                ref input,
                ref call_id,
            } if tool_name == "Read" => {
                let file_path = input["file_path"]
                    .as_str()
                    .or_else(|| input["path"].as_str())
                    .unwrap_or("")
                    .to_string();
                if !file_path.is_empty() {
                    pending_reads.insert(call_id.clone(), (session_id.clone(), file_path));
                }
            }

            // Check the result of a Read for the autodoc marker.
            Event::ToolResult {
                ref call_id,
                ref output,
                is_error,
                ..
            } if !is_error => {
                if let Some((session_id, file_path)) = pending_reads.remove(call_id)
                    && output.contains(AUTODOC_MARKER)
                {
                    {
                        let enabled = state.read().await.palimpsest_enabled;
                        if !enabled {
                            continue;
                        }
                    }

                    let _ = event_tx.send(Event::DebugLog {
                        session_id: session_id.clone(),
                        message: format!(
                            "palimpsest → autodoc detected in {}",
                            short_path(&file_path)
                        ),
                    });

                    // Acquire per-file lock to serialise updates.
                    let file_lock = {
                        let mut locks = file_locks.lock().await;
                        locks
                            .entry(file_path.clone())
                            .or_insert_with(|| Arc::new(Mutex::new(())))
                            .clone()
                    };

                    // Snapshot conversation context while we have state access.
                    let context_turns: Vec<(String, String)> = {
                        let s = state.read().await;
                        s.sessions
                            .get(&session_id)
                            .map(|sess| {
                                sess.conversation
                                    .turns
                                    .iter()
                                    .rev()
                                    .take(MAX_CONTEXT_TURNS)
                                    .rev()
                                    .map(|t| (format!("{:?}", t.role), t.content.clone()))
                                    .collect()
                            })
                            .unwrap_or_default()
                    };

                    // Spawn the update without blocking the event loop.
                    let provider_clone = provider.clone();
                    let config_clone = update_config.clone();
                    let event_tx_clone = event_tx.clone();
                    let file_path_clone = file_path.clone();
                    let session_id_clone = session_id.clone();

                    tokio::spawn(async move {
                        let _guard = file_lock.lock().await;
                        match update_autodoc(
                            &file_path_clone,
                            &context_turns,
                            &provider_clone,
                            &config_clone,
                        )
                        .await
                        {
                            Ok(true) => {
                                let _ = event_tx_clone.send(Event::DebugLog {
                                    session_id: session_id_clone,
                                    message: format!(
                                        "palimpsest ← updated {}",
                                        short_path(&file_path_clone)
                                    ),
                                });
                            }
                            Ok(false) => {
                                // Marker removed or file gone — stop tracking silently
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "palimpsest: update failed for {file_path_clone}: {e}"
                                );
                                let _ = event_tx_clone.send(Event::DebugLog {
                                    session_id: session_id_clone,
                                    message: format!("palimpsest: update failed — {e}"),
                                });
                            }
                        }
                    });
                }

                // Clean up stale entries for ToolResult events with no matching request.
                pending_reads.retain(|_, _| true); // keep all — cleaned on match above
            }

            Event::Quit => break,
            _ => {}
        }

        // Prune stale pending_reads (cap at 200 entries).
        if pending_reads.len() > 200 {
            let excess = pending_reads.len() - 200;
            let keys: Vec<_> = pending_reads.keys().take(excess).cloned().collect();
            for k in keys {
                pending_reads.remove(&k);
            }
        }
    }
}

// ── Document update ───────────────────────────────────────────────────────────

/// Returns `Ok(true)` if the file was updated, `Ok(false)` if deregistered.
async fn update_autodoc(
    file_path: &str,
    context_turns: &[(String, String)],
    provider: &Arc<dyn AiProvider>,
    config: &ModelConfig,
) -> anyhow::Result<bool> {
    let path = PathBuf::from(file_path);

    let current = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };

    // Deregister if marker was removed.
    if !current.contains(AUTODOC_MARKER) {
        return Ok(false);
    }

    let context_text = context_turns
        .iter()
        .map(|(role, content)| {
            let truncated = if content.len() > 500 {
                format!("{}…", &content[..500])
            } else {
                content.clone()
            };
            format!("{role}: {truncated}")
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    let prompt = format!(
        "You are maintaining a living document. Update it with relevant learnings from \
         the recent conversation below. Preserve the existing structure, style, and \
         the `{AUTODOC_MARKER}` marker exactly as-is.\n\n\
         Guidelines:\n\
         - Add at most 1-3 bullet points per section\n\
         - Remove outdated information if you can identify it\n\
         - Keep additions concise and factual\n\
         - Output the complete updated document only — no preamble or explanation\n\n\
         CURRENT DOCUMENT:\n{current}\n\n\
         RECENT CONVERSATION (for context):\n{context_text}"
    );

    let messages = vec![Message {
        role: Role::User,
        content: prompt,
    }];

    let response = provider.send_message(&messages, config).await?;
    let updated = response.content.trim();

    if updated.is_empty() {
        return Ok(true); // Model returned nothing — leave file unchanged
    }

    // Sanity check: updated doc must still contain the marker.
    if !updated.contains(AUTODOC_MARKER) {
        return Err(anyhow::anyhow!(
            "model removed autodoc marker — refusing to write"
        ));
    }

    std::fs::write(&path, updated)?;
    Ok(true)
}

fn short_path(path: &str) -> String {
    PathBuf::from(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
        .to_string()
}
