//! Health-Checks (rein auf gesammelten Fakten, einzeln abschaltbar).
use crate::level::Level;
use crate::text::ago;
use serde::{Deserialize, Serialize};

pub const ALL_CHECKS: &[(&str, &str)] = &[
    ("unpushed", "Ungepushte Commits"),
    ("no_upstream", "Branches ohne Upstream"),
    ("stale_fetch", "Lange kein Fetch"),
    ("stale_pull", "Lange kein Pull"),
    ("stale_commit", "Lange kein Commit"),
    ("old_dirty", "Alte uncommittete Änderungen"),
    ("large_files", "Große Dateien"),
    ("git_size", ".git-Größe"),
    ("build_artifacts", "Große Build-Artefakte"),
    ("no_gitignore", "Fehlende .gitignore"),
    ("no_readme", "Fehlende README"),
    ("detached", "Detached HEAD"),
    ("in_progress", "Laufender Merge/Rebase"),
    ("conflicts", "Konflikte"),
    ("remote", "Remote nicht erreichbar"),
    ("secrets", "Secret-Scan (gitleaks)"),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HealthSettings {
    pub disabled: Vec<String>,
    pub stale_fetch_days: u32,
    pub stale_pull_days: u32,
    pub stale_commit_days: u32,
    pub dirty_days: u32,
    pub large_file_mb: u64,
    pub git_size_mb: u64,
    pub build_artifacts_mb: u64,
}

impl Default for HealthSettings {
    fn default() -> Self {
        Self {
            disabled: vec![],
            stale_fetch_days: 14,
            stale_pull_days: 30,
            stale_commit_days: 90,
            dirty_days: 7,
            large_file_mb: 50,
            git_size_mb: 1000,
            build_artifacts_mb: 500,
        }
    }
}

impl HealthSettings {
    pub fn enabled(&self, check: &str) -> bool {
        !self.disabled.iter().any(|d| d == check)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub detached: bool,
    pub branch: Option<String>,
    pub ahead: u32,
    pub has_upstream: bool,
    pub branches_without_upstream: u32,
    pub has_remote: bool,
    pub last_fetch: Option<i64>,
    pub last_pull: Option<i64>,
    pub last_commit: Option<i64>,
    /// Älteste mtime einer uncommitteten Datei.
    pub oldest_dirty: Option<i64>,
    pub large_files: Vec<(String, u64)>,
    pub git_size_bytes: u64,
    pub build_size_bytes: u64,
    pub build_artifacts_count: u32,
    pub has_gitignore: bool,
    pub has_readme: bool,
    pub in_progress: Option<String>,
    pub conflicts: u32,
    pub remote_ok: Option<bool>,
    pub secrets: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub check: &'static str,
    pub level: Level,
    pub text: String,
    pub advice: String,
}

const DAY: i64 = 86400;

pub fn evaluate(f: &Facts, s: &HealthSettings, now: i64) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut add = |check: &'static str, level, text: String, advice: &str| {
        if s.enabled(check) {
            out.push(Finding { check, level, text, advice: advice.into() });
        }
    };
    if f.ahead > 0 {
        add("unpushed", Level::Warn, format!("{} ungepushte Commits", f.ahead), "Push ausführen.");
    }
    if f.branches_without_upstream > 0 || (!f.detached && f.branch.is_some() && !f.has_upstream && f.has_remote) {
        let n = f.branches_without_upstream.max(1);
        add("no_upstream", Level::Warn, format!("{n} Branch(es) ohne Upstream"), "Mit `git push -u origin <branch>` verknüpfen.");
    }
    if f.has_remote {
        match f.last_fetch {
            None => add("stale_fetch", Level::Warn, "Noch nie gefetcht".into(), "Fetch ausführen."),
            Some(t) if now - t > s.stale_fetch_days as i64 * DAY => {
                add("stale_fetch", Level::Warn, format!("Letzter Fetch {}", ago(Some(t), now)), "Fetch ausführen.")
            }
            _ => {}
        }
    }
    if let Some(t) = f.last_pull.filter(|t| now - t > s.stale_pull_days as i64 * DAY) {
        add("stale_pull", Level::Warn, format!("Letzter Pull {}", ago(Some(t), now)), "Pull ausführen.");
    }
    if let Some(t) = f.last_commit.filter(|t| now - t > s.stale_commit_days as i64 * DAY) {
        add("stale_commit", Level::Warn, format!("Letzter Commit {}", ago(Some(t), now)), "Repo noch aktiv? Ggf. archivieren.");
    }
    if let Some(t) = f.oldest_dirty.filter(|t| now - t > s.dirty_days as i64 * DAY) {
        add("old_dirty", Level::Warn, format!("Uncommittete Änderungen seit {}", ago(Some(t), now)), "Committen oder stashen.");
    }
    let limit = s.large_file_mb * 1024 * 1024;
    let big: Vec<_> = f.large_files.iter().filter(|(_, sz)| *sz > limit).collect();
    if let Some((name, sz)) = big.first() {
        add(
            "large_files",
            Level::Warn,
            format!("{} große Datei(en), z. B. {name} ({} MB)", big.len(), sz / 1024 / 1024),
            "Git LFS nutzen oder aus der Historie entfernen.",
        );
    }
    if f.git_size_bytes > s.git_size_mb * 1024 * 1024 {
        add("git_size", Level::Warn, format!(".git ist {} MB groß", f.git_size_bytes / 1024 / 1024), "`git gc --aggressive` oder Historie bereinigen.");
    }
    let build_limit = s.build_artifacts_mb * 1024 * 1024;
    if f.build_size_bytes > build_limit {
        add(
            "build_artifacts",
            Level::Warn,
            format!("Build-Artefakte belegen {}", crate::clean::format_bytes(f.build_size_bytes)),
            "Build-Cache bereinigen (Button 'Clean'), um RAM freizugeben.",
        );
    }
    if !f.has_gitignore {
        add("no_gitignore", Level::Warn, ".gitignore fehlt".into(), "Eine .gitignore anlegen.");
    }
    if !f.has_readme {
        add("no_readme", Level::Warn, "README fehlt".into(), "Eine README anlegen.");
    }
    if f.detached {
        add("detached", Level::Warn, "Detached HEAD".into(), "Auf einen Branch wechseln.");
    }
    if let Some(p) = &f.in_progress {
        add("in_progress", Level::Error, format!("Laufender {p}"), "Abschließen oder abbrechen.");
    }
    if f.conflicts > 0 {
        add("conflicts", Level::Error, format!("{} Konflikte", f.conflicts), "Konflikte auflösen.");
    }
    if f.has_remote && f.remote_ok == Some(false) {
        add("remote", Level::Error, "Remote nicht erreichbar".into(), "Netzwerk/URL/Zugang prüfen.");
    }
    if !f.has_remote {
        add("remote", Level::Warn, "Kein Remote konfiguriert".into(), "Remote hinzufügen, sonst gibt es kein Backup.");
    }
    if f.secrets == Some(true) {
        add("secrets", Level::Error, "gitleaks meldet mögliche Secrets".into(), "Funde prüfen und Secrets rotieren.");
    }
    out
}

pub fn overall(findings: &[Finding]) -> Level {
    findings.iter().map(|f| f.level).max().unwrap_or(Level::Ok)
}
