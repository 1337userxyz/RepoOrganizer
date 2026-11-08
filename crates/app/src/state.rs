use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use repoorganizer_core::cache::RepoStats;
use repoorganizer_core::config::{detect_user_from_url, LocalConfig, SharedConfig};
use repoorganizer_core::drift::Message;
use repoorganizer_core::health::{overall, Finding};
use repoorganizer_core::level::Level;
use repoorganizer_core::model::{Location, OsPaths, Repo, Role, Snapshot, Status};
use repoorganizer_core::paths::{resolve, Os, Resolved};
use repoorganizer_core::scan::ScanClass;
use repoorganizer_core::stats::Bar;
use repoorganizer_core::views::{keyword_matches, view_matches, RepoCtx};
use repoorganizer_git::CliGit;
use slint::{ModelRc, VecModel};

use crate::{present, BarItem, ScanWindow, SettingsWindow};

pub struct TaskEntry {
    pub id: u64,
    pub title: String,
    /// 0 läuft · 1 ok · 2 fehlgeschlagen
    pub state: i32,
    pub detail: String,
}

pub struct ScanItem {
    pub path: String,
    pub remote: Option<String>,
    pub class: ScanClass,
    pub checked: bool,
}

pub struct State {
    pub stats: HashMap<String, RepoStats>,
    pub stats_busy: bool,
    pub stats_path: Option<PathBuf>,
    pub drift: HashMap<String, Vec<Message>>,
    pub health: HashMap<String, Vec<(String, Finding)>>,
    pub health_busy: bool,
    pub view: String,
    pub cache_path: Option<PathBuf>,
    pub scan_items: Vec<ScanItem>,
    pub scanning: bool,
    pub scan_progress: String,
    pub cancel: Arc<AtomicBool>,
    pub tasks: Vec<TaskEntry>,
    pub next_task: u64,
    pub local: LocalConfig,
    pub local_path: PathBuf,
    pub shared: SharedConfig,
    pub shared_path: PathBuf,
    pub shared_mtime: Option<SystemTime>,
    pub snaps: HashMap<String, Snapshot>,
    pub os: Os,
    pub filter: String,
    pub selected: String,
    pub error: String,
    pub task: String,
    pub scan_win: Option<slint::Weak<ScanWindow>>,
    pub settings_win: Option<slint::Weak<SettingsWindow>>,
}

pub type Shared = Arc<Mutex<State>>;

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub fn resolved(state: &State, loc: &Location) -> Resolved {
    resolve(loc, state.os, &state.shared.mappings)
}

/// Passt Filtertext auf Name, Remote oder Tags.
pub fn matches(repo: &Repo, filter: &str) -> bool {
    let f = filter.trim().to_lowercase();
    f.is_empty()
        || repo.name.to_lowercase().contains(&f)
        || repo.remote_url.to_lowercase().contains(&f)
        || repo.tags.iter().any(|t| t.to_lowercase().contains(&f))
}

pub fn repo_status(state: &State, repo_id: &str) -> Status {
    state
        .shared
        .locations_of(repo_id)
        .filter_map(|l| state.snaps.get(&l.id))
        .map(|s| s.state.clone())
        .max_by_key(present::severity)
        .unwrap_or(Status::Unavailable)
}

pub fn offline_state(s: &Status) -> bool {
    matches!(s, Status::Unavailable | Status::NotARepo)
}

pub fn ctx_for<'a>(st: &'a State, repo: &'a Repo, now: i64) -> RepoCtx<'a> {
    let snaps: Vec<&Snapshot> = st.shared.locations_of(&repo.id).filter_map(|l| st.snaps.get(&l.id)).collect();
    RepoCtx {
        repo,
        statuses: snaps.iter().map(|s| s.state.clone()).collect(),
        last_sync: snaps.iter().flat_map(|s| s.last_pull.into_iter().chain(s.last_fetch)).max(),
        last_commit: snaps.iter().filter_map(|s| s.last_commit).max(),
        drift: st.drift.get(&repo.id).map(|m| !m.is_empty()).unwrap_or(false),
        offline: snaps.iter().all(|s| offline_state(&s.state)),
        health: st.health.get(&repo.id).map(|v| overall(&v.iter().map(|(_, f)| f.clone()).collect::<Vec<_>>())).unwrap_or(Level::Ok),
        has_build_artifacts: snaps.iter().any(|s| s.build_size_bytes > 0),
        now,
    }
}

pub fn view_ok(st: &State, view: &str, c: &RepoCtx) -> bool {
    let days = st.local.stale_days;
    if let Some(kw) = view.strip_prefix("b:") {
        keyword_matches(kw, c, days)
    } else if let Some(name) = view.strip_prefix("c:") {
        st.shared.views.iter().find(|v| v.name == name).map(|v| view_matches(v, c, days)).unwrap_or(true)
    } else {
        true
    }
}

pub fn level_kind(l: Level) -> i32 {
    match l {
        Level::Ok => 0,
        Level::Warn => 2,
        Level::Error => 3,
    }
}

pub fn bars(b: &[Bar]) -> ModelRc<BarItem> {
    let max = b.iter().map(|x| x.value).max().unwrap_or(0).max(1) as f32;
    ModelRc::new(VecModel::from(b.iter().map(|x| BarItem { label: x.label.as_str().into(), value: x.value as i32, frac: x.value as f32 / max }).collect::<Vec<_>>()))
}

