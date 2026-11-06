//! Smarte Ansichten: regelbasierte Filter auf Snapshot/Health/Tags.
use crate::config::SavedView;
use crate::level::Level;
use crate::model::{Repo, Status};

/// (Schlüsselwort, Anzeigename) der eingebauten Ansichten.
pub const BUILTIN: &[(&str, &str)] = &[
    ("today", "Heute geändert"),
    ("unpushed", "Unpushed"),
    ("behind", "Behind"),
    ("dirty", "Dirty"),
    ("stale", "Veraltet"),
    ("drift", "Drift"),
    ("offline", "Offline"),
    ("favorite", "Favoriten"),
    ("health", "Health-Warnungen"),
    ("build", "Build-Daten"),
];

pub struct RepoCtx<'a> {
    pub repo: &'a Repo,
    pub statuses: Vec<Status>,
    /// Letzter Pull/Fetch über alle Standorte (max).
    pub last_sync: Option<i64>,
    pub last_commit: Option<i64>,
    pub drift: bool,
    pub offline: bool,
    pub health: Level,
    pub has_build_artifacts: bool,
    pub now: i64,
}

pub fn keyword_matches(kw: &str, c: &RepoCtx, stale_days: u32) -> bool {
    let any = |f: fn(&Status) -> bool| c.statuses.iter().any(f);
    match kw {
        "today" => c.last_commit.map(|t| c.now - t < 86400).unwrap_or(false) || any(|s| matches!(s, Status::Dirty)),
        "unpushed" => any(|s| matches!(s, Status::Ahead(_))),
        "behind" => any(|s| matches!(s, Status::Behind(_))),
        "dirty" => any(|s| matches!(s, Status::Dirty)),
        "stale" => !c.offline && c.last_sync.map(|t| c.now - t > stale_days as i64 * 86400).unwrap_or(true),
        "drift" => c.drift,
        "offline" => c.offline,
        "online" => !c.offline,
        "favorite" => c.repo.favorite,
        "health" => c.health >= Level::Warn,
        "build" => c.has_build_artifacts,
        _ => false,
    }
}

/// Alle Tags müssen passen (UND); Status-Schlüsselwörter: mindestens eines (ODER);
/// `older_than_days`: letzter Sync liegt länger zurück.
pub fn view_matches(v: &SavedView, c: &RepoCtx, default_stale_days: u32) -> bool {
    let tags_ok = v.tags.iter().all(|t| c.repo.tags.iter().any(|x| x.eq_ignore_ascii_case(t)));
    let status_ok = v.status.is_empty() || v.status.iter().any(|k| keyword_matches(k, c, default_stale_days));
    let age_ok = v.older_than_days.map(|d| c.last_sync.map(|t| c.now - t > d as i64 * 86400).unwrap_or(true)).unwrap_or(true);
    tags_ok && status_ok && age_ok
}
