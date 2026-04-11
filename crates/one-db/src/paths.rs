//! Filesystem path computation for the One storage layout.
//!
//! Layout:
//!   ~/.one/                                             ← profile dir
//!   ~/.one/{project_slug}/                              ← project dir
//!   ~/.one/{project_slug}/ONE.md                        ← project context
//!   ~/.one/{project_slug}/{branch}__{dt}____{hash}/     ← session dir
//!   ~/.one/{project_slug}/{branch}__{dt}____{hash}/session.db

use std::path::PathBuf;

use anyhow::Result;

/// All filesystem paths for one session.
#[derive(Clone)]
pub struct StoragePaths {
    /// `~/.one/`
    pub profile_dir: PathBuf,
    /// `~/.one/{project_slug}/`
    pub project_dir: PathBuf,
    /// `~/.one/{project_slug}/{session_folder}/`
    pub session_dir: PathBuf,
    /// `~/.one/{project_slug}/{session_folder}/session.db`
    pub session_db: PathBuf,
    /// `~/.one/{project_slug}/ONE.md`
    pub one_md: PathBuf,
    /// 6-char lowercase hex session identifier — printed on exit, used with `--session`
    pub session_hash: String,
    /// Original branch name (not slug)
    pub branch: String,
}

impl StoragePaths {
    /// Compute paths for a brand-new session opening right now.
    pub fn for_new_session(project_path: &str, branch: &str) -> Result<Self> {
        let profile_dir = profile_dir()?;
        let project_dir = profile_dir.join(slugify_path(project_path));
        let one_md = project_dir.join("ONE.md");

        let hash = generate_session_hash();
        let dt = chrono::Local::now()
            .format("%Y_%m_%d_%H_%M_%S")
            .to_string();
        let folder = session_folder_name(branch, &dt, &hash);
        let session_dir = project_dir.join(&folder);
        let session_db = session_dir.join("session.db");

        Ok(Self {
            profile_dir,
            project_dir,
            session_dir,
            session_db,
            one_md,
            session_hash: hash,
            branch: branch.to_string(),
        })
    }

    /// Resolve paths for an existing session by 6-char hash.
    /// Scans `~/.one/{project_slug}/` for a folder ending in `____{hash}`.
    pub fn for_existing_session(project_path: &str, hash: &str) -> Result<Option<Self>> {
        let profile_dir = profile_dir()?;
        let project_dir = profile_dir.join(slugify_path(project_path));

        if !project_dir.exists() {
            return Ok(None);
        }

        let suffix = format!("____{hash}");
        for entry in std::fs::read_dir(&project_dir)? {
            let entry = entry?;
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.ends_with(&suffix) {
                let session_dir = entry.path();
                let session_db = session_dir.join("session.db");
                let one_md = project_dir.join("ONE.md");
                let branch = branch_from_folder(&name_str);
                return Ok(Some(Self {
                    profile_dir,
                    project_dir,
                    session_dir,
                    session_db,
                    one_md,
                    session_hash: hash.to_string(),
                    branch,
                }));
            }
        }
        Ok(None)
    }

    /// List all sessions for a project + branch, newest first.
    pub fn list_sessions(project_path: &str, branch: &str) -> Result<Vec<SessionListing>> {
        let profile_dir = profile_dir()?;
        let project_dir = profile_dir.join(slugify_path(project_path));

        if !project_dir.exists() {
            return Ok(vec![]);
        }

        let prefix = format!("{}__{}", slugify_branch(branch), "");
        let mut listings: Vec<SessionListing> = Vec::new();

        for entry in std::fs::read_dir(&project_dir)? {
            let entry = entry?;
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            if name_str.starts_with(&prefix) {
                let session_dir = entry.path();
                let session_db = session_dir.join("session.db");
                if !session_db.exists() {
                    continue;
                }
                let hash = hash_from_folder(&name_str);
                let opened_at = datetime_from_folder(&name_str);
                listings.push(SessionListing {
                    session_dir,
                    session_db,
                    session_hash: hash,
                    opened_at,
                    branch: branch.to_string(),
                });
            }
        }

        // YYYY_MM_DD_HH_MM_SS sorts correctly as a plain string
        listings.sort_by(|a, b| b.opened_at.cmp(&a.opened_at));
        Ok(listings)
    }
}

/// A discovered session folder with enough info for a session picker UI.
#[derive(Clone)]
pub struct SessionListing {
    pub session_dir: PathBuf,
    pub session_db: PathBuf,
    /// 6-char hash
    pub session_hash: String,
    /// `YYYY_MM_DD_HH_MM_SS`
    pub opened_at: String,
    pub branch: String,
}

