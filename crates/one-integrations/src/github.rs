use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use tokio::sync::broadcast;

use one_core::event::{Event, Notification, NotificationSource};

use crate::Integration;

const GITHUB_API: &str = "https://api.github.com";
const POLL_INTERVAL_SECS: u64 = 60;

pub struct GitHubIntegration {
    token: String,
    repos: Vec<String>,
    running: Arc<AtomicBool>,
}

impl GitHubIntegration {
    pub fn new(token: String, repos: Vec<String>) -> Self {
        Self {
            token,
            repos,
            running: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Integration for GitHubIntegration {
    fn name(&self) -> &str {
        "github"
    }

    async fn start(&mut self, event_tx: broadcast::Sender<Event>) -> Result<()> {
        if self.token.is_empty() {
            tracing::warn!("GitHub integration: no token configured, skipping");
            return Ok(());
        }

        self.running.store(true, Ordering::SeqCst);
        let token = self.token.clone();
        let repos = self.repos.clone();
        let running = self.running.clone();

        tokio::spawn(async move {
            tracing::info!("GitHub integration started for {} repos", repos.len());

            let client = reqwest::Client::builder()
                .user_agent("one-cli/0.1.0")
                .build()
                .unwrap();

            let mut last_modified: Option<String> = None;

            while running.load(Ordering::SeqCst) {
                match poll_notifications(&client, &token, &last_modified).await {
                    Ok(PollResult::New {
                        notifications,
                        modified,
                    }) => {
                        last_modified = modified;
                        let count = notifications.len();
                        for notif in notifications {
                            let _ = event_tx.send(Event::Notification(notif));
                        }
                        if count > 0 {
                            let _ = event_tx.send(Event::DebugLog {
                                session_id: String::new(),
                                message: format!("github: {count} new notification(s)"),
                            });
                        }
                    }
                    Ok(PollResult::NotModified) => {
                        // Nothing new — this is the expected common case
                    }
                    Err(e) => {
                        tracing::error!("GitHub poll error: {e}");
                    }
                }

                // Also poll for PRs on configured repos
                for repo in &repos {
                    match poll_repo_prs(&client, &token, repo).await {
                        Ok(notifs) => {
                            for notif in notifs {
                                let _ = event_tx.send(Event::Notification(notif));
                            }
                        }
                        Err(e) => {
                            tracing::error!("GitHub PR poll error for {repo}: {e}");
                        }
                    }
                }

                tokio::time::sleep(std::time::Duration::from_secs(POLL_INTERVAL_SECS)).await;
            }

            tracing::info!("GitHub integration stopped");
        });

        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }
}

enum PollResult {
    New {
        notifications: Vec<Notification>,
        modified: Option<String>,
    },
    NotModified,
}

/// Poll GitHub's notifications API. Uses If-Modified-Since to avoid
/// rate limit waste — GitHub returns 304 when nothing changed.
async fn poll_notifications(
    client: &reqwest::Client,
    token: &str,
    last_modified: &Option<String>,
) -> Result<PollResult> {
    let mut req = client
        .get(format!("{GITHUB_API}/notifications"))
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json");

    if let Some(lm) = last_modified {
        req = req.header("If-Modified-Since", lm);
    }

    let resp = req.send().await?;

    if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(PollResult::NotModified);
    }

    if !resp.status().is_success() {
        anyhow::bail!("GitHub API error: {}", resp.status());
    }

    let modified = resp
        .headers()
        .get("Last-Modified")
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    let data: Vec<serde_json::Value> = resp.json().await?;

    let notifications: Vec<Notification> = data
        .into_iter()
        .filter_map(|item| {
            let subject = item["subject"]["title"].as_str()?;
            let reason = item["reason"].as_str().unwrap_or("unknown");
            let repo = item["repository"]["full_name"].as_str().unwrap_or("?");
            let url = item["subject"]["url"].as_str().map(|u| {
                // Convert API URL to web URL
                u.replace("api.github.com/repos", "github.com")
                    .replace("/pulls/", "/pull/")
            });
            let updated = item["updated_at"]
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(chrono::Utc::now);

            let title = format!("[{repo}] {subject}");
            let body = format!("Reason: {reason}");

            Some(Notification {
                source: NotificationSource::GitHub,
                title,
                body,
                url,
                timestamp: updated,
            })
        })
        .collect();

    Ok(PollResult::New {
        notifications,
        modified,
    })
}

/// Poll open PRs for a specific repo to surface new ones.
async fn poll_repo_prs(
    client: &reqwest::Client,
    token: &str,
    repo: &str,
) -> Result<Vec<Notification>> {
    let resp = client
        .get(format!("{GITHUB_API}/repos/{repo}/pulls"))
        .query(&[("state", "open"), ("sort", "updated"), ("per_page", "5")])
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?;

    if !resp.status().is_success() {
        return Ok(Vec::new());
    }

    let data: Vec<serde_json::Value> = resp.json().await?;

    let notifs: Vec<Notification> = data
        .into_iter()
        .filter_map(|pr| {
            let title = pr["title"].as_str()?;
            let number = pr["number"].as_u64()?;
            let user = pr["user"]["login"].as_str().unwrap_or("?");
            let url = pr["html_url"].as_str().map(String::from);
            let updated = pr["updated_at"]
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(chrono::Utc::now);

            Some(Notification {
                source: NotificationSource::GitHub,
                title: format!("[{repo}] #{number}: {title}"),
                body: format!("by {user}"),
                url,
                timestamp: updated,
            })
        })
        .collect();

    Ok(notifs)
}
