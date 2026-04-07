//! Git worktree management for agent isolation.
//!
//! Creates temporary git worktrees so sub-agents can work on an isolated
//! copy of the repo without affecting the main working directory.

use anyhow::{Context, Result};

/// Create a temporary git worktree for an agent.
/// Returns (worktree_path, branch_name).
pub async fn create_agent_worktree(repo_dir: &str, agent_id: &str) -> Result<(String, String)> {
    let branch = format!("agent/{agent_id}");
    let worktree_dir = format!("/tmp/one-worktree-{agent_id}");

    // Create the worktree with a new branch
    let output = tokio::process::Command::new("git")
        .args(["worktree", "add", "-b", &branch, &worktree_dir])
        .current_dir(repo_dir)
        .output()
        .await
        .context("Failed to run git worktree add")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git worktree add failed: {stderr}");
    }

    Ok((worktree_dir, branch))
}

/// Check if a worktree has uncommitted changes.
pub async fn worktree_has_changes(worktree_dir: &str) -> bool {
    let output = tokio::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(worktree_dir)
        .output()
        .await;

    match output {
        Ok(o) => !String::from_utf8_lossy(&o.stdout).trim().is_empty(),
        Err(_) => false,
    }
}

/// Remove a worktree and optionally delete its branch.
pub async fn remove_agent_worktree(
    repo_dir: &str,
    worktree_dir: &str,
    branch: &str,
    delete_branch: bool,
) -> Result<()> {
    // Remove the worktree
    let output = tokio::process::Command::new("git")
        .args(["worktree", "remove", "--force", worktree_dir])
        .current_dir(repo_dir)
        .output()
        .await
        .context("Failed to run git worktree remove")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!("git worktree remove warning: {stderr}");
    }

    // Delete the branch if requested
    if delete_branch {
        let _ = tokio::process::Command::new("git")
            .args(["branch", "-D", branch])
            .current_dir(repo_dir)
            .output()
            .await;
    }

    // Clean up directory if it still exists
    let _ = tokio::fs::remove_dir_all(worktree_dir).await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_worktree_lifecycle() {
        // Only run if we're in a git repo
        let output = tokio::process::Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .output()
            .await;

        if output.map(|o| o.status.success()).unwrap_or(false) {
            let cwd = std::env::current_dir()
                .unwrap()
                .to_string_lossy()
                .to_string();
            let agent_id = format!("test_{}", std::process::id());

            // Create worktree
            let result = create_agent_worktree(&cwd, &agent_id).await;
            if let Ok((wt_dir, branch)) = result {
                // Check it exists
                assert!(std::path::Path::new(&wt_dir).exists());

                // No changes initially
                assert!(!worktree_has_changes(&wt_dir).await);

                // Clean up
                let _ = remove_agent_worktree(&cwd, &wt_dir, &branch, true).await;
                assert!(!std::path::Path::new(&wt_dir).exists());
            }
            // If create failed (e.g., no commits yet), that's fine for testing
        }
    }
}
