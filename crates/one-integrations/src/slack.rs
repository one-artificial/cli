use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use tokio::sync::broadcast;

use one_core::event::{Event, Notification, NotificationSource};

use crate::Integration;

const POLL_INTERVAL_SECS: u64 = 30;

pub struct SlackIntegration {
    token: String,
    channels: Vec<String>,
    running: Arc<AtomicBool>,
}

impl SlackIntegration {
    pub fn new(token: String, channels: Vec<String>) -> Self {
        Self {
            token,
            channels,
            running: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Integration for SlackIntegration {
    fn name(&self) -> &str {
        "slack"
    }

    async fn start(&mut self, event_tx: broadcast::Sender<Event>) -> Result<()> {
        if self.token.is_empty() {
            tracing::warn!("Slack integration: no token configured, skipping");
            return Ok(());
        }

        self.running.store(true, Ordering::SeqCst);
        let token = self.token.clone();
        let channels = self.channels.clone();
        let running = self.running.clone();

        tokio::spawn(async move {
            tracing::info!("Slack integration started for {} channels", channels.len());

            let client = reqwest::Client::new();
            let mut last_ts: Option<String> = None;

            while running.load(Ordering::SeqCst) {
                for channel in &channels {
                    match poll_channel(&client, &token, channel, &last_ts).await {
                        Ok(messages) => {
                            for (notif, ts) in messages {
                                // Track latest timestamp to avoid re-fetching
                                if last_ts.as_ref().is_none_or(|lt| ts > *lt) {
                                    last_ts = Some(ts);
                                }
                                let _ = event_tx.send(Event::Notification(notif));
                            }
                        }
                        Err(e) => {
                            tracing::error!("Slack poll error for {channel}: {e}");
                        }
                    }
                }

                tokio::time::sleep(std::time::Duration::from_secs(POLL_INTERVAL_SECS)).await;
            }

            tracing::info!("Slack integration stopped");
        });

        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }
}

async fn poll_channel(
    client: &reqwest::Client,
    token: &str,
    channel: &str,
    oldest: &Option<String>,
) -> Result<Vec<(Notification, String)>> {
    let mut params = vec![
        ("channel", channel.to_string()),
        ("limit", "10".to_string()),
    ];

    if let Some(ts) = oldest {
        params.push(("oldest", ts.clone()));
    }

    let resp = client
        .get("https://slack.com/api/conversations.history")
        .header("Authorization", format!("Bearer {token}"))
        .query(&params)
        .send()
        .await?;

    let data: serde_json::Value = resp.json().await?;

    if data["ok"].as_bool() != Some(true) {
        let err = data["error"].as_str().unwrap_or("unknown");
        anyhow::bail!("Slack API error: {err}");
    }

    let messages = data["messages"]
        .as_array()
        .map(|arr| arr.to_vec())
        .unwrap_or_default();

    let results: Vec<(Notification, String)> = messages
        .into_iter()
        .filter_map(|msg| {
            let text = msg["text"].as_str()?;
            let user = msg["user"].as_str().unwrap_or("unknown");
            let ts = msg["ts"].as_str()?.to_string();

            let timestamp = ts
                .split('.')
                .next()
                .and_then(|s| s.parse::<i64>().ok())
                .map(|secs| {
                    chrono::DateTime::from_timestamp(secs, 0).unwrap_or_else(chrono::Utc::now)
                })
                .unwrap_or_else(chrono::Utc::now);

            // Truncate long messages
            let body = if text.len() > 100 {
                format!("{}...", &text[..100])
            } else {
                text.to_string()
            };

            Some((
                Notification {
                    source: NotificationSource::Slack,
                    title: format!("#{channel} — {user}"),
                    body,
                    url: None,
                    timestamp,
                },
                ts,
            ))
        })
        .collect();

    Ok(results)
}
