//! MCP transport layer: stdio subprocess communication.
//!
//! Spawns an MCP server as a child process and communicates via
//! JSON-RPC 2.0 over stdin/stdout (newline-delimited JSON).

use std::collections::HashMap;
use std::process::Stdio;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};

use super::jsonrpc::{Notification, Request, Response};

/// A running stdio MCP server process.
#[allow(dead_code)] // `pending` is used by spawned reader/writer tasks via Arc clones
pub struct StdioTransport {
    /// Channel to send requests to the writer task.
    request_tx: mpsc::Sender<(String, Option<oneshot::Sender<Response>>)>,
    /// Pending response receivers keyed by request ID.
    pending: std::sync::Arc<tokio::sync::Mutex<HashMap<u64, oneshot::Sender<Response>>>>,
    /// Handle to the child process for cleanup.
    child: std::sync::Arc<tokio::sync::Mutex<Child>>,
}

impl StdioTransport {
    /// Spawn an MCP server subprocess and set up communication channels.
    pub async fn spawn(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null()) // Suppress stderr to avoid noise
            .kill_on_drop(true);

        // Merge env vars (inherit current + add configured)
        for (key, val) in env {
            cmd.env(key, val);
        }

        let mut child = cmd.spawn().with_context(|| {
            format!("Failed to spawn MCP server: {} {}", command, args.join(" "))
        })?;

        let stdin = child.stdin.take().context("Failed to capture stdin")?;
        let stdout = child.stdout.take().context("Failed to capture stdout")?;

        let pending: std::sync::Arc<tokio::sync::Mutex<HashMap<u64, oneshot::Sender<Response>>>> =
            std::sync::Arc::new(tokio::sync::Mutex::new(HashMap::new()));

        // Writer task: sends messages to stdin
        let (request_tx, mut request_rx) =
            mpsc::channel::<(String, Option<oneshot::Sender<Response>>)>(64);

        let pending_for_writer = pending.clone();
        tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some((msg, response_tx)) = request_rx.recv().await {
                // If this is a request (not notification), extract ID and register pending
                if let Some(tx) = response_tx
                    && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&msg)
                    && let Some(id) = parsed["id"].as_u64()
                {
                    pending_for_writer.lock().await.insert(id, tx);
                }

                let line = format!("{}\n", msg);
                if stdin.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                if stdin.flush().await.is_err() {
                    break;
                }
            }
        });

        // Reader task: reads responses from stdout and dispatches to pending
        let pending_for_reader = pending.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }

                match serde_json::from_str::<Response>(&line) {
                    Ok(response) => {
                        if let Some(id) = response.id {
                            let mut pending = pending_for_reader.lock().await;
                            if let Some(tx) = pending.remove(&id) {
                                let _ = tx.send(response);
                            }
                        }
                        // Notifications (no id) are silently consumed for now
                    }
                    Err(e) => {
                        tracing::debug!("MCP: ignoring non-JSON-RPC line: {e}");
                    }
                }
            }
        });

        Ok(Self {
            request_tx,
            pending,
            child: std::sync::Arc::new(tokio::sync::Mutex::new(child)),
        })
    }

    /// Send a JSON-RPC request and wait for the response.
    pub async fn request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let req = Request::new(method, params);
        let id = req.id;
        let msg = serde_json::to_string(&req)?;

        let (response_tx, response_rx) = oneshot::channel();

        self.request_tx
            .send((msg, Some(response_tx)))
            .await
            .context("MCP transport channel closed")?;

        let response = tokio::time::timeout(std::time::Duration::from_secs(30), response_rx)
            .await
            .context("MCP request timed out (30s)")?
            .context("MCP response channel dropped")?;

        if let Some(error) = response.error {
            anyhow::bail!("MCP server error: {error}");
        }

        response.result.context(format!(
            "MCP response for '{}' (id={}) had no result or error",
            method, id
        ))
    }

    /// Send a JSON-RPC notification (no response expected).
    pub async fn notify(&self, method: &str, params: Option<serde_json::Value>) -> Result<()> {
        let notif = Notification::new(method, params);
        let msg = serde_json::to_string(&notif)?;

        self.request_tx
            .send((msg, None))
            .await
            .context("MCP transport channel closed")?;

        Ok(())
    }

    /// Shut down the MCP server process.
    pub async fn shutdown(&self) -> Result<()> {
        // Try graceful shutdown first
        let _ = self.notify("notifications/cancelled", None).await;

        // Kill the process
        let mut child = self.child.lock().await;
        let _ = child.kill().await;

        Ok(())
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        // Best-effort cleanup — child has kill_on_drop(true) anyway
    }
}
