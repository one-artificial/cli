//! Simple cron scheduler for session-scoped recurring tasks.
//!
//! Jobs are stored in memory and lost when the session ends.
//! Each job has a cron expression, a prompt to execute, and a unique ID.

use std::collections::HashMap;

/// A scheduled cron job.
#[derive(Debug, Clone)]
pub struct CronJob {
    pub id: String,
    pub cron_expression: String,
    pub prompt: String,
    pub recurring: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Session-scoped cron scheduler.
#[derive(Debug, Clone, Default)]
pub struct CronScheduler {
    jobs: HashMap<String, CronJob>,
    next_id: u32,
}

impl CronScheduler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new cron job. Returns the job ID.
    pub fn create(&mut self, cron_expression: &str, prompt: &str, recurring: bool) -> String {
        self.next_id += 1;
        let id = format!("{:08x}", fastrand_simple(self.next_id));

        let job = CronJob {
            id: id.clone(),
            cron_expression: cron_expression.to_string(),
            prompt: prompt.to_string(),
            recurring,
            created_at: chrono::Utc::now(),
        };

        self.jobs.insert(id.clone(), job);
        id
    }

    /// Delete a cron job by ID. Returns true if found.
    pub fn delete(&mut self, id: &str) -> bool {
        self.jobs.remove(id).is_some()
    }

    /// List all active jobs.
    pub fn list(&self) -> Vec<&CronJob> {
        let mut jobs: Vec<_> = self.jobs.values().collect();
        jobs.sort_by_key(|j| &j.created_at);
        jobs
    }

    /// Get a job by ID.
    pub fn get(&self, id: &str) -> Option<&CronJob> {
        self.jobs.get(id)
    }

    /// Format a human-readable list of all jobs.
    pub fn summary(&self) -> String {
        let jobs = self.list();
        if jobs.is_empty() {
            return "No scheduled jobs.".to_string();
        }

        let mut lines = vec![format!("{} scheduled job(s):", jobs.len())];
        for job in &jobs {
            let kind = if job.recurring {
                "recurring"
            } else {
                "one-shot"
            };
            let prompt_preview = if job.prompt.len() > 50 {
                format!("{}...", &job.prompt[..50])
            } else {
                job.prompt.clone()
            };
            lines.push(format!(
                "  {} [{kind}] {} — \"{prompt_preview}\"",
                job.id, job.cron_expression
            ));
        }
        lines.join("\n")
    }
}

/// Simple deterministic hash for generating IDs (avoids fastrand dependency).
fn fastrand_simple(seed: u32) -> u32 {
    let mut x = seed.wrapping_mul(2654435761);
    x ^= x >> 16;
    x = x.wrapping_mul(2246822519);
    x ^= x >> 13;
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_list() {
        let mut sched = CronScheduler::new();
        let id = sched.create("*/5 * * * *", "check status", true);

        assert!(!id.is_empty());
        assert_eq!(sched.list().len(), 1);
        assert_eq!(sched.list()[0].prompt, "check status");
    }

    #[test]
    fn test_delete() {
        let mut sched = CronScheduler::new();
        let id = sched.create("0 * * * *", "hourly", true);

        assert!(sched.delete(&id));
        assert!(sched.list().is_empty());
        assert!(!sched.delete("nonexistent"));
    }

    #[test]
    fn test_summary_empty() {
        let sched = CronScheduler::new();
        assert!(sched.summary().contains("No scheduled"));
    }

    #[test]
    fn test_summary_with_jobs() {
        let mut sched = CronScheduler::new();
        sched.create("*/5 * * * *", "check deploy", true);
        sched.create("0 9 * * *", "morning standup", false);

        let summary = sched.summary();
        assert!(summary.contains("2 scheduled"));
        assert!(summary.contains("check deploy"));
        assert!(summary.contains("recurring"));
        assert!(summary.contains("one-shot"));
    }
}