// ── Path helpers ──────────────────────────────────────────────────────────────

/// Returns `~/.one/`, creating it if absent.
pub fn profile_dir() -> Result<PathBuf> {
    let home = dirs_next::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    let dir = home.join(".one");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Slugify an absolute project path: strip leading `/`, replace `/` with `__`.
/// `/Users/luke/my-app` → `Users__luke__my-app`
pub fn slugify_path(path: &str) -> String {
    path.trim_start_matches('/').replace('/', "__")
}

/// Slugify a branch name: replace `/` → `__`, `-` and `.` → `_`, lowercase.
/// Then collapse any run of 3+ underscores to `__` so `____` stays unambiguous
/// as the field separator.
/// `feat/auth-work` → `feat__auth_work`
pub fn slugify_branch(branch: &str) -> String {
    let slug = branch
        .to_lowercase()
        .replace('/', "__")
        .replace(['-', '.'], "_");
    // Collapse 3+ consecutive underscores to 2, so `____` remains our unique separator
    collapse_underscores(&slug)
}

fn collapse_underscores(s: &str) -> String {
    // Runs of 1 or 2 underscores are kept verbatim (single `_` from `-`/`.`,
    // double `__` from `/`). Runs of 3+ collapse to 1, ensuring `____` can
    // never appear in a branch slug and always uniquely marks the hash separator.
    let mut out = String::with_capacity(s.len());
    let mut run = 0usize;
    for ch in s.chars() {
        if ch == '_' {
            run += 1;
        } else {
            if run > 0 {
                let keep = if run <= 2 { run } else { 1 };
                for _ in 0..keep {
                    out.push('_');
                }
                run = 0;
            }
            out.push(ch);
        }
    }
    if run > 0 {
        let keep = if run <= 2 { run } else { 1 };
        for _ in 0..keep {
            out.push('_');
        }
    }
    out
}

fn session_folder_name(branch: &str, dt: &str, hash: &str) -> String {
    format!("{}__{dt}____{hash}", slugify_branch(branch))
}

fn generate_session_hash() -> String {
    // First 6 hex chars of a UUID v4 — 16^6 = ~16M possibilities, negligible collision risk
    uuid::Uuid::new_v4()
        .to_string()
        .replace('-', "")
        .chars()
        .take(6)
        .collect()
}

// ── Folder name parsing ───────────────────────────────────────────────────────
// Format: {branch_slug}__{YYYY_MM_DD_HH_MM_SS}____{6-char-hash}
// `____` (4 underscores) is the sole separator between datetime and hash.
// Datetime is always exactly 19 chars.

fn hash_from_folder(name: &str) -> String {
    name.rfind("____")
        .map(|p| name[p + 4..].to_string())
        .unwrap_or_default()
}

fn datetime_from_folder(name: &str) -> String {
    name.rfind("____").map_or(String::new(), |p| {
        let before = &name[..p];
        if before.len() >= 19 {
            before[before.len() - 19..].to_string()
        } else {
            String::new()
        }
    })
}

/// Returns the branch slug (not the original branch name — that lives in session_meta).
fn branch_from_folder(name: &str) -> String {
    name.rfind("____").map_or_else(
        || name.to_string(),
        |p| {
            let before = &name[..p]; // ends with `__{datetime}`
            if before.len() > 21 {
                before[..before.len() - 21].to_string()
            } else {
                String::new()
            }
        },
    )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slugify_path() {
        assert_eq!(slugify_path("/Users/luke/my-app"), "Users__luke__my-app");
        assert_eq!(slugify_path("/a/b/c"), "a__b__c");
    }

    #[test]
    fn test_slugify_branch() {
        assert_eq!(slugify_branch("feat/auth-work"), "feat__auth_work");
        assert_eq!(slugify_branch("main"), "main");
        assert_eq!(slugify_branch("fix/v1.2-patch"), "fix__v1_2_patch");
        // Collapse excess underscores from pathological names
        assert_eq!(slugify_branch("a---b"), "a_b");
    }

    #[test]
    fn test_folder_round_trip() {
        let branch = "feat/auth-work-for-sam";
        let dt = "2026_04_11_10_46_11";
        let hash = "12bdq2";
        let folder = session_folder_name(branch, dt, hash);
        assert_eq!(folder, "feat__auth_work_for_sam__2026_04_11_10_46_11____12bdq2");
        assert_eq!(hash_from_folder(&folder), hash);
        assert_eq!(datetime_from_folder(&folder), dt);
        assert_eq!(branch_from_folder(&folder), "feat__auth_work_for_sam");
    }
}
