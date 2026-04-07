//! Task management system for tracking multi-step work.
//!
//! Tasks are session-scoped and stored in memory (not persisted to disk).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single task with status tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub description: String,
    pub status: TaskStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskStatus::Pending => write!(f, "pending"),
            TaskStatus::InProgress => write!(f, "in_progress"),
            TaskStatus::Completed => write!(f, "completed"),
            TaskStatus::Cancelled => write!(f, "cancelled"),
        }
    }
}

/// Session-scoped task manager.
#[derive(Debug, Clone, Default)]
pub struct TaskManager {
    tasks: Vec<Task>,
    next_id: u32,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
            next_id: 1,
        }
    }

    /// Create a new task and return its ID.
    pub fn create(&mut self, description: &str) -> String {
        let id = format!("task_{}", self.next_id);
        self.next_id += 1;

        self.tasks.push(Task {
            id: id.clone(),
            description: description.to_string(),
            status: TaskStatus::Pending,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        });

        id
    }

    /// Update a task's status. Returns true if found.
    pub fn update_status(&mut self, id: &str, status: TaskStatus) -> bool {
        if let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) {
            task.status = status;
            task.updated_at = Utc::now();
            true
        } else {
            false
        }
    }

    /// Get a task by ID.
    pub fn get(&self, id: &str) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }

    /// List all tasks.
    pub fn list(&self) -> &[Task] {
        &self.tasks
    }

    /// List tasks by status.
    pub fn list_by_status(&self, status: TaskStatus) -> Vec<&Task> {
        self.tasks.iter().filter(|t| t.status == status).collect()
    }

    /// Get a summary string for display.
    pub fn summary(&self) -> String {
        if self.tasks.is_empty() {
            return "No tasks.".to_string();
        }

        let pending = self
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Pending)
            .count();
        let in_progress = self
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::InProgress)
            .count();
        let completed = self
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Completed)
            .count();
        let total = self.tasks.len();

        let mut lines = vec![format!(
            "{total} tasks ({pending} pending, {in_progress} in progress, {completed} done)"
        )];

        for task in &self.tasks {
            let marker = match task.status {
                TaskStatus::Pending => "[ ]",
                TaskStatus::InProgress => "[~]",
                TaskStatus::Completed => "[x]",
                TaskStatus::Cancelled => "[-]",
            };
            lines.push(format!("  {} {} — {}", marker, task.id, task.description));
        }

        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_task() {
        let mut mgr = TaskManager::new();
        let id = mgr.create("Fix the bug");
        assert_eq!(id, "task_1");
        assert_eq!(mgr.list().len(), 1);
        assert_eq!(mgr.list()[0].status, TaskStatus::Pending);
    }

    #[test]
    fn test_update_status() {
        let mut mgr = TaskManager::new();
        let id = mgr.create("Fix the bug");
        assert!(mgr.update_status(&id, TaskStatus::InProgress));
        assert_eq!(mgr.get(&id).unwrap().status, TaskStatus::InProgress);

        assert!(mgr.update_status(&id, TaskStatus::Completed));
        assert_eq!(mgr.get(&id).unwrap().status, TaskStatus::Completed);
    }

    #[test]
    fn test_update_nonexistent() {
        let mut mgr = TaskManager::new();
        assert!(!mgr.update_status("task_999", TaskStatus::Completed));
    }

    #[test]
    fn test_list_by_status() {
        let mut mgr = TaskManager::new();
        mgr.create("Task A");
        let id_b = mgr.create("Task B");
        mgr.create("Task C");
        mgr.update_status(&id_b, TaskStatus::Completed);

        assert_eq!(mgr.list_by_status(TaskStatus::Pending).len(), 2);
        assert_eq!(mgr.list_by_status(TaskStatus::Completed).len(), 1);
    }

    #[test]
    fn test_summary() {
        let mut mgr = TaskManager::new();
        mgr.create("First task");
        let id = mgr.create("Second task");
        mgr.update_status(&id, TaskStatus::InProgress);

        let summary = mgr.summary();
        assert!(summary.contains("2 tasks"));
        assert!(summary.contains("1 pending"));
        assert!(summary.contains("1 in progress"));
    }
}
