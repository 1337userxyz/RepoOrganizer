//! Domänenmodell: Repo, Location, Snapshot, Activity.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repo {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub remote_url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub favorite: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub clean_keep_binary: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    #[default]
    Primary,
    Backup,
    Mirror,
    Worktree,
}

/// Pfade je Betriebssystem oder generischer Pfad.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct OsPaths {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linux: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub macos: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub id: String,
    pub repo_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(default)]
    pub paths: OsPaths,
    #[serde(default)]
    pub role: Role,
    #[serde(default, skip_serializing_if = "is_false")]
    pub auto_fetch: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Unavailable,
    NotARepo,
    UpToDate,
    Behind(u32),
    Ahead(u32),
    Diverged,
    Dirty,
    Error(String),
}

/// Lokaler Cache (pro Rechner).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub location_id: String,
    pub last_checked: Option<i64>,
    pub last_fetch: Option<i64>,
    pub last_pull: Option<i64>,
    #[serde(default)]
    pub last_commit: Option<i64>,
    #[serde(default)]
    pub last_commit_hash: Option<String>,
    #[serde(default)]
    pub last_commit_author: Option<String>,
    #[serde(default)]
    pub last_commit_msg: Option<String>,
    #[serde(default)]
    pub commit_count: u32,
    #[serde(default)]
    pub file_count: u32,
    #[serde(default)]
    pub size_kb: u64,
    #[serde(default)]
    pub build_size_bytes: u64,
    #[serde(default)]
    pub remote_url: Option<String>,
    pub branch: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub dirty_files: u32,
    pub untracked: u32,
    pub stash_count: u32,
    pub head_sha: Option<String>,
    pub state: Status,
    #[serde(default)]
    pub dirty_details: Vec<DirtyFile>,
}

/// Einzelne uncommittete Datei mit Pfad, Status und letzter Änderungszeit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirtyFile {
    pub path: String,
    pub status: String,
    pub status_code: String,
    pub kind: i32,
    pub mtime: Option<i64>,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activity {
    pub repo_id: String,
    pub date: String,
    pub commits: u32,
    pub authors: BTreeMap<String, u32>,
    #[serde(default)]
    pub added: u64,
    #[serde(default)]
    pub deleted: u64,
}

impl Snapshot {
    /// Leerer Snapshot mit gegebenem Zustand (z. B. Unavailable bei Timeout).
    pub fn empty(location_id: &str, state: Status) -> Self {
        Snapshot {
            location_id: location_id.into(),
            last_checked: None,
            last_fetch: None,
            last_pull: None,
            last_commit: None,
            last_commit_hash: None,
            last_commit_author: None,
            last_commit_msg: None,
            commit_count: 0,
            file_count: 0,
            size_kb: 0,
            build_size_bytes: 0,
            remote_url: None,
            branch: None,
            ahead: 0,
            behind: 0,
            dirty_files: 0,
            untracked: 0,
            stash_count: 0,
            head_sha: None,
            state,
            dirty_details: Vec::new(),
        }
    }
}
