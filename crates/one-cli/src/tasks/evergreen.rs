//! Background Evergreen compression task.
//!
//! Listens for completed AI response turns and, when the session has
//! accumulated enough uncompressed history, runs tiered compression:
//!
//! 1. Reads eligible turns from the per-session SQLite DB.
//! 2. Checks the ROI gate — skips if savings don't exceed compression cost.
//! 3. Calls the AI provider to summarize the eligible span.
//! 4. Writes an `evergreen_chunks` record and marks source messages compressed.
//! 5. Emits `Event::EvergreenCompressed` on the event bus.
//!
//! Only sessions that have a `db_path` set (new-style filesystem sessions)
//! are processed.  Legacy UUID-based sessions are silently skipped.

use std::path::PathBuf;
use std::sync::Arc;

use one_core::event::Event;
use one_core::evergreen::{EvergreenConfig, estimate_tokens, plan_compression, roi_gate};
use one_core::provider::{AiProvider, Message, ModelConfig, Role};
use one_core::state::SharedState;
use one_db::SessionDb;
use tokio::sync::broadcast;

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
    let cfg = EvergreenConfig::default();

    // Summarization uses a small output budget — we want concise compression,
    // not a full-length response.  Lower temperature keeps the summary factual.
    let compress_config = ModelConfig {
        max_tokens: 1024,
        temperature: Some(0.2),
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
            // A full AI response turn has just completed — this is our trigger point.
            // The persistence task is also subscribed and will have written the turn
            // to SessionDb; the 100 ms sleep gives it a head start.
            Event::AiResponseChunk {
                session_id,
                done: true,
                ..
            } => {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;

                if let Err(e) = maybe_compress(
                    &session_id,
                    &state,
                    &event_tx,
                    &provider,
                    &compress_config,
                    &cfg,
                )
                .await
                {
                    tracing::warn!("evergreen: compression attempt failed for {session_id}: {e}");
                }
            }
            Event::Quit => break,
            _ => {}
        }
    }
}

// ── Compression logic ─────────────────────────────────────────────────────────

/// Intermediate data read from the DB before the AI summarization call.
struct SpanData {
    span_start_id: i64,
    span_end_id: i64,
    /// Chronological (role, content) pairs for the span to summarize.
    turns: Vec<(String, String)>,
}

async fn maybe_compress(
    session_id: &str,
    state: &SharedState,
    event_tx: &broadcast::Sender<Event>,
    provider: &Arc<dyn AiProvider>,
    compress_config: &ModelConfig,
    cfg: &EvergreenConfig,
) -> anyhow::Result<()> {
    // Resolve db_path — legacy sessions have an empty PathBuf; skip them.
    let db_path: PathBuf = {
        let s = state.read().await;
        let Some(session) = s.sessions.get(session_id) else {
            return Ok(());
        };
        if session.db_path.as_os_str().is_empty() {
            return Ok(());
        }
        session.db_path.clone()
    };

    // ── Phase 1: decide whether compression is worthwhile (blocking DB reads) ──

    let cfg1 = cfg.clone();
    let db_path1 = db_path.clone();

    let span_data: Option<SpanData> = tokio::task::spawn_blocking(move || {
        let db = SessionDb::open(&db_path1)?;
        let uncompressed_count = db.uncompressed_message_count()?;

        let Some(plan) = plan_compression(uncompressed_count, &cfg1) else {
            return anyhow::Ok(None);
        };

        // Prefer archive (2nd-pass) over compress (1st-pass) — higher ROI per call.
        let batch = if let Some(b) = plan.archive_batch {
            b
        } else {
            plan.compress_batch.unwrap() // plan is non-empty; at least one batch exists
        };

        let all_msgs = db.load_messages(None, None, false)?;
        if all_msgs.is_empty() {
            return anyhow::Ok(None);
        }

        let end_idx = batch.end_idx.min(all_msgs.len() - 1);
        if batch.start_idx > end_idx {
            return anyhow::Ok(None);
        }

        let span = &all_msgs[batch.start_idx..=end_idx];

        // Token count for the span
        let span_tokens: u64 = span.iter().map(|m| estimate_tokens(&m.content)).sum();
        if span_tokens < cfg1.min_span_tokens {
            return anyhow::Ok(None); // too small to be worth the API call
        }

        // Conservative summary size estimate: 10% of source, clamped 100–500 tokens
        let est_summary_tokens = (span_tokens / 10).clamp(100, 500);
        if !roi_gate(span_tokens, est_summary_tokens, cfg1.compression_api_cost) {
            return anyhow::Ok(None); // negative ROI — skip
        }

        anyhow::Ok(Some(SpanData {
            span_start_id: span.first().unwrap().id,
            span_end_id: span.last().unwrap().id,
            turns: span
                .iter()
                .map(|m| (m.role.clone(), m.content.clone()))
                .collect(),
        }))
    })
    .await??;

    let Some(span) = span_data else {
        return Ok(());
    };

    // ── Phase 2: AI summarization (async) ─────────────────────────────────────

    let conversation_text = span
        .turns
        .iter()
        .map(|(role, content)| format!("{role}: {content}"))
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");

    let messages = vec![Message {
        role: Role::User,
        content: format!(
            "Summarize this conversation excerpt for long-term context compression. \
             Be dense and factual. Preserve: key decisions, file paths, function names, \
             specific values, errors and how they were resolved. \
             Omit pleasantries and repetition. 200-400 words.\n\n\
             CONVERSATION:\n{conversation_text}"
        ),
    }];

    let response = provider.send_message(&messages, compress_config).await?;
    let summary = response.content.trim().to_string();
    if summary.is_empty() {
        return Ok(());
    }

    // ── Phase 3: write results back to the DB (blocking) ─────────────────────

    let turns_compressed = span.turns.len();
    let span_start_id = span.span_start_id;
    let span_end_id = span.span_end_id;
    let db_path3 = db_path.clone();
    let summary3 = summary.clone();

    tokio::task::spawn_blocking(move || {
        let db = SessionDb::open(&db_path3)?;
        db.save_evergreen_chunk(span_start_id, span_end_id, &summary3, None)?;
        db.mark_messages_compressed(span_start_id, span_end_id)?;
        anyhow::Ok(())
    })
    .await??;

    // ── Phase 4: notify the event bus ────────────────────────────────────────

    let _ = event_tx.send(Event::EvergreenCompressed {
        session_id: session_id.to_string(),
        turns_compressed,
    });

    tracing::info!(
        "evergreen: compressed {turns_compressed} turns \
         [msg_id {span_start_id}..{span_end_id}] for session {session_id}"
    );

    Ok(())
}
