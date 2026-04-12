use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use tokio::sync::broadcast;

use one_core::event::{Event, Notification, NotificationSource};

use crate::Integration;

const NOTION_API: &str = "https://api.notion.com/v1";
const POLL_INTERVAL_SECS: u64 = 120;

pub struct NotionIntegration {
    token: String,
    running: Arc<AtomicBool>,
}

impl NotionIntegration {
    pub fn new(token: String) -> Self {
        Self {
            token,
            running: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Integration for NotionIntegration {
    fn name(&self) -> &str {
        "notion"
    }

    async fn start(&mut self, event_tx: broadcast::Sender<Event>) -> Result<()> {
        if self.token.is_empty() {
            tracing::warn!("Notion integration: no token configured, skipping");
            return Ok(());
        }

        self.running.store(true, Ordering::SeqCst);
        let token = self.token.clone();
        let running = self.running.clone();

        tokio::spawn(async move {
            tracing::info!("Notion integration started");

            let client = reqwest::Client::new();

            while running.load(Ordering::SeqCst) {
                match poll_recent_pages(&client, &token).await {
                    Ok(notifs) => {
                        let count = notifs.len();
                        for notif in notifs {
                            let _ = event_tx.send(Event::Notification(notif));
                        }
                        if count > 0 {
                            let _ = event_tx.send(Event::DebugLog {
                                session_id: String::new(),
                                message: format!("notion: {count} new page update(s)"),
                            });
                        }
                    }
                    Err(e) => {
                        tracing::error!("Notion poll error: {e}");
                    }
                }

                tokio::time::sleep(std::time::Duration::from_secs(POLL_INTERVAL_SECS)).await;
            }

            tracing::info!("Notion integration stopped");
        });

        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }
}

/// Search for recently edited pages via Notion's search API.
async fn poll_recent_pages(client: &reqwest::Client, token: &str) -> Result<Vec<Notification>> {
    let body = serde_json::json!({
        "sort": {
            "direction": "descending",
            "timestamp": "last_edited_time"
        },
        "page_size": 10
    });

    let resp = client
        .post(format!("{NOTION_API}/search"))
        .header("Authorization", format!("Bearer {token}"))
        .header("Notion-Version", "2022-06-28")
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("Notion API error: {}", resp.status());
    }

    let data: serde_json::Value = resp.json().await?;

    let results = data["results"]
        .as_array()
        .map(|arr| arr.to_vec())
        .unwrap_or_default();

    #[allow(clippy::unnecessary_filter_map)]
    let notifs: Vec<Notification> = results
        .into_iter()
        .filter_map(|page| {
            // Extract title from properties
            let title = extract_title(&page).unwrap_or_else(|| "Untitled".to_string());
            let url = page["url"].as_str().map(String::from);
            let edited_by = page["last_edited_by"]["id"].as_str().unwrap_or("someone");

            let last_edited = page["last_edited_time"]
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(chrono::Utc::now);

            Some(Notification {
                source: NotificationSource::Notion,
                title,
                body: format!("Edited by {edited_by}"),
                url,
                timestamp: last_edited,
            })
        })
        .collect();

    Ok(notifs)
}

/// Notion pages store titles in different property formats.
fn extract_title(page: &serde_json::Value) -> Option<String> {
    // Try properties.title (database pages)
    if let Some(props) = page["properties"].as_object() {
        for (_key, val) in props {
            if val["type"].as_str() == Some("title")
                && let Some(arr) = val["title"].as_array()
            {
                let text: String = arr
                    .iter()
                    .filter_map(|t| t["plain_text"].as_str())
                    .collect();
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
    }

    // Try child_page title
    page["child_page"]["title"].as_str().map(String::from)
}