pub fn suggest_clone_dest(st: &State, repo_id: &str) -> String {
    let repo = st.shared.repos.iter().find(|r| r.id == repo_id);
    let repo_name = repo.map(|r| r.name.as_str()).unwrap_or(repo_id);

    // 1. Gibt es einen Standort, dessen Mapping einen lokalen Pfad ergibt?
    for l in st.shared.locations_of(repo_id) {
        if let Resolved::Local(p) = resolved(st, l) {
            return p.display().to_string();
        }
    }

    let bp = &st.shared.base_paths;

    // 2. Geteilter Basispfad laut Hauptkonfig
    if st.os == Os::Windows {
        if let Some(shared_win) = &bp.shared_windows {
            if Path::new(shared_win).exists() {
                return format!(r"{shared_win}\{repo_name}");
            }
        }
    } else if let Some(shared_unix) = &bp.shared_unix {
        if Path::new(shared_unix).exists() {
            return format!("{shared_unix}/{repo_name}");
        }
    }

    // 3. Lokaler Basispfad für aktuelles Betriebssystem laut Hauptkonfig
    if st.os == Os::Windows {
        if let Some(local_win) = &bp.local_windows {
            if Path::new(local_win).exists() {
                return format!(r"{local_win}\{repo_name}");
            }
        }
    } else if let Some(local_linux) = &bp.local_linux {
        if Path::new(local_linux).exists() {
            return format!("{local_linux}/{repo_name}");
        }
    }

    // 4. Fallbacks (Hardcoded Defaults)
    if Path::new("/r/git").exists() {
        return format!("/r/git/{repo_name}");
    }
    if st.os == Os::Windows && Path::new(r"R:\git").exists() {
        return format!(r"R:\git\{repo_name}");
    }
    if let Ok(home) = std::env::var("HOME") {
        let projekte = format!("{home}/Projekte");
        if Path::new(&projekte).exists() {
            return format!("{projekte}/{repo_name}");
        }
    }

    format!("./{repo_name}")
}

pub fn name_from_path(path: &str) -> String {
    let trimmed = path.trim().trim_end_matches(&['/', '\\'][..]);
    let seg = trimmed.rsplit(&['/', '\\'][..]).next().unwrap_or(trimmed);
    seg.strip_suffix(".git").unwrap_or(seg).trim().to_string()
}

pub fn add_repo(st: &mut State, name: &str, path: &str, remote_url: &str) -> String {
    let eff_name = if !name.trim().is_empty() {
        name.trim().to_string()
    } else {
        name_from_path(path)
    };
    let base = present::slug(&eff_name);
    let base = if base.is_empty() { "repo".to_string() } else { base };
    let mut id = base.clone();
    let mut n = 2;
    while st.shared.repos.iter().any(|r| r.id == id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    let eff_remote = if !remote_url.trim().is_empty() {
        remote_url.trim().to_string()
    } else {
        let p = path.trim();
        let existing = if !p.is_empty() {
            CliGit::new(st.local.git_path.as_deref()).remote_origin(Path::new(p))
        } else {
            None
        };
        existing.unwrap_or_else(|| st.local.derive_remote_url(&eff_name))
    };
    if st.local.git_user.trim().is_empty() {
        if let Some(u) = detect_user_from_url(&eff_remote) {
            st.local.git_user = u;
            save_local(st);
        }
    }
    let mut paths = OsPaths::default();
    let p = Some(path.trim().to_string());
    match st.os {
        Os::Windows => paths.windows = p,
        Os::Linux => paths.linux = p,
        Os::MacOs => paths.macos = p,
    }
    st.shared.repos.push(Repo {
        id: id.clone(),
        name: eff_name.clone(),
        remote_url: eff_remote,
        tags: vec![],
        notes: String::new(),
        favorite: false,
    });
    st.shared.locations.push(Location {
        id: format!("{id}-1"),
        repo_id: id.clone(),
        label: eff_name,
        paths,
        role: Role::Primary,
        auto_fetch: false,
    });
    st.selected = id.clone();
    match st.shared.save(&st.shared_path) {
        Ok(()) => {
            st.error.clear();
            st.shared_mtime = file_mtime(&st.shared_path);
        }
        Err(e) => st.error = e.to_string(),
    }
    id
}

pub fn file_mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).ok()?.modified().ok()
}

pub fn save_shared(st: &mut State) {
    match st.shared.save(&st.shared_path) {
        Ok(()) => {
            st.error.clear();
            st.shared_mtime = file_mtime(&st.shared_path);
        }
        Err(e) => st.error = e.to_string(),
    }
}

pub fn save_local(st: &mut State) {
    if let Err(e) = st.local.save(&st.local_path) {
        st.error = e.to_string();
    }
}

pub fn open_external(path: &Path, tool: &str) {
    let _ = match tool {
        "explorer" => kit_launcher::open_path(path),
        "terminal" => kit_launcher::open_in_terminal(path),
        "code" => kit_launcher::open_in_editor(path, Some("code")),
        _ => return,
    };
}

pub fn open_url(url: &str) {
    let _ = kit_launcher::open_url(url);
}
