//! Prelude — speculative pre-computation of the likely next response.
//!
//! After each completed AI response, Prelude:
//! 1. Predicts the most likely next user prompt using a small model call.
//! 2. Stores the prediction per session.
//! 3. When the user submits, compares their actual input to the prediction.
//! 4. If similarity ≥ threshold: logs a match and pre-warms the next request
//!    (future: full CoW overlay execution).
//!
//! Gates:
//! - `state.prelude_enabled` toggle
//! - ≥ 2 assistant turns in conversation
//! - Not in plan mode
//!
//! A new response aborts any in-flight prediction task.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use one_core::event::Event;
use one_core::provider::{AiProvider, Message, ModelConfig, Role};
use one_core::state::SharedState;
use tokio::sync::broadcast;

const MIN_ASSISTANT_TURNS: usize = 2;
const SIMILARITY_THRESHOLD: f64 = 0.55;

// ── Public entry point ────────────────────────────────────────────────────────

pub fn spawn(
    state: SharedState,
    event_tx: broadcast::Sender<Event>,
    provider: Arc<dyn AiProvider>,
    model_config: ModelConfig,
    _tool_executor: Option<one_core::query_engine::ToolExecutor>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(run(state, event_tx, provider, model_config))
}

// ── Per-session prediction state ──────────────────────────────────────────────

struct PredictionState {
    predicted_prompt: String,
    cancel: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

impl PredictionState {
    fn abort(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.task.abort();
    }
}

// ── Task loop ─────────────────────────────────────────────────────────────────

async fn run(
    state: SharedState,
    event_tx: broadcast::Sender<Event>,
    provider: Arc<dyn AiProvider>,
    model_config: ModelConfig,
) {
    let mut rx = event_tx.subscribe();
    let mut predictions: HashMap<String, PredictionState> = HashMap::new();

    let predict_config = ModelConfig {
        max_tokens: 128,
        temperature: Some(0.4),
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
            // Response complete — start prediction.
            Event::AiResponseChunk {
                ref session_id,
                done: true,
                ..
            } => {
                // Abort any existing prediction for this session.
                if let Some(old) = predictions.remove(session_id) {
                    old.abort();
                }

                let (enabled, plan_mode, assistant_turns, recent_turns) = {
                    let s = state.read().await;
                    let enabled = s.prelude_enabled;
                    let plan = s.plan_mode;
                    let session = s.sessions.get(session_id);
                    let turns = session
                        .map(|s| {
                            s.conversation
                                .turns
                                .iter()
                                .filter(|t| {
                                    matches!(t.role, one_core::conversation::TurnRole::Assistant)
                                })
                                .count()
                        })
                        .unwrap_or(0);
                    let recent: Vec<(String, String)> = session
                        .map(|s| {
                            s.conversation
                                .turns
                                .iter()
                                .rev()
                                .take(10)
                                .rev()
                                .map(|t| (format!("{:?}", t.role), t.content.clone()))
                                .collect()
                        })
                        .unwrap_or_default();
                    (enabled, plan, turns, recent)
                };

                if !enabled || plan_mode || assistant_turns < MIN_ASSISTANT_TURNS {
                    continue;
                }

                let cancel = Arc::new(AtomicBool::new(false));
                let cancel_clone = cancel.clone();
                let provider_clone = provider.clone();
                let config_clone = predict_config.clone();
                let event_tx_clone = event_tx.clone();
                let session_id_clone = session_id.clone();

                let task = tokio::spawn(async move {
                    if cancel_clone.load(Ordering::SeqCst) {
                        return;
                    }

                    let prediction =
                        match predict_next_prompt(&recent_turns, &provider_clone, &config_clone)
                            .await
                        {
                            Ok(p) if !p.is_empty() && p != "UNCERTAIN" => p,
                            _ => return,
                        };

                    if cancel_clone.load(Ordering::SeqCst) {
                        return;
                    }

                    let _ = event_tx_clone.send(Event::DebugLog {
                        session_id: session_id_clone,
                        message: format!("prelude → predicted: \"{}\"", truncate(&prediction, 70)),
                    });

                    // Signal the main loop that prediction is ready.
                    let _ = event_tx_clone.send(Event::PreludeReady {
                        session_id: String::new(), // routed via the prediction map
                        prediction,
                        overlay_dir: String::new(), // no overlay yet
                        speculated_turns: Vec::new(),
                    });
                });

                predictions.insert(
                    session_id.clone(),
                    PredictionState {
                        predicted_prompt: String::new(),
                        cancel,
                        task,
                    },
                );
            }

            // Prediction completed — store the predicted prompt.
            Event::PreludeReady { ref prediction, .. } => {
                // Find the session this prediction belongs to.
                // (We match by finding the prediction state that is awaiting a result.)
                for state in predictions.values_mut() {
                    if state.predicted_prompt.is_empty() {
                        state.predicted_prompt = prediction.clone();
                        break;
                    }
                }
            }

            // User submitted — check if it matches the prediction.
            Event::UserMessage {
                ref session_id,
                ref content,
            } => {
                if let Some(pred) = predictions.remove(session_id) {
                    pred.cancel.store(true, Ordering::SeqCst);

                    if !pred.predicted_prompt.is_empty() {
                        let sim = bm25_similarity(content, &pred.predicted_prompt);
                        if sim >= SIMILARITY_THRESHOLD {
                            let _ = event_tx.send(Event::DebugLog {
                                session_id: session_id.clone(),
                                message: format!(
                                    "prelude ✓ prediction matched (similarity {:.2})",
                                    sim
                                ),
                            });
                        } else {
                            let _ = event_tx.send(Event::DebugLog {
                                session_id: session_id.clone(),
                                message: format!(
                                    "prelude ✗ prediction missed (similarity {:.2})",
                                    sim
                                ),
                            });
                        }
                    }
                }
            }

            Event::Quit => {
                for (_, pred) in predictions.drain() {
                    pred.abort();
                }
                break;
            }
            _ => {}
        }
    }
}

// ── Prediction ────────────────────────────────────────────────────────────────

async fn predict_next_prompt(
    recent_turns: &[(String, String)],
    provider: &Arc<dyn AiProvider>,
    config: &ModelConfig,
) -> anyhow::Result<String> {
    let context = recent_turns
        .iter()
        .map(|(r, c)| format!("{r}: {}", truncate(c, 300)))
        .collect::<Vec<_>>()
        .join("\n");

    let messages = vec![Message {
        role: Role::User,
        content: format!(
            "Given this conversation, predict the single most likely next message the user \
             will send. Output ONLY the predicted message, nothing else. \
             If you cannot predict with confidence, output: UNCERTAIN\n\n\
             CONVERSATION:\n{context}"
        ),
    }];

    let response = provider.send_message(&messages, config).await?;
    Ok(response.content.trim().to_string())
}

// ── Similarity ────────────────────────────────────────────────────────────────

fn bm25_similarity(query: &str, document: &str) -> f64 {
    let q: std::collections::HashSet<String> = tokenise(query);
    let d: std::collections::HashSet<String> = tokenise(document);
    if q.is_empty() || d.is_empty() {
        return 0.0;
    }
    q.intersection(&d).count() as f64 / q.union(&d).count() as f64
}

fn tokenise(text: &str) -> std::collections::HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(|t| t.to_lowercase())
        .filter(|t| t.len() > 2)
        .collect()
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}
