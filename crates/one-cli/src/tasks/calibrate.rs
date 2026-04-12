//! Calibrate — skill improvement from detected preference corrections.
//!
//! Every 5 user messages per session, Calibrate:
//! 1. Scans the delta messages since last analysis for correction signals.
//! 2. Calls the cheapest model with a structured extraction prompt.
//! 3. Parses `<updates>[{"skill","section","change","reason"}]</updates>` output.
//! 4. Rewrites the affected project-level skill `.md` files in-place.
//!
//! Only modifies skills in the active project's `.one/commands/` or
//! `.claude/commands/` directories — never global or git-root skills.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use one_core::event::Event;
use one_core::provider::{AiProvider, Message, ModelConfig, Role};
use one_core::state::SharedState;
use tokio::sync::broadcast;

const TURN_BATCH_SIZE: usize = 5;

#[derive(serde::Deserialize, Debug)]
struct SkillUpdate {
    skill: String,
    section: String,
    change: String,
    reason: String,
}

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
    // Per-session: (user_message_count_since_last_check, last_analyzed_turn_index)
    let mut trackers: HashMap<String, (usize, usize)> = HashMap::new();

    // Use cheapest model — Calibrate doesn't need depth, just pattern recognition.
    let calibrate_config = ModelConfig {
        max_tokens: 512,
        temperature: Some(0.1),
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
            Event::UserMessage { ref session_id, .. } => {
                let entry = trackers.entry(session_id.clone()).or_insert((0, 0));
                entry.0 += 1;

                if entry.0 < TURN_BATCH_SIZE {
                    continue;
                }

                let (enabled, project_path, turns_snapshot, last_idx) = {
                    let s = state.read().await;
                    let enabled = s.calibrate_enabled;
                    let session = s.sessions.get(session_id);
                    let path = session.map(|s| s.project_path.clone()).unwrap_or_default();
                    let turns: Vec<(String, String)> = session
                        .map(|s| {
                            s.conversation
                                .turns
                                .iter()
                                .map(|t| (format!("{:?}", t.role), t.content.clone()))
                                .collect()
                        })
                        .unwrap_or_default();
                    (enabled, path, turns, entry.1)
                };

                if !enabled || project_path.is_empty() {
                    trackers.insert(session_id.clone(), (0, last_idx));
                    continue;
                }

                // Locate project-level skills.
                let skill_dirs = project_skill_dirs(&project_path);
                if skill_dirs.is_empty() {
                    trackers.insert(session_id.clone(), (0, turns_snapshot.len()));
                    continue;
                }

                // Delta: only messages since last check.
                let delta: Vec<_> = turns_snapshot[last_idx..].to_vec();
                if delta.is_empty() {
                    trackers.insert(session_id.clone(), (0, turns_snapshot.len()));
                    continue;
                }

                let new_last_idx = turns_snapshot.len();
                trackers.insert(session_id.clone(), (0, new_last_idx));

                // Quick symbolic pre-check: any correction signals in the delta?
                let combined_text = delta
                    .iter()
                    .map(|(_, c)| c.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
                    .to_lowercase();

                let correction_signals = [
                    "no actually",
                    "don't do that",
                    "instead of",
                    "i prefer",
                    "always use",
                    "never use",
                    "stop doing",
                    "please don't",
                    "not like that",
                    "wrong approach",
                    "that's wrong",
                    "incorrect",
                ];
                let has_signal = correction_signals.iter().any(|s| combined_text.contains(s));

                if !has_signal {
                    continue; // Fast path: no corrections detected, skip model call
                }

                let _ = event_tx.send(Event::DebugLog {
                    session_id: session_id.clone(),
                    message: "calibrate → correction signals detected, analysing".to_string(),
                });

                // Format delta for model.
                let delta_text = delta
                    .iter()
                    .map(|(role, content)| format!("{role}: {content}"))
                    .collect::<Vec<_>>()
                    .join("\n\n");

                let available_skills = list_project_skills(&skill_dirs);
                let skills_list = available_skills.join(", ");

                let prompt = format!(
                    "Analyse this conversation delta for user preference corrections that should \
                     update skill definitions.\n\n\
                     Available project skills: {skills_list}\n\n\
                     Look for: explicit corrections (\"no, instead...\"), stated preferences \
                     (\"I always want...\"), and style feedback (\"don't use...\").\n\n\
                     If corrections found, output EXACTLY:\n\
                     <updates>[{{\"skill\":\"name\",\"section\":\"section heading\",\
                     \"change\":\"what to change\",\"reason\":\"why\"}}]</updates>\n\n\
                     If no actionable corrections, output: <updates>[]</updates>\n\n\
                     CONVERSATION DELTA:\n{delta_text}"
                );

                let messages = vec![Message {
                    role: Role::User,
                    content: prompt,
                }];

                let response = match provider.send_message(&messages, &calibrate_config).await {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!("calibrate: model call failed: {e}");
                        continue;
                    }
                };

                if let Some(updates) = extract_updates(&response.content) {
                    if updates.is_empty() {
                        let _ = event_tx.send(Event::DebugLog {
                            session_id: session_id.clone(),
                            message: "calibrate: no actionable updates found".to_string(),
                        });
                        continue;
                    }

                    let mut applied = 0usize;
                    for update in &updates {
                        if apply_skill_update(update, &skill_dirs) {
                            applied += 1;
                        }
                    }

                    let _ = event_tx.send(Event::DebugLog {
                        session_id: session_id.clone(),
                        message: format!("calibrate ← applied {} skill update(s)", applied),
                    });
                }
            }
            Event::Quit => break,
            _ => {}
        }
    }
}

// ── Update application ────────────────────────────────────────────────────────

fn extract_updates(text: &str) -> Option<Vec<SkillUpdate>> {
    let start = text.find("<updates>")?;
    let end = text.find("</updates>")?;
    let json = &text[start + 9..end].trim();
    serde_json::from_str(json).ok()
}

fn apply_skill_update(update: &SkillUpdate, skill_dirs: &[PathBuf]) -> bool {
    for dir in skill_dirs {
        let skill_file = dir.join(format!("{}.md", update.skill));
        if !skill_file.exists() {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&skill_file) else {
            continue;
        };

        // Find section and append the change as a new bullet.
        let section_header = format!("## {}", update.section);
        let new_bullet = format!("- {} (calibrated: {})", update.change, update.reason);

        let new_content = if content.contains(&section_header) {
            // Insert bullet after the section header.
            content.replacen(
                &section_header,
                &format!("{}\n{}", section_header, new_bullet),
                1,
            )
        } else {
            // Append as a new section.
            format!(
                "{}\n\n{}\n{}",
                content.trim_end(),
                section_header,
                new_bullet
            )
        };

        if std::fs::write(&skill_file, new_content).is_ok() {
            tracing::info!(
                "calibrate: updated skill '{}' section '{}'",
                update.skill,
                update.section
            );
            return true;
        }
    }
    false
}

// ── Skill discovery ───────────────────────────────────────────────────────────

fn project_skill_dirs(project_path: &str) -> Vec<PathBuf> {
    let project = PathBuf::from(project_path);
    [".one/commands", ".claude/commands"]
        .iter()
        .map(|sub| project.join(sub))
        .filter(|p| p.is_dir())
        .collect()
}

fn list_project_skills(dirs: &[PathBuf]) -> Vec<String> {
    dirs.iter()
        .flat_map(|d| {
            std::fs::read_dir(d)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("md"))
                .filter_map(|e| {
                    e.path()
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s.to_string())
                })
        })
        .collect()
}
