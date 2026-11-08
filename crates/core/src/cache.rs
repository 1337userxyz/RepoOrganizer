//! JSON-Cache für Snapshots (pro Rechner, nicht geteilt).
use crate::config::{write_atomic, ConfigError};
use crate::model::{Snapshot, Status};
use std::path::{Path, PathBuf};

pub fn default_path() -> Option<PathBuf> {
    kit_paths::appdirs::cache_dir("RepoOrganizer").map(|d| d.join("snapshots.json"))
}

/// Fehlende oder defekte Datei → leerer Cache.
pub fn load(path: &Path) -> Vec<Snapshot> {
    std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save(path: &Path, snaps: &[Snapshot]) -> Result<(), ConfigError> {
    let text = serde_json::to_string_pretty(snaps).unwrap_or_else(|_| "[]".into());
    write_atomic(path, &text)
}

/// Ist ein Standort jetzt nicht erreichbar, bleibt der letzte bekannte Stand (als Unavailable) erhalten.
pub fn merge(old: Option<&Snapshot>, new: Snapshot) -> Snapshot {
    match (&new.state, old) {
        (Status::Unavailable, Some(o)) if o.head_sha.is_some() => Snapshot { state: Status::Unavailable, ..o.clone() },
        _ => new,
    }
}

/// Gecachte Statistik je Repo.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RepoStats {
    pub fetched_at: i64,
    pub git_size: u64,
    pub activity: Vec<crate::model::Activity>,
}

pub fn activity_path() -> Option<PathBuf> {
    kit_paths::appdirs::cache_dir("RepoOrganizer").map(|d| d.join("activity.json"))
}

pub fn load_activity(path: &Path) -> std::collections::HashMap<String, RepoStats> {
    std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save_activity(path: &Path, stats: &std::collections::HashMap<String, RepoStats>) -> Result<(), ConfigError> {
    // Sortiert schreiben (stabile Datei).
    let sorted: std::collections::BTreeMap<_, _> = stats.iter().collect();
    write_atomic(path, &serde_json::to_string(&sorted).unwrap_or_else(|_| "{}".into()))
}
