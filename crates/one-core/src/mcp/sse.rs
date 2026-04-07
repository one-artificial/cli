//! MCP SSE transport — connects to remote MCP servers via Server-Sent Events.
//!
//! Protocol:
//! 1. Client sends GET to SSE endpoint → receives event stream
//! 2. Server sends an `endpoint` event with the POST URL for messages
//! 3. Client sends JSON-RPC requests via POST to that endpoint
//! 4. Server sends responses as SSE `message` events
//!
//! This enables connecting to hosted MCP servers (Notion, Playwright, etc.)

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::{Mutex, oneshot};

use super::jsonrpc;

/// SSE transport for remote MCP servers.
pub struct SseTransport {
    /// The POST endpoint for sending messages (discovered from SSE stream).
    message_endpoint: Arc<Mutex<Option<String>>>,
    /// HTTP client for sending requests.
    client: reqwest::Client,
    /// Custom headers (e.g., Authorization).
    headers: HashMap<String, String>,
    /// Pending response channels keyed by request ID.
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
    /// Handle to the SSE listener task.
    _listener_handle: tokio::task::JoinHandle<()>,
}

impl SseTransport {
    /// Connect to a remote MCP server via SSE.
    pub async fn connect(sse_url: &str, headers: &HashMap<String, String>) -> Result<Self> {
        let client = reqwest::Client::new();
        let message_endpoint = Arc::new(Mutex::new(None::<String>));
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Build SSE request with custom headers
        let mut req = client.get(sse_url);
        for (k, v) in headers {
            req = req.header(k.as_str(), v.as_str());
        }
        req = req.header("Accept", "text/event-stream");

        let response = req
            .send()
            .await
            .context("Failed to connect to SSE endpoint")?;

        if !response.status().is_success() {
            anyhow::bail!(
                "SSE connection failed: {} {}",
                response.status(),
                response.text().await.unwrap_or_default()
            );
        }

        // Spawn SSE listener
        let ep_clone = message_endpoint.clone();
        let pending_clone = pending.clone();
        let sse_base = sse_url.to_string();

        let listener_handle = tokio::spawn(async move {
            let mut stream = response.bytes_stream();
            let mut buffer = String::new();

            use futures_util::StreamExt;

            while let Some(chunk_result) = stream.next().await {
                let Ok(chunk) = chunk_result else { break };
                buffer.push_str(&String::from_utf8_lossy(&chunk));

                // Parse SSE events from buffer
                while let Some(boundary) = buffer.find("\n\n") {
                    let event_text = buffer[..boundary].to_string();
                    buffer = buffer[boundary + 2..].to_string();

                    let mut event_type = String::new();
                    let mut event_data = String::new();

                    for line in event_text.lines() {
                        if let Some(t) = line.strip_prefix("event: ") {
                            event_type = t.trim().to_string();
                        } else if let Some(d) = line.strip_prefix("data: ") {
                            if !event_data.is_empty() {
                                event_data.push('\n');
                            }
                            event_data.push_str(d);
                        }
                    }

                    match event_type.as_str() {
                        "endpoint" => {
                            // Server tells us where to POST messages
                            let endpoint = if event_data.starts_with("http") {
                                event_data.clone()
                            } else {
                                // Relative URL — resolve against SSE base
                                let base = url::Url::parse(&sse_base).ok();
                                base.and_then(|b| b.join(&event_data).ok())
                                    .map(|u| u.to_string())
                                    .unwrap_or(event_data.clone())
                            };
                            *ep_clone.lock().await = Some(endpoint);
                            tracing::info!("MCP SSE: message endpoint discovered");
                        }
                        "message" => {
                            // JSON-RPC response
                            if let Ok(response) =
                                serde_json::from_str::<serde_json::Value>(&event_data)
                                && let Some(id) = response["id"].as_u64()
                            {
                                let mut pending = pending_clone.lock().await;
                                if let Some(sender) = pending.remove(&id) {
                                    let _ = sender.send(response["result"].clone());
                                }
                            }
                        }
                        _ => {} // Ignore unknown events
                    }
                }
            }
        });

        // Wait briefly for the endpoint event
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        Ok(Self {
            message_endpoint,
            client,
            headers: headers.clone(),
            pending,
            _listener_handle: listener_handle,
        })
    }

    /// Send a JSON-RPC request and wait for the response.
    pub async fn request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let endpoint = {
            let ep = self.message_endpoint.lock().await;
            ep.clone()
                .context("No message endpoint discovered from SSE stream")?
        };

        let request = jsonrpc::Request::new(method, params);
        let id = request.id;

        // Register pending response
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id, tx);
        }

        // Send POST request
        let mut req = self.client.post(&endpoint).json(&request);
        for (k, v) in &self.headers {
            req = req.header(k.as_str(), v.as_str());
        }

        let response = req.send().await.context("Failed to send MCP request")?;
        if !response.status().is_success() {
            // Clean up pending
            self.pending.lock().await.remove(&id);
            anyhow::bail!("MCP POST failed: {}", response.status());
        }

        // Wait for response via SSE with timeout
        match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(_)) => anyhow::bail!("MCP SSE response channel closed"),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                anyhow::bail!("MCP SSE response timed out after 30s")
            }
        }
    }

    /// Send a notification (no response expected).
    pub async fn notify(&self, method: &str, params: Option<serde_json::Value>) -> Result<()> {
        let endpoint = {
            let ep = self.message_endpoint.lock().await;
            ep.clone().context("No message endpoint discovered")?
        };

        let notification = jsonrpc::Notification::new(method, params);

        let mut req = self.client.post(&endpoint).json(&notification);
        for (k, v) in &self.headers {
            req = req.header(k.as_str(), v.as_str());
        }

        req.send().await?;
        Ok(())
    }
}
