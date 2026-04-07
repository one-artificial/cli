use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use tokio::sync::broadcast;

use one_core::event::{Event, Notification, NotificationSource};

use crate::Integration;

const ASANA_API: &str = "https://app.asana.com/api/1.0";
const POLL_INTERVAL_SECS: u64 = 120;

pub struct AsanaIntegration {
    token: String,
    workspace: Option<String>,
    running: Arc<AtomicBool>,
}

impl AsanaIntegration {
    pub fn new(token: String, workspace: Option<String>) -> Self {
        Self {
            token,
            workspace,
            running: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Integration for AsanaIntegration {
    fn name(&self) -> &str {
        "asana"
    }

    async fn start(&mut self, event_tx: broadcast::Sender<Event>) -> Result<()> {
        if self.token.is_empty() {
            tracing::warn!("Asana integration: no token configured, skipping");
            return Ok(());
        }

        self.running.store(true, Ordering::SeqCst);
        let token = self.token.clone();
        let workspace = self.workspace.clone();
        let running = self.running.clone();

        tokio::spawn(async move {
            tracing::info!("Asana integration started");

            let client = reqwest::Client::new();

            while running.load(Ordering::SeqCst) {
                match poll_tasks(&client, &token, &workspace).await {
                    Ok(notifs) => {
                        for notif in notifs {
                            let _ = event_tx.send(Event::Notification(notif));
                        }
                    }
                    Err(e) => {
                        tracing::error!("Asana poll error: {e}");
                    }
                }

                tokio::time::sleep(std::time::Duration::from_secs(POLL_INTERVAL_SECS)).await;
            }

            tracing::info!("Asana integration stopped");
        });

        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }
}

/// Polls for tasks assigned to the authenticated user.
async fn poll_tasks(
    client: &reqwest::Client,
    token: &str,
    workspace: &Option<String>,
) -> Result<Vec<Notification>> {
    // First get the current user
    let me_resp = client
        .get(format!("{ASANA_API}/users/me"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await?;

    let me: serde_json::Value = me_resp.json().await?;
    let user_gid = me["data"]["gid"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Could not get Asana user ID"))?;

    // Get workspace GID
    let ws_gid = if let Some(ws) = workspace {
        ws.clone()
    } else {
        me["data"]["workspaces"][0]["gid"]
            .as_str()
            .unwrap_or("")
            .to_string()
    };

    if ws_gid.is_empty() {
        return Ok(Vec::new());
    }

    // Get tasks assigned to me, modified recently
    let resp = client
        .get(format!("{ASANA_API}/tasks"))
        .query(&[
            ("assignee", user_gid),
            ("workspace", &ws_gid),
            (
                "opt_fields",
                "name,due_on,completed,modified_at,permalink_url",
            ),
            ("completed_since", "now"),
            ("limit", "20"),
        ])
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await?;

    let data: serde_json::Value = resp.json().await?;

    let tasks = data["data"]
        .as_array()
        .map(|arr| arr.to_vec())
        .unwrap_or_default();

    let notifs: Vec<Notification> = tasks
        .into_iter()
        .filter_map(|task| {
            let name = task["name"].as_str()?;
            let due = task["due_on"].as_str().unwrap_or("no due date");
            let url = task["permalink_url"].as_str().map(String::from);
            let modified = task["modified_at"]
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(chrono::Utc::now);

            Some(Notification {
                source: NotificationSource::Asana,
                title: name.to_string(),
                body: format!("Due: {due}"),
                url,
                timestamp: modified,
            })
        })
        .collect();

    Ok(notifs)
}
