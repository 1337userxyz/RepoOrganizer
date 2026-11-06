mod present;

use repoorganizer_core::config::{detect_user_from_url, LocalConfig, SharedConfig};
use repoorganizer_core::model::{Location, OsPaths, Repo, Role, Snapshot, Status};
use repoorganizer_core::paths::{resolve, Os, Resolved};
use repoorganizer_core::reach::{self, Reachability};
use repoorganizer_core::scan::{adopt, classify, link_location, Adopt, ScanClass};
use repoorganizer_git::scan::{scan, ScanOptions};
use std::sync::atomic::{AtomicBool, Ordering};
use repoorganizer_core::cache::{self, RepoStats};
use repoorganizer_core::stats::{summarize, to_activity, Bar};
use repoorganizer_core::config::SavedView;
use repoorganizer_core::drift::{drift_messages, DriftLoc, Message};
use repoorganizer_core::health::{evaluate, overall, Finding, ALL_CHECKS};
use repoorganizer_core::level::Level;
use repoorganizer_core::paths::PathMapping;
use repoorganizer_core::views::{keyword_matches, view_matches, RepoCtx, BUILTIN};
use repoorganizer_git::facts::FactOptions;
use repoorganizer_git::{guard_pull, guard_push, snapshots_parallel, CliGit, GitBackend, PullMode};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

slint::include_modules!();

struct TaskEntry {
    id: u64,
    title: String,
    /// 0 läuft · 1 ok · 2 fehlgeschlagen
    state: i32,
    detail: String,
}

struct ScanItem {
    path: String,
    remote: Option<String>,
    class: ScanClass,
    checked: bool,
}

struct State {
    stats: HashMap<String, RepoStats>,
    stats_busy: bool,
    stats_path: Option<PathBuf>,
    drift: HashMap<String, Vec<Message>>,
    health: HashMap<String, Vec<(String, Finding)>>,
    health_busy: bool,
    view: String,
    cache_path: Option<PathBuf>,
    scan_items: Vec<ScanItem>,
    scanning: bool,
    scan_progress: String,
    cancel: Arc<AtomicBool>,
    tasks: Vec<TaskEntry>,
    next_task: u64,
    local: LocalConfig,
    local_path: PathBuf,
    shared: SharedConfig,
    shared_path: PathBuf,
    shared_mtime: Option<SystemTime>,
    snaps: HashMap<String, Snapshot>,
    os: Os,
    filter: String,
    selected: String,
    error: String,
    task: String,
    scan_win: Option<slint::Weak<ScanWindow>>,
    settings_win: Option<slint::Weak<SettingsWindow>>,
}

type Shared = Arc<Mutex<State>>;

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn resolved(state: &State, loc: &Location) -> Resolved {
    resolve(loc, state.os, &state.shared.mappings)
}

/// Passt Filtertext auf Name, Remote oder Tags.
fn matches(repo: &Repo, filter: &str) -> bool {
    let f = filter.trim().to_lowercase();
    f.is_empty()
        || repo.name.to_lowercase().contains(&f)
        || repo.remote_url.to_lowercase().contains(&f)
        || repo.tags.iter().any(|t| t.to_lowercase().contains(&f))
}

fn repo_status(state: &State, repo_id: &str) -> Status {
    state
        .shared
        .locations_of(repo_id)
        .filter_map(|l| state.snaps.get(&l.id))
        .map(|s| s.state.clone())
        .max_by_key(present::severity)
        .unwrap_or(Status::Unavailable)
}

fn offline_state(s: &Status) -> bool {
    matches!(s, Status::Unavailable | Status::NotARepo)
}

fn ctx_for<'a>(st: &'a State, repo: &'a Repo, now: i64) -> RepoCtx<'a> {
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

fn view_ok(st: &State, view: &str, c: &RepoCtx) -> bool {
    let days = st.local.stale_days;
    if let Some(kw) = view.strip_prefix("b:") {
        keyword_matches(kw, c, days)
    } else if let Some(name) = view.strip_prefix("c:") {
        st.shared.views.iter().find(|v| v.name == name).map(|v| view_matches(v, c, days)).unwrap_or(true)
    } else {
        true
    }
}

fn level_kind(l: Level) -> i32 {
    match l {
        Level::Ok => 0,
        Level::Warn => 2,
        Level::Error => 3,
    }
}

fn bars(b: &[Bar]) -> ModelRc<BarItem> {
    let max = b.iter().map(|x| x.value).max().unwrap_or(0).max(1) as f32;
    ModelRc::new(VecModel::from(b.iter().map(|x| BarItem { label: x.label.as_str().into(), value: x.value as i32, frac: x.value as f32 / max }).collect::<Vec<_>>()))
}

fn apply_theme_all(ui: &MainWindow, st: &State) {
    let t: SharedString = st.local.theme.as_str().into();
    ui.invoke_apply_theme(t.clone());
    if let Some(sw) = st.scan_win.as_ref().and_then(|w| w.upgrade()) {
        sw.invoke_apply_theme(t.clone());
    }
    if let Some(cfgw) = st.settings_win.as_ref().and_then(|w| w.upgrade()) {
        cfgw.invoke_apply_theme(t);
    }
}

fn apply(ui: &MainWindow, st: &mut State) {
    let now_t = now();
    apply_theme_all(ui, st);

    // Alle vorkommenden Branches für die Branch-ComboBox sammeln
    let mut all_branches: Vec<String> = st.snaps.values().filter_map(|s| s.branch.clone()).filter(|b| !b.is_empty()).collect();
    all_branches.sort();
    all_branches.dedup();
    let mut b_opts: Vec<SharedString> = vec!["Branch: Alle".into()];
    b_opts.extend(all_branches.iter().map(|b| SharedString::from(b.as_str())));
    ui.set_branch_options(ModelRc::new(VecModel::from(b_opts)));

    let col_repo = ui.get_col_repo_filter();
    let col_status = ui.get_col_status_filter();
    let col_branch = ui.get_col_branch_filter().to_string();
    let col_sync = ui.get_col_sync_filter();
    let col_commit = ui.get_col_commit_filter();
    let col_remote = ui.get_col_remote_filter();

    // Gefilterte Repos sammeln (inkl. Spalten-ComboBox-Filter) und nach Commit-Datum sortieren
    let mut filtered: Vec<(Repo, Option<i64>)> = st
        .shared
        .repos
        .iter()
        .filter(|r| matches(r, &st.filter) && view_ok(st, &st.view, &ctx_for(st, r, now_t)))
        .filter_map(|r| {
            let snaps: Vec<&Snapshot> = st.shared.locations_of(&r.id).filter_map(|l| st.snaps.get(&l.id)).collect();
            let last_c = snaps.iter().filter_map(|s| s.last_commit).max();
            let status = repo_status(st, &r.id);
            let branch = snaps.iter().find_map(|s| s.branch.clone()).unwrap_or_default();
            let ahead = snaps.iter().map(|s| s.ahead).max().unwrap_or(0);
            let behind = snaps.iter().map(|s| s.behind).max().unwrap_or(0);
            let dirty = snaps.iter().map(|s| s.dirty_files + s.untracked).max().unwrap_or(0);
            let stashes = snaps.iter().map(|s| s.stash_count).max().unwrap_or(0);
            let has_drift = st.drift.get(&r.id).map(|m| !m.is_empty()).unwrap_or(false);
            let eff_remote = if !r.remote_url.is_empty() {
                r.remote_url.clone()
            } else {
                snaps.iter().find_map(|s| s.remote_url.clone()).unwrap_or_default()
            };

            if col_repo == 1 && !r.favorite {
                return None;
            }
            let status_ok = match col_status {
                1 => matches!(status, Status::UpToDate) && ahead == 0 && behind == 0 && dirty == 0,
                2 => ahead > 0,
                3 => behind > 0,
                4 => (ahead > 0 && behind > 0) || matches!(status, Status::Diverged),
                5 => eff_remote.is_empty(),
                6 => matches!(status, Status::Unavailable),
                7 => matches!(status, Status::Error(_) | Status::NotARepo),
                _ => true,
            };
            if !status_ok {
                return None;
            }
            if !col_branch.is_empty() && col_branch != "Branch: Alle" && branch != col_branch {
                return None;
            }
            let sync_ok = match col_sync {
                1 => dirty == 0,
                2 => dirty > 0,
                3 => stashes > 0,
                _ => true,
            };
            if !sync_ok {
                return None;
            }
            let commit_ok = match col_commit {
                2 => last_c.map(|ts| now_t - ts <= 86400).unwrap_or(false),
                3 => last_c.map(|ts| now_t - ts <= 7 * 86400).unwrap_or(false),
                4 => last_c.map(|ts| now_t - ts <= 30 * 86400).unwrap_or(false),
                5 => last_c.map(|ts| now_t - ts > 30 * 86400).unwrap_or(true),
                _ => true,
            };
            if !commit_ok {
                return None;
            }
            let remote_ok = match col_remote {
                1 => eff_remote.is_empty(),
                2 => !eff_remote.is_empty(),
                3 => matches!(status, Status::Error(_) | Status::NotARepo),
                4 => has_drift,
                _ => true,
            };
            if !remote_ok {
                return None;
            }

            let mut repo_clone = r.clone();
            if repo_clone.remote_url.is_empty() && !eff_remote.is_empty() {
                repo_clone.remote_url = eff_remote;
            }
            Some((repo_clone, last_c))
        })
        .collect();

    if col_commit == 1 {
        // Älteste zuerst
        filtered.sort_by(|(a, ca), (b, cb)| ca.unwrap_or(i64::MAX).cmp(&cb.unwrap_or(i64::MAX)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    } else {
        // Standard: Neueste zuerst
        filtered.sort_by(|(a, ca), (b, cb)| cb.unwrap_or(0).cmp(&ca.unwrap_or(0)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    }

    // Automatisch oberstes Repo auswählen (beim Programmstart oder wenn Auswahl durch Filter entfällt)
    if !filtered.is_empty() && (st.selected.is_empty() || !filtered.iter().any(|(r, _)| r.id == st.selected)) {
        st.selected = filtered[0].0.id.clone();
    } else if filtered.is_empty() && !st.shared.repos.iter().any(|r| r.id == st.selected) {
        st.selected.clear();
    }

    let mut rows = Vec::with_capacity(filtered.len());
    for (r, last_c) in &filtered {
        let status = repo_status(st, &r.id);
        let snaps: Vec<&Snapshot> = st.shared.locations_of(&r.id).filter_map(|l| st.snaps.get(&l.id)).collect();
        let branch = snaps.iter().find_map(|s| s.branch.clone()).unwrap_or_default();
        let ahead = snaps.iter().map(|s| s.ahead).max().unwrap_or(0);
        let behind = snaps.iter().map(|s| s.behind).max().unwrap_or(0);
        let dirty = snaps.iter().map(|s| s.dirty_files + s.untracked).max().unwrap_or(0);
        let stashes = snaps.iter().map(|s| s.stash_count).max().unwrap_or(0);
        let commits = snaps.iter().map(|s| s.commit_count).max().unwrap_or(0);
        let files = snaps.iter().map(|s| s.file_count).max().unwrap_or(0);
        let size_kb = snaps
            .iter()
            .map(|s| s.size_kb)
            .max()
            .unwrap_or(0)
            .max(st.stats.get(&r.id).map(|rs| rs.git_size / 1024).unwrap_or(0));
        let has_drift = st.drift.get(&r.id).map(|m| !m.is_empty()).unwrap_or(false);

        let build_bytes: u64 = snaps.iter().map(|s| s.build_size_bytes).sum();
        let build_size = repoorganizer_core::clean::format_bytes(build_bytes);
        let has_remote = !r.remote_url.is_empty();
        rows.push(RepoRow {
            id: r.id.as_str().into(),
            name: r.name.as_str().into(),
            remote: r.remote_url.as_str().into(),
            tags: r.tags.join(", ").into(),
            status: present::repo_display_status(&status, ahead, behind, dirty, has_remote).into(),
            error_msg: present::status_error_short(&status).into(),
            kind: present::repo_display_kind(&status, ahead, behind, dirty, has_remote),
            branch: branch.into(),
            sync_info: present::working_tree_text(dirty, stashes).into(),
            sync_kind: present::working_tree_kind(dirty),
            last_commit: present::format_commit_date(*last_c, now_t).into(),
            commits: if commits > 0 { commits.to_string().into() } else { "–".into() },
            files: if files > 0 { files.to_string().into() } else { "–".into() },
            size: present::format_size_kb(size_kb).into(),
            size_kind: present::size_kind(size_kb),
            build_size: build_size.into(),
            build_kind: present::build_size_kind(build_bytes),
            locations: st.shared.locations_of(&r.id).count() as i32,
            favorite: r.favorite,
            has_drift,
        });
    }
    ui.set_repos(ModelRc::new(VecModel::from(rows)));

    // Smart-Sync Zähler berechnen
    let mut sync_behind = 0;
    let mut sync_dirty = 0;
    let mut sync_ahead = 0;
    let mut dirty_repos = 0;
    let mut top_dirty_repo: Option<(String, String, u32)> = None;

    for r in &st.shared.repos {
        for l in st.shared.locations_of(&r.id) {
            if let Resolved::Local(p) = resolved(st, l) {
                if let Some(s) = st.snaps.get(&l.id) {
                    let dirty = s.dirty_files + s.untracked;
                    if dirty > 0 {
                        dirty_repos += 1;
                        sync_dirty += 1;
                        let desc = format!("{} ({}) – {} uncommittete Datei(en)", r.name, p.display(), dirty);
                        if top_dirty_repo.as_ref().map(|(_, _, d)| dirty > *d).unwrap_or(true) {
                            top_dirty_repo = Some((l.id.clone(), desc, dirty));
                        }
                    } else if s.ahead > 0 {
                        sync_ahead += 1;
                    }
                    if s.behind > 0 && dirty == 0 {
                        sync_behind += 1;
                    }
                }
            }
        }
    }

    let mut sync_missing_bp = 0;
    let bp = &st.shared.base_paths;
    for r in &st.shared.repos {
        let mut check_bp = |base: Option<&String>, on_os: bool| {
            if let Some(b) = base {
                let b_trimmed = b.trim();
                if !b_trimmed.is_empty() && on_os {
                    let sep = if st.os == Os::Windows { "\\" } else { "/" };
                    let target_path = format!("{b_trimmed}{sep}{}", r.name);
                    let exists = st.shared.locations_of(&r.id).any(|l| match resolved(st, l) {
                        Resolved::Local(p) => p == std::path::Path::new(&target_path),
                        _ => {
                            if st.os == Os::Windows {
                                l.paths.windows.as_deref() == Some(&target_path)
                            } else {
                                l.paths.linux.as_deref() == Some(&target_path)
                            }
                        }
                    });
                    if !exists {
                        sync_missing_bp += 1;
                    }
                }
            }
        };

        check_bp(bp.shared_unix.as_ref(), st.os != Os::Windows);
        check_bp(bp.local_linux.as_ref(), st.os == Os::Linux);
        check_bp(bp.shared_windows.as_ref(), st.os == Os::Windows);
        check_bp(bp.local_windows.as_ref(), st.os == Os::Windows);
    }

    let total_sync_actions = sync_behind + sync_dirty + sync_ahead + sync_missing_bp;
    ui.set_sync_behind_count(sync_behind as i32);
    ui.set_sync_dirty_count(sync_dirty as i32);
    ui.set_sync_ahead_count(sync_ahead as i32);
    ui.set_sync_missing_bp_count(sync_missing_bp as i32);
    ui.set_sync_total_actions(total_sync_actions as i32);

    ui.set_dirty_repos_count(dirty_repos);
    if let Some((loc_id, desc, _)) = top_dirty_repo {
        ui.set_quick_commit_target_id(loc_id.into());
        ui.set_quick_commit_target_desc(desc.into());
    } else if let Some(r) = st.shared.repos.iter().find(|r| r.id == st.selected) {
        let loc = st.shared.locations_of(&r.id).find_map(|l| match resolved(st, l) {
            Resolved::Local(p) => Some((l.id.clone(), format!("{} ({})", r.name, p.display()))),
            _ => None,
        });
        if let Some((loc_id, desc)) = loc {
            ui.set_quick_commit_target_id(loc_id.into());
            ui.set_quick_commit_target_desc(desc.into());
        } else {
            ui.set_quick_commit_target_id("".into());
            ui.set_quick_commit_target_desc(r.name.as_str().into());
        }
    } else {
        ui.set_quick_commit_target_id("".into());
        ui.set_quick_commit_target_desc("Kein aktives Repository".into());
    }

    ui.set_stat_busy(st.stats_busy);
    match st.stats.get(&st.selected) {
        Some(rs) => {
            let s = summarize(&rs.activity, now_t);
            ui.set_stat_text(
                format!(
                    "Commits (12 Monate): {}   ·   letzte 30 Tage: {}\nZeilen: +{} / −{}   ·   .git: {:.1} MB\nLetzte Aktivität: {}   ·   Stand: {}",
                    s.total_commits,
                    s.commits_30d,
                    s.added,
                    s.deleted,
                    rs.git_size as f64 / 1_048_576.0,
                    s.last_activity.as_deref().unwrap_or("–"),
                    present::ago(Some(rs.fetched_at), now_t)
                )
                .into(),
            );
            ui.set_stat_weeks(bars(&s.weeks));
            ui.set_stat_months(bars(&s.months));
            ui.set_stat_authors(bars(&s.authors));
            ui.set_stat_heat(ModelRc::new(VecModel::from(s.heat.iter().map(|c| HeatItem { col: c.col as i32, row: c.row as i32, level: c.level as i32 }).collect::<Vec<_>>())));
        }
        None => {
            ui.set_stat_text(if st.stats_busy { "Lade Statistik…".into() } else { "Noch keine Statistik – „Neu laden“ drücken.".into() });
            ui.set_stat_weeks(ModelRc::default());
            ui.set_stat_months(ModelRc::default());
            ui.set_stat_authors(ModelRc::default());
            ui.set_stat_heat(ModelRc::default());
        }
    }
    // Ansichten (Sidebar) mit Zählern
    let mut vrows = vec![ViewRow { key: "".into(), name: "Alle Repos".into(), count: st.shared.repos.len() as i32, selected: st.view.is_empty(), custom: false }];
    let mut keys: Vec<(String, String, bool)> = BUILTIN.iter().map(|(k, n)| (format!("b:{k}"), n.to_string(), false)).collect();
    keys.extend(st.shared.views.iter().map(|v| (format!("c:{}", v.name), v.name.clone(), true)));
    for (key, name, custom) in keys {
        let count = st.shared.repos.iter().filter(|r| view_ok(st, &key, &ctx_for(st, r, now_t))).count() as i32;
        vrows.push(ViewRow { selected: st.view == key, key: key.into(), name: name.into(), count, custom });
    }
    ui.set_views(ModelRc::new(VecModel::from(vrows)));
    ui.set_active_view(st.view.as_str().into());
    let drows: Vec<InfoRow> = st
        .drift
        .get(&st.selected)
        .map(|m| m.iter().map(|m| InfoRow { label: "".into(), text: m.text.as_str().into(), advice: "".into(), kind: level_kind(m.level) }).collect())
        .unwrap_or_default();
    ui.set_drift_rows(ModelRc::new(VecModel::from(drows)));
    let hf = st.health.get(&st.selected);
    ui.set_health_rows(ModelRc::new(VecModel::from(
        hf.map(|v| {
            v.iter()
                .map(|(label, f)| InfoRow { label: label.as_str().into(), text: f.text.as_str().into(), advice: f.advice.as_str().into(), kind: level_kind(f.level) })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default(),
    )));
    let (htext, hkind) = match hf {
        None => ("Noch nicht geprüft".to_string(), 4),
        Some(v) if v.is_empty() => ("Alles in Ordnung".to_string(), 0),
        Some(v) => {
            let lvl = overall(&v.iter().map(|(_, f)| f.clone()).collect::<Vec<_>>());
            (format!("{} Hinweis(e) – {}", v.len(), if lvl == Level::Error { "Fehler" } else { "Warnung" }), level_kind(lvl))
        }
    };
    ui.set_health_text(htext.into());
    ui.set_health_kind(hkind);
    ui.set_health_busy(st.health_busy);

    ui.set_error_text(st.error.as_str().into());
    ui.set_task_text(st.task.as_str().into());

    if let Some(sw) = st.scan_win.as_ref().and_then(|w| w.upgrade()) {
        sw.set_scanning(st.scanning);
        sw.set_scan_progress(st.scan_progress.as_str().into());
        sw.set_scan_rows(ModelRc::new(VecModel::from(
            st.scan_items
                .iter()
                .map(|i| {
                    let (text, kind, known) = match &i.class {
                        ScanClass::Known { .. } => ("Bekannt".to_string(), 4, true),
                        ScanClass::ExtraLocation { repo_id } => {
                            let name = st.shared.repos.iter().find(|r| &r.id == repo_id).map(|r| r.name.clone()).unwrap_or_default();
                            (format!("Weiterer Standort: {name}"), 1, false)
                        }
                        ScanClass::New => ("Neues Repo".to_string(), 0, false),
                    };
                    ScanRow { path: i.path.as_str().into(), remote: i.remote.clone().unwrap_or_default().into(), kind_text: text.into(), kind, checked: i.checked && !known, known }
                })
                .collect::<Vec<_>>(),
        )));
    }
    ui.set_tasks(ModelRc::new(VecModel::from(
        st.tasks
            .iter()
            .rev()
            .map(|t| TaskRow { title: t.title.as_str().into(), state: t.state, detail: t.detail.lines().last().unwrap_or("").into() })
            .collect::<Vec<_>>(),
    )));
    ui.set_selected_id(st.selected.as_str().into());
    let mut locs = Vec::new();
    if let Some(r) = st.shared.repos.iter().find(|r| r.id == st.selected) {
        let eff_remote = if !r.remote_url.is_empty() {
            r.remote_url.clone()
        } else {
            st.shared.locations_of(&r.id).filter_map(|l| st.snaps.get(&l.id)).find_map(|s| s.remote_url.clone()).unwrap_or_default()
        };
        let suggested = if eff_remote.is_empty() {
            st.local.derive_remote_url(&r.name)
        } else {
            eff_remote.clone()
        };
        ui.set_selected_name(r.name.as_str().into());
        ui.set_selected_remote(eff_remote.as_str().into());
        ui.set_suggested_remote(suggested.as_str().into());
        ui.set_selected_tags(r.tags.join(", ").into());
        ui.set_selected_notes(r.notes.as_str().into());
        ui.set_selected_favorite(r.favorite);

        let is_offline = st.shared.locations_of(&r.id).all(|l| {
            match resolved(st, l) {
                Resolved::OtherOs => true,
                Resolved::Local(_) => st.snaps.get(&l.id).map(|s| matches!(s.state, Status::Unavailable)).unwrap_or(true),
            }
        });
        ui.set_selected_is_offline(is_offline);

        let clone_dest = suggest_clone_dest(st, &r.id);
        ui.set_selected_suggested_dest(clone_dest.into());

        let mut best_dirty_loc: Option<(String, String, u32)> = None;
        let mut best_ahead_loc: Option<(String, String, u32)> = None;

        let t = now();
        for l in st.shared.locations_of(&r.id) {
            let (path, other_os) = match resolved(st, l) {
                Resolved::Local(p) => (p.display().to_string(), false),
                Resolved::OtherOs => {
                    let any = l.paths.windows.clone().or(l.paths.linux.clone()).or(l.paths.macos.clone()).unwrap_or_default();
                    (format!("{any}  (auf anderem System)"), true)
                }
            };
            let snap = st.snaps.get(&l.id);
            let ahead = snap.map(|s| s.ahead).unwrap_or(0);
            let behind = snap.map(|s| s.behind).unwrap_or(0);
            let dirty = snap.map(|s| s.dirty_files + s.untracked).unwrap_or(0);
            let stashes = snap.map(|s| s.stash_count).unwrap_or(0);
            let has_remote = snap.and_then(|s| s.remote_url.as_ref()).map(|u| !u.is_empty()).unwrap_or(!r.remote_url.is_empty());
            let state = if other_os { Status::Unavailable } else { snap.map(|s| s.state.clone()).unwrap_or(Status::Unavailable) };
            let can_pull = !other_os && guard_pull(&state).is_ok();
            let can_push = !other_os && guard_push(&state).is_ok() && ahead > 0;
            let can_commit = !other_os && dirty > 0;
            let b_bytes = snap.map(|s| s.build_size_bytes).unwrap_or(0);
            let can_clean = !other_os && b_bytes > 0;
            let b_size = repoorganizer_core::clean::format_bytes(b_bytes);
            let (rel_text, rel_kind) = if other_os {
                ("⭘ Anderes OS".to_string(), 4)
            } else if matches!(state, Status::Unavailable) {
                ("⭘ Offline".to_string(), 4)
            } else if matches!(state, Status::Error(_)) {
                ("⚠ Fehler".to_string(), 3)
            } else {
                present::remote_relation_badge(ahead, behind, has_remote)
            };

            if !other_os {
                let lbl = if l.label.is_empty() { l.id.clone() } else { l.label.clone() };
                let display_name = format!("{lbl} ({path})");
                if dirty > 0 {
                    if best_dirty_loc.as_ref().map(|(_, _, d)| dirty > *d).unwrap_or(true) {
                        best_dirty_loc = Some((l.id.clone(), display_name.clone(), dirty));
                    }
                }
                if ahead > 0 {
                    if best_ahead_loc.as_ref().map(|(_, _, a)| ahead > *a).unwrap_or(true) {
                        best_ahead_loc = Some((l.id.clone(), display_name, ahead));
                    }
                }
            }

            locs.push(LocationRow {
                id: l.id.as_str().into(),
                can_pull,
                can_push,
                can_commit,
                can_clean,
                build_size: b_size.into(),
                label: if l.label.is_empty() { l.id.as_str().into() } else { l.label.as_str().into() },
                path: path.into(),
                role: format!("{:?}", l.role).to_lowercase().into(),
                status: rel_text.into(),
                error_msg: present::status_error(&state).into(),
                kind: rel_kind,
                branch: snap.and_then(|s| s.branch.clone()).unwrap_or_default().into(),
                ahead: ahead as i32,
                behind: behind as i32,
                dirty: dirty as i32,
                stashes: stashes as i32,
                working_tree: present::working_tree_text(dirty, stashes).into(),
                working_tree_kind: present::working_tree_kind(dirty),
                fetched: present::ago(snap.and_then(|s| s.last_fetch), t).into(),
                pulled: present::ago(snap.and_then(|s| s.last_pull), t).into(),
                other_os,
            });
        }

        if let Some((loc_id, name, dirty_count)) = best_dirty_loc {
            ui.set_selected_active_loc_id(loc_id.into());
            ui.set_selected_active_loc_name(name.into());
            ui.set_selected_dirty_count(dirty_count as i32);
            ui.set_selected_ahead_count(0);
        } else if let Some((loc_id, name, ahead_count)) = best_ahead_loc {
            ui.set_selected_active_loc_id(loc_id.into());
            ui.set_selected_active_loc_name(name.into());
            ui.set_selected_dirty_count(0);
            ui.set_selected_ahead_count(ahead_count as i32);
        } else {
            ui.set_selected_active_loc_id("".into());
            ui.set_selected_active_loc_name("".into());
            ui.set_selected_dirty_count(0);
            ui.set_selected_ahead_count(0);
        }

        // Fehlende Basispfade für das ausgewählte Repo ermitteln
        let bp = &st.shared.base_paths;
        let mut missing_bp_rows = Vec::new();

        // 1. Shared Unix (z. B. /r/git)
        if let Some(shared_u) = &bp.shared_unix {
            let su = shared_u.trim();
            if !su.is_empty() && st.os != Os::Windows {
                let target_path = format!("{su}/{}", r.name);
                let has_location = st.shared.locations_of(&r.id).any(|l| {
                    match resolved(st, l) {
                        Resolved::Local(p) => p == std::path::Path::new(&target_path),
                        _ => l.paths.linux.as_deref() == Some(&target_path),
                    }
                });
                if !has_location {
                    missing_bp_rows.push(MissingBasePathRow {
                        label: format!("In RAM-Disk ({su})").into(),
                        target_path: target_path.into(),
                        base_kind: "shared_unix".into(),
                    });
                }
            }
        }

        // 2. Local Linux (z. B. /home/tristan/Projekte)
        if let Some(local_l) = &bp.local_linux {
            let ll = local_l.trim();
            if !ll.is_empty() && st.os == Os::Linux {
                let target_path = format!("{ll}/{}", r.name);
                let has_location = st.shared.locations_of(&r.id).any(|l| {
                    match resolved(st, l) {
                        Resolved::Local(p) => p == std::path::Path::new(&target_path),
                        _ => l.paths.linux.as_deref() == Some(&target_path),
                    }
                });
                if !has_location {
                    let display_base = if let Ok(home) = std::env::var("HOME") {
                        if ll.starts_with(&home) {
                            format!("~{}", &ll[home.len()..])
                        } else {
                            ll.to_string()
                        }
                    } else {
                        ll.to_string()
                    };
                    missing_bp_rows.push(MissingBasePathRow {
                        label: format!("In Lokale SSD ({display_base})").into(),
                        target_path: target_path.into(),
                        base_kind: "local_linux".into(),
                    });
                }
            }
        }

        // 3. Shared Windows (z. B. R:\git) – Ignoriert wenn leer oder nicht Windows
        if let Some(shared_w) = &bp.shared_windows {
            let sw = shared_w.trim();
            if !sw.is_empty() && st.os == Os::Windows {
                let target_path = format!(r"{sw}\{}", r.name);
                let has_location = st.shared.locations_of(&r.id).any(|l| {
                    match resolved(st, l) {
                        Resolved::Local(p) => p == std::path::Path::new(&target_path),
                        _ => l.paths.windows.as_deref() == Some(&target_path),
                    }
                });
                if !has_location {
                    missing_bp_rows.push(MissingBasePathRow {
                        label: format!("In Windows Geteilt ({sw})").into(),
                        target_path: target_path.into(),
                        base_kind: "shared_windows".into(),
                    });
                }
            }
        }

        // 4. Local Windows – Ignoriert wenn leer oder nicht Windows
        if let Some(local_w) = &bp.local_windows {
            let lw = local_w.trim();
            if !lw.is_empty() && st.os == Os::Windows {
                let target_path = format!(r"{lw}\{}", r.name);
                let has_location = st.shared.locations_of(&r.id).any(|l| {
                    match resolved(st, l) {
                        Resolved::Local(p) => p == std::path::Path::new(&target_path),
                        _ => l.paths.windows.as_deref() == Some(&target_path),
                    }
                });
                if !has_location {
                    missing_bp_rows.push(MissingBasePathRow {
                        label: format!("In Windows Lokal ({lw})").into(),
                        target_path: target_path.into(),
                        base_kind: "local_windows".into(),
                    });
                }
            }
        }

        ui.set_missing_base_paths(ModelRc::new(VecModel::from(missing_bp_rows)));
    } else {
        ui.set_selected_name("Kein Repository".into());
        ui.set_selected_active_loc_id("".into());
        ui.set_selected_active_loc_name("".into());
        ui.set_selected_dirty_count(0);
        ui.set_selected_ahead_count(0);
        ui.set_missing_base_paths(ModelRc::default());
        ui.set_selected_remote("".into());
        ui.set_suggested_remote("".into());
        ui.set_selected_tags("".into());
        ui.set_selected_notes("".into());
        ui.set_selected_favorite(false);
        ui.set_selected_is_offline(false);
        ui.set_selected_suggested_dest("".into());
    }
    ui.set_locations(ModelRc::new(VecModel::from(locs)));
    let disk_msg = st.shared.locations.iter()
        .find_map(|l| match resolved(st, l) {
            Resolved::Local(p) => {
                let root_prefix = p.components().next().map(|c| c.as_os_str().to_string_lossy().to_string()).unwrap_or_default();
                repoorganizer_core::clean::disk_space(&p).map(|(free, total)| {
                    format!("{root_prefix} {} frei von {}",
                        repoorganizer_core::clean::format_bytes(free),
                        repoorganizer_core::clean::format_bytes(total)
                    )
                })
            }
            _ => None,
        })
        .unwrap_or_default();
    ui.set_disk_info(disk_msg.into());
    if let Some(cfgw) = st.settings_win.as_ref().and_then(|w| w.upgrade()) {
        cfgw.set_cfg_mappings(ModelRc::new(VecModel::from(
            st.shared.mappings.iter().map(|m| MappingRow { windows: m.windows.as_str().into(), unix: m.unix.as_str().into() }).collect::<Vec<_>>(),
        )));
        cfgw.set_cfg_checks(ModelRc::new(VecModel::from(
            ALL_CHECKS
                .iter()
                .map(|(id, label)| CheckRow { id: (*id).into(), label: (*label).into(), enabled: st.local.health.enabled(id) })
                .collect::<Vec<_>>(),
        )));
    }
}

fn sync_settings_fields(cfgw: &SettingsWindow, st: &State) {
    cfgw.set_cfg_theme(st.local.theme.as_str().into());
    cfgw.set_cfg_git_user(st.local.git_user.as_str().into());
    cfgw.set_cfg_git_path(st.local.git_path.clone().unwrap_or_default().into());
    cfgw.set_cfg_refresh_secs(st.local.refresh_interval_secs.to_string().into());
    cfgw.set_cfg_timeout_secs(st.local.reach_timeout_secs.to_string().into());
    cfgw.set_cfg_stale_days(st.local.stale_days.to_string().into());
    let bp = &st.shared.base_paths;
    cfgw.set_cfg_shared_win(bp.shared_windows.clone().unwrap_or_default().into());
    cfgw.set_cfg_shared_unix(bp.shared_unix.clone().unwrap_or_default().into());
    cfgw.set_cfg_local_linux(bp.local_linux.clone().unwrap_or_default().into());
    cfgw.set_cfg_local_win(bp.local_windows.clone().unwrap_or_default().into());
}

/// Drift je Repo: lokale Vergleiche zwischen erreichbaren Standorten, Offline-Standorte per letztem Snapshot.
fn compute_drift(git: &CliGit, groups: &[(String, Vec<(String, String, Option<PathBuf>)>)], snaps: &HashMap<String, Snapshot>) -> HashMap<String, Vec<Message>> {
    let mut out = HashMap::new();
    for (repo_id, locs) in groups {
        if locs.len() < 2 {
            continue;
        }
        let dl: Vec<DriftLoc> = locs
            .iter()
            .map(|(id, label, path)| {
                let s = snaps.get(id);
                DriftLoc {
                    label: label.clone(),
                    reachable: path.is_some() && s.map(|s| !offline_state(&s.state) && !matches!(s.state, Status::Error(_))).unwrap_or(false),
                    stale_at: s.and_then(|s| s.last_checked),
                    branch: s.and_then(|s| s.branch.clone()),
                    head_sha: s.and_then(|s| s.head_sha.clone()),
                    dirty: s.map(|s| s.dirty_files + s.untracked > 0).unwrap_or(false),
                }
            })
            .collect();
        let mut rel = |i: usize, j: usize| match (&locs[i].2, &locs[j].2) {
            (Some(a), Some(b)) => git.compare_locations(a, b),
            _ => None,
        };
        let msgs = drift_messages(&dl, &mut rel, now());
        if !msgs.is_empty() {
            out.insert(repo_id.clone(), msgs);
        }
    }
    out
}

/// Statistik (Commits, Zeilen, Autoren, Größe) des ersten erreichbaren Standorts laden.
fn load_stats(ui: &MainWindow, state: &Shared, repo_id: &str, force: bool) {
    if repo_id.is_empty() {
        return;
    }
    let (path, git_path) = {
        let mut st = state.lock().unwrap();
        if st.stats_busy {
            return;
        }
        if !force && st.stats.get(repo_id).map(|s| now() - s.fetched_at < 600).unwrap_or(false) {
            return;
        }
        let path = st
            .shared
            .locations_of(repo_id)
            .filter(|l| st.snaps.get(&l.id).map(|s| !offline_state(&s.state)).unwrap_or(false))
            .find_map(|l| match resolved(&st, l) {
                Resolved::Local(p) => Some(p),
                Resolved::OtherOs => None,
            });
        let Some(path) = path else {
            st.task = "Statistik: kein erreichbarer Standort".into();
            apply(ui, &mut st);
            return;
        };
        st.stats_busy = true;
        apply(ui, &mut st);
        (path, st.local.git_path.clone())
    };
    let (weak, state, repo_id) = (ui.as_weak(), state.clone(), repo_id.to_string());
    std::thread::spawn(move || {
        let git = CliGit::new(git_path.as_deref());
        let commits = git.collect_activity(&path, 365);
        let rs = RepoStats { fetched_at: now(), git_size: git.git_size(&path), activity: to_activity(&repo_id, &commits) };
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else { return };
            let mut st = state.lock().unwrap();
            st.stats.insert(repo_id, rs);
            st.stats_busy = false;
            if let Some(p) = &st.stats_path {
                let _ = cache::save_activity(p, &st.stats);
            }
            apply(&ui, &mut st);
        });
    });
}

/// Health-Checks für ein Repo (oder alle bei leerer ID) im Hintergrund.
fn run_health(ui: &MainWindow, state: &Shared, repo_id: &str) {
    let (targets, git_path, settings): (Vec<(String, Vec<(String, PathBuf)>)>, Option<String>, _) = {
        let mut st = state.lock().unwrap();
        if st.health_busy {
            return;
        }
        let targets = st
            .shared
            .repos
            .iter()
            .filter(|r| repo_id.is_empty() || r.id == repo_id)
            .map(|r| {
                let locs = st
                    .shared
                    .locations_of(&r.id)
                    .filter(|l| st.snaps.get(&l.id).map(|s| !offline_state(&s.state)).unwrap_or(false))
                    .filter_map(|l| match resolved(&st, l) {
                        Resolved::Local(p) => Some((if l.label.is_empty() { l.id.clone() } else { l.label.clone() }, p)),
                        Resolved::OtherOs => None,
                    })
                    .collect();
                (r.id.clone(), locs)
            })
            .collect();
        st.health_busy = true;
        st.task = "Health-Check läuft…".into();
        apply(ui, &mut st);
        (targets, st.local.git_path.clone(), st.local.health.clone())
    };
    let (weak, state) = (ui.as_weak(), state.clone());
    std::thread::spawn(move || {
        let git = CliGit::new(git_path.as_deref());
        let opts = FactOptions { check_remote: settings.enabled("remote"), secret_scan: settings.enabled("secrets"), large_file_min_bytes: settings.large_file_mb * 1024 * 1024, ..Default::default() };
        let results: Vec<(String, Vec<(String, Finding)>)> = targets
            .into_iter()
            .map(|(id, locs)| {
                let mut all = Vec::new();
                for (label, p) in locs {
                    let facts = git.collect_facts(&p, &opts);
                    all.extend(evaluate(&facts, &settings, now()).into_iter().map(|f| (label.clone(), f)));
                }
                (id, all)
            })
            .collect();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else { return };
            let mut st = state.lock().unwrap();
            let n = results.len();
            st.health.extend(results);
            st.health_busy = false;
            st.task = format!("Health-Check für {n} Repo(s) fertig");
            apply(&ui, &mut st);
        });
    });
}

/// Erreichbarkeit (mit Timeout, parallel) prüfen und Git-Snapshots im Hintergrund laden.
fn refresh(ui: &MainWindow, state: &Shared) {
    type Group = (String, Vec<(String, String, Option<PathBuf>)>);
    let (old, groups, cache_path): (HashMap<String, Snapshot>, Vec<Group>, Option<PathBuf>) = {
        let st = state.lock().unwrap();
        let groups = st
            .shared
            .repos
            .iter()
            .map(|r| {
                let locs = st
                    .shared
                    .locations_of(&r.id)
                    .map(|l| {
                        let p = match resolved(&st, l) {
                            Resolved::Local(p) => Some(p),
                            Resolved::OtherOs => None,
                        };
                        (l.id.clone(), if l.label.is_empty() { l.id.clone() } else { l.label.clone() }, p)
                    })
                    .collect();
                (r.id.clone(), locs)
            })
            .collect();
        (st.snaps.clone(), groups, st.cache_path.clone())
    };
    let (items, git_path, timeout, offline): (Vec<(String, PathBuf)>, Option<String>, u64, Vec<String>) = {
        let mut st = state.lock().unwrap();
        st.task = "Aktualisiere…".into();
        let mut items = Vec::new();
        let mut offline = Vec::new();
        for l in &st.shared.locations {
            match resolved(&st, l) {
                Resolved::Local(p) => items.push((l.id.clone(), p)),
                Resolved::OtherOs => offline.push(l.id.clone()),
            }
        }
        (items, st.local.git_path.clone(), st.local.reach_timeout_secs, offline)
    };
    ui.set_busy(true);
    let weak = ui.as_weak();
    let state = state.clone();
    std::thread::spawn(move || {
        let t = Duration::from_secs(timeout.max(1));
        let reach: Vec<Reachability> = std::thread::scope(|s| {
            let hs: Vec<_> = items.iter().map(|(_, p)| s.spawn(move || reach::check(p, t))).collect();
            hs.into_iter().map(|h| h.join().unwrap_or(Reachability::Unavailable)).collect()
        });
        let mut snaps: Vec<Snapshot> = Vec::new();
        let mut ok = Vec::new();
        for ((id, p), r) in items.into_iter().zip(reach) {
            match r {
                Reachability::Repo => ok.push((id, p)),
                Reachability::NotARepo => snaps.push(Snapshot::empty(&id, Status::NotARepo)),
                _ => snaps.push(Snapshot::empty(&id, Status::Unavailable)),
            }
        }
        let git = CliGit::new(git_path.as_deref());
        snaps.extend(snapshots_parallel(&git, &ok, 4));
        for id in offline {
            snaps.push(Snapshot::empty(&id, Status::Unavailable));
        }
        let n = snaps.len();
        let merged: HashMap<String, Snapshot> = snaps
            .into_iter()
            .map(|s| {
                let m = cache::merge(old.get(&s.location_id), s);
                (m.location_id.clone(), m)
            })
            .collect();
        let drift = compute_drift(&git, &groups, &merged);
        if let Some(p) = &cache_path {
            let all: Vec<Snapshot> = merged.values().cloned().collect();
            let _ = cache::save(p, &all);
        }
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else { return };
            let mut st = state.lock().unwrap();
            st.snaps = merged;
            st.drift = drift;
            // Falls ein Repo in repos.toml noch keine remote_url hat, aber Git origin kennt -> übernehmen
            let mut updated_remote = false;
            for idx in 0..st.shared.repos.len() {
                if st.shared.repos[idx].remote_url.is_empty() {
                    let rid = st.shared.repos[idx].id.clone();
                    let found_url = st.shared.locations_of(&rid).filter_map(|l| st.snaps.get(&l.id)).find_map(|s| s.remote_url.clone());
                    if let Some(u) = found_url {
                        st.shared.repos[idx].remote_url = u;
                        updated_remote = true;
                    }
                }
            }
            if updated_remote {
                save_shared(&mut st);
            }
            st.task = format!("{n} Standorte aktualisiert");
            apply(&ui, &mut st);
            ui.set_busy(false);
        });
    });
}

/// Führt eine Git-Aktion je Standort im Hintergrund aus und protokolliert sie im Task-Panel.
fn run_actions(ui: &MainWindow, state: &Shared, jobs: Vec<(String, PathBuf, String)>, kind: &str, msg: String) {
    if jobs.is_empty() {
        return;
    }
    let git_path = state.lock().unwrap().local.git_path.clone();
    let mut queued = Vec::new();
    {
        let mut st = state.lock().unwrap();
        for (loc_id, path, label) in jobs {
            let id = st.next_task;
            st.next_task += 1;
            st.tasks.push(TaskEntry { id, title: format!("{kind}: {label}"), state: 0, detail: "läuft…".into() });
            queued.push((id, loc_id, path));
        }
        if st.tasks.len() > 60 {
            let n = st.tasks.len() - 60;
            st.tasks.drain(..n);
        }
        apply(ui, &mut st);
    }
    let (weak, state, kind) = (ui.as_weak(), state.clone(), kind.to_string());
    std::thread::spawn(move || {
        let git = CliGit::new(git_path.as_deref());
        for (task_id, loc_id, path) in queued {
            let result: Result<String, String> = (|| {
                if kind == "pull" || kind == "push" {
                    let fresh = git.snapshot(&loc_id, &path).state;
                    if kind == "pull" { guard_pull(&fresh)? } else { guard_push(&fresh)? }
                }
                match kind.as_str() {
                    "fetch" => git.fetch(&path),
                    "pull" => git.pull(&path, PullMode::FfOnly),
                    "push" => git.push(&path),
                    "stash" => git.stash(&path),
                    "stash_pop" => git.stash_pop(&path),
                    "trust" => git.trust_directory(&path),
                    "take_ownership" => git.take_ownership(&path),
                    "commit" => git.stage_all(&path).and_then(|_| git.commit(&path, &msg)),
                    "commit_push" => git.stage_all(&path).and_then(|_| git.commit(&path, &msg)).and_then(|_| git.push(&path)),
                    "set_remote" => git.set_remote(&path, &msg),
                    "set_remote_push" => git.set_remote_and_push(&path, &msg),
                    "publish_gh_priv" => git.publish_github(&path, &msg, true),
                    "publish_gh_pub" => git.publish_github(&path, &msg, false),
                    "clean" => {
                        let freed = repoorganizer_core::clean::clean_repo_artifacts(&path).map_err(|e| e.to_string())?;
                        Ok(format!("{} freigegeben", repoorganizer_core::clean::format_bytes(freed)))
                    },
                    other => Ok(format!("unbekannte Aktion {other}")),
                }
                .map_err(|e| e.to_string())
            })();
            let snap = git.snapshot(&loc_id, &path);
            let (weak_ui, state_ui, res_ui, snap_ui) = (weak.clone(), state.clone(), result.clone(), snap.clone());
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = weak_ui.upgrade() else { return };
                let mut st = state_ui.lock().unwrap();
                if let Some(t) = st.tasks.iter_mut().find(|t| t.id == task_id) {
                    match &res_ui {
                        Ok(o) => {
                            t.state = 1;
                            t.detail = if o.is_empty() { "ok".into() } else { o.clone() };
                        }
                        Err(e) => {
                            t.state = 2;
                            t.detail = e.clone();
                        }
                    }
                }
                if let Some(remote_u) = snap_ui.remote_url.clone() {
                    if let Some(repo_id) = st.shared.locations.iter().find(|l| l.id == snap_ui.location_id).map(|l| l.repo_id.clone()) {
                        if let Some(r) = st.shared.repos.iter_mut().find(|r| r.id == repo_id) {
                            if r.remote_url != remote_u {
                                r.remote_url = remote_u;
                                save_shared(&mut st);
                            }
                        }
                    }
                }
                st.snaps.insert(snap_ui.location_id.clone(), snap_ui);
                apply(&ui, &mut st);
            });

            // Wenn Push oder Commit+Push erfolgreich war: automatisch erreichbare Alternativpfade desselben Repos aktualisieren
            if (kind == "push" || kind == "commit_push") && result.is_ok() {
                let alt_targets: Vec<(String, PathBuf, String)> = {
                    let st = state.lock().unwrap();
                    let repo_id = st.shared.locations.iter().find(|l| l.id == loc_id).map(|l| l.repo_id.clone());
                    if let Some(rid) = repo_id {
                        st.shared
                            .locations_of(&rid)
                            .filter(|l| l.id != loc_id)
                            .filter_map(|l| match resolved(&st, l) {
                                Resolved::Local(p) if p.is_dir() => {
                                    let label = if l.label.is_empty() { l.id.clone() } else { l.label.clone() };
                                    Some((l.id.clone(), p, label))
                                }
                                _ => None,
                            })
                            .collect()
                    } else {
                        Vec::new()
                    }
                };

                for (alt_id, alt_path, alt_label) in alt_targets {
                    let pre_snap = git.snapshot(&alt_id, &alt_path);
                    let dirty = pre_snap.dirty_files + pre_snap.untracked;

                    let alt_task_id = {
                        let mut st = state.lock().unwrap();
                        let tid = st.next_task;
                        st.next_task += 1;
                        st.tasks.push(TaskEntry {
                            id: tid,
                            title: format!("Auto-Sync: {alt_label}"),
                            state: 0,
                            detail: "prüfe & aktualisiere Alternativpfad…".into(),
                        });
                        if let Some(ui) = weak.upgrade() {
                            apply(&ui, &mut st);
                        }
                        tid
                    };

                    // Sicherheitscheck: Niemals uncommittete Änderungen des Alternativpfads antasten!
                    if dirty > 0 {
                        let (weak_alt, state_alt) = (weak.clone(), state.clone());
                        let _ = slint::invoke_from_event_loop(move || {
                            let Some(ui) = weak_alt.upgrade() else { return };
                            let mut st = state_alt.lock().unwrap();
                            if let Some(t) = st.tasks.iter_mut().find(|t| t.id == alt_task_id) {
                                t.state = 2;
                                t.detail = format!("Sicherheitsstopp: {dirty} uncommittete Datei(en) vorhanden – kein Überschreiben");
                            }
                            st.snaps.insert(alt_id, pre_snap);
                            apply(&ui, &mut st);
                        });
                        continue;
                    }

                    // Sicherheitscheck: Niemals pullen, wenn der Alternativpfad eigene ungesendete Commits hat (Divergenzschutz)!
                    if pre_snap.ahead > 0 {
                        let (weak_alt, state_alt) = (weak.clone(), state.clone());
                        let _ = slint::invoke_from_event_loop(move || {
                            let Some(ui) = weak_alt.upgrade() else { return };
                            let mut st = state_alt.lock().unwrap();
                            if let Some(t) = st.tasks.iter_mut().find(|t| t.id == alt_task_id) {
                                t.state = 2;
                                t.detail = "Sicherheitsstopp: Eigene ungesendete Commits vorhanden (Divergenzschutz)".into();
                            }
                            st.snaps.insert(alt_id, pre_snap);
                            apply(&ui, &mut st);
                        });
                        continue;
                    }

                    let pull_res = git.fetch(&alt_path).and_then(|_| git.pull(&alt_path, PullMode::FfOnly));
                    let alt_snap = git.snapshot(&alt_id, &alt_path);
                    let (weak_alt, state_alt) = (weak.clone(), state.clone());
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(ui) = weak_alt.upgrade() else { return };
                        let mut st = state_alt.lock().unwrap();
                        if let Some(t) = st.tasks.iter_mut().find(|t| t.id == alt_task_id) {
                            match &pull_res {
                                Ok(out) => {
                                    t.state = 1;
                                    t.detail = if out.trim().is_empty() { "Bereits aktuell".into() } else { out.trim().to_string() };
                                }
                                Err(e) => {
                                    t.state = 2;
                                    t.detail = format!("Hinweis: {e}");
                                }
                            }
                        }
                        st.snaps.insert(alt_id, alt_snap);
                        apply(&ui, &mut st);
                    });
                }
            }
        }
    });
}

/// Jobs für einen Standort oder (bei `repo_id` leer) für alle erreichbaren Standorte.
fn jobs_for(st: &State, loc_id: Option<&str>, repo_id: Option<&str>) -> Vec<(String, PathBuf, String)> {
    st.shared
        .locations
        .iter()
        .filter(|l| loc_id.map(|i| l.id == i).unwrap_or(true))
        .filter(|l| repo_id.map(|r| r.is_empty() || l.repo_id == r).unwrap_or(true))
        .filter_map(|l| {
            let repo = st.shared.repos.iter().find(|r| r.id == l.repo_id)?;
            match resolved(st, l) {
                Resolved::Local(p) if st.snaps.get(&l.id).map(|s| !matches!(s.state, Status::Unavailable | Status::NotARepo)).unwrap_or(true) => {
                    Some((l.id.clone(), p, format!("{} / {}", repo.name, if l.label.is_empty() { &l.id } else { &l.label })))
                }
                _ => None,
            }
        })
        .collect()
}

/// Scan im Hintergrund starten; Fortschritt gedrosselt melden.
fn start_scan(ui: &MainWindow, state: &Shared, roots: &str, depth: &str) {
    let roots: Vec<PathBuf> = roots.split(|c| c == ';' || c == '\n').map(str::trim).filter(|s| !s.is_empty()).map(PathBuf::from).collect();
    if roots.is_empty() {
        return;
    }
    let (git_path, timeout, cancel) = {
        let mut st = state.lock().unwrap();
        if st.scanning {
            return;
        }
        st.scanning = true;
        st.scan_items.clear();
        st.scan_progress = "Starte Scan…".into();
        st.cancel.store(false, Ordering::Relaxed);
        apply(ui, &mut st);
        (st.local.git_path.clone(), st.local.reach_timeout_secs, st.cancel.clone())
    };
    let opts = ScanOptions { roots, max_depth: depth.trim().parse().unwrap_or(4), ..Default::default() };
    let (weak, state) = (ui.as_weak(), state.clone());
    std::thread::spawn(move || {
        let t = Duration::from_secs(timeout.max(1));
        let mut skipped = Vec::new();
        let roots: Vec<PathBuf> = opts
            .roots
            .iter()
            .filter(|r| match reach::check(r, t) {
                Reachability::NotARepo | Reachability::Repo => true,
                _ => {
                    skipped.push(r.display().to_string());
                    false
                }
            })
            .cloned()
            .collect();
        let opts = ScanOptions { roots, ..opts };
        let git = CliGit::new(git_path.as_deref());
        let (w2, s2) = (weak.clone(), state.clone());
        let found = scan(&opts, &git, &cancel, &mut |n, dir| {
            if n % 25 == 0 {
                let (w, s, msg) = (w2.clone(), s2.clone(), format!("{n} Ordner geprüft – {}", dir.display()));
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = w.upgrade() {
                        let mut st = s.lock().unwrap();
                        st.scan_progress = msg;
                        apply(&ui, &mut st);
                    }
                });
            }
        });
        let cancelled = cancel.load(Ordering::Relaxed);
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else { return };
            let mut st = state.lock().unwrap();
            let os = st.os;
            let items: Vec<ScanItem> = found
                .into_iter()
                .map(|f| {
                    let class = classify(&st.shared, os, &f.path, f.remote.as_deref());
                    let checked = !matches!(class, ScanClass::Known { .. });
                    ScanItem { path: f.path.display().to_string(), remote: f.remote, class, checked }
                })
                .collect();
            st.scan_progress = format!(
                "{} {} Repos gefunden{}",
                if cancelled { "Abgebrochen:" } else { "Fertig:" },
                items.len(),
                if skipped.is_empty() { String::new() } else { format!(" – nicht erreichbar: {}", skipped.join(", ")) }
            );
            st.scan_items = items;
            st.scanning = false;
            apply(&ui, &mut st);
        });
    });
}

/// Klont im Hintergrund und trägt den neuen Standort danach ein (`repo_id` None: automatisch zuordnen).
fn run_clone(ui: &MainWindow, state: &Shared, url: String, dest: String, repo_id: Option<String>) {
    let (git_path, id) = {
        let mut st = state.lock().unwrap();
        let id = st.next_task;
        st.next_task += 1;
        st.tasks.push(TaskEntry { id, title: format!("clone: {url}"), state: 0, detail: "läuft…".into() });
        apply(ui, &mut st);
        (st.local.git_path.clone(), id)
    };
    let (weak, state) = (ui.as_weak(), state.clone());
    std::thread::spawn(move || {
        let git = CliGit::new(git_path.as_deref());
        let res = git.clone_repo(&url, std::path::Path::new(&dest)).map_err(|e| e.to_string());
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else { return };
            {
                let mut st = state.lock().unwrap();
                let ok = res.is_ok();
                if let Some(t) = st.tasks.iter_mut().find(|t| t.id == id) {
                    t.state = if ok { 1 } else { 2 };
                    t.detail = match &res {
                        Ok(o) => o.lines().last().unwrap_or("ok").to_string(),
                        Err(e) => e.clone(),
                    };
                }
                if ok {
                    let os = st.os;
                    match &repo_id {
                        Some(r) => link_location(&mut st.shared, os, r, &dest, ""),
                        None => {
                            let class = classify(&st.shared, os, std::path::Path::new(&dest), Some(&url));
                            let item = Adopt { path: dest.clone(), remote: Some(url.clone()), class };
                            adopt(&mut st.shared, os, &[item]);
                        }
                    }
                    save_shared(&mut st);
                }
                apply(&ui, &mut st);
            }
            if res.is_ok() {
                refresh(&ui, &state);
            }
        });
    });
}

fn run_smart_sync(
    ui: &MainWindow,
    state: &Shared,
    do_pull: bool,
    do_commit_push: bool,
    commit_msg: String,
    do_clone_missing: bool,
    do_auto_sync: bool,
) {
    let (git_path, os) = {
        let st = state.lock().unwrap();
        (st.local.git_path.clone(), st.os)
    };

    let eff_msg = if commit_msg.trim().is_empty() {
        "Smart-Sync: Änderungen gesichert".to_string()
    } else {
        commit_msg.trim().to_string()
    };

    let (weak, state_clone) = (ui.as_weak(), state.clone());
    std::thread::spawn(move || {
        let git = CliGit::new(git_path.as_deref());

        // 1. Commit & Push
        if do_commit_push {
            let targets: Vec<(String, PathBuf, String, bool, bool)> = {
                let st = state_clone.lock().unwrap();
                let mut list = Vec::new();
                for r in &st.shared.repos {
                    for l in st.shared.locations_of(&r.id) {
                        if let Resolved::Local(p) = resolved(&st, l) {
                            if p.is_dir() {
                                let snap = st.snaps.get(&l.id);
                                let dirty = snap.map(|s| s.dirty_files + s.untracked > 0).unwrap_or(false);
                                let ahead = snap.map(|s| s.ahead > 0).unwrap_or(false);
                                if dirty || ahead {
                                    let label = if l.label.is_empty() { r.name.clone() } else { format!("{} ({})", r.name, l.label) };
                                    list.push((l.id.clone(), p, label, dirty, ahead));
                                }
                            }
                        }
                    }
                }
                list
            };

            for (loc_id, path, label, dirty, _ahead) in targets {
                let kind = if dirty { "commit & push" } else { "push" };
                let task_id = {
                    let mut st = state_clone.lock().unwrap();
                    let id = st.next_task;
                    st.next_task += 1;
                    st.tasks.push(TaskEntry {
                        id,
                        title: format!("{kind}: {label}"),
                        state: 0,
                        detail: "wird ausgeführt…".into(),
                    });
                    if let Some(ui) = weak.upgrade() {
                        apply(&ui, &mut st);
                    }
                    id
                };

                let res = (|| -> Result<String, String> {
                    if dirty {
                        git.stage_all(&path).map_err(|e| e.to_string())?;
                        git.commit(&path, &eff_msg).map_err(|e| e.to_string())?;
                    }
                    git.push(&path).map_err(|e| e.to_string())
                })();

                let res_ok = res.is_ok();
                let snap = git.snapshot(&loc_id, &path);
                {
                    let (w, s, r_val, s_val, l_id) = (weak.clone(), state_clone.clone(), res.clone(), snap.clone(), loc_id.clone());
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(ui) = w.upgrade() else { return };
                        let mut st = s.lock().unwrap();
                        if let Some(t) = st.tasks.iter_mut().find(|t| t.id == task_id) {
                            match &r_val {
                                Ok(out) => {
                                    t.state = 1;
                                    t.detail = if out.trim().is_empty() { "ok".into() } else { out.trim().to_string() };
                                }
                                Err(e) => {
                                    t.state = 2;
                                    t.detail = e.clone();
                                }
                            }
                        }
                        st.snaps.insert(l_id, s_val);
                        apply(&ui, &mut st);
                    });
                }

                // Push erfolgreich & do_auto_sync aktiv: saubere Alternativpfade automatisch angleichen
                if res_ok && do_auto_sync {
                    let alt_targets: Vec<(String, PathBuf, String)> = {
                        let st = state_clone.lock().unwrap();
                        let repo_id = st.shared.locations.iter().find(|l| l.id == loc_id).map(|l| l.repo_id.clone());
                        if let Some(rid) = repo_id {
                            st.shared
                                .locations_of(&rid)
                                .filter(|l| l.id != loc_id)
                                .filter_map(|l| match resolved(&st, l) {
                                    Resolved::Local(p) if p.is_dir() => {
                                        let label = if l.label.is_empty() { l.id.clone() } else { l.label.clone() };
                                        Some((l.id.clone(), p, label))
                                    }
                                    _ => None,
                                })
                                .collect()
                        } else {
                            Vec::new()
                        }
                    };

                    for (alt_id, alt_path, alt_label) in alt_targets {
                        let pre_snap = git.snapshot(&alt_id, &alt_path);
                        let alt_dirty = pre_snap.dirty_files + pre_snap.untracked;
                        let alt_task_id = {
                            let mut st = state_clone.lock().unwrap();
                            let tid = st.next_task;
                            st.next_task += 1;
                            st.tasks.push(TaskEntry {
                                id: tid,
                                title: format!("Auto-Sync: {alt_label}"),
                                state: 0,
                                detail: "prüfe & aktualisiere Alternativpfad…".into(),
                            });
                            if let Some(ui) = weak.upgrade() {
                                apply(&ui, &mut st);
                            }
                            tid
                        };

                        if alt_dirty > 0 {
                            let (w, s, p_snap, a_id) = (weak.clone(), state_clone.clone(), pre_snap.clone(), alt_id.clone());
                            let _ = slint::invoke_from_event_loop(move || {
                                let Some(ui) = w.upgrade() else { return };
                                let mut st = s.lock().unwrap();
                                if let Some(t) = st.tasks.iter_mut().find(|t| t.id == alt_task_id) {
                                    t.state = 2;
                                    t.detail = format!("Sicherheitsstopp: {alt_dirty} uncommittete Datei(en) vorhanden – kein Überschreiben");
                                }
                                st.snaps.insert(a_id, p_snap);
                                apply(&ui, &mut st);
                            });
                            continue;
                        }

                        if pre_snap.ahead > 0 {
                            let (w, s, p_snap, a_id) = (weak.clone(), state_clone.clone(), pre_snap.clone(), alt_id.clone());
                            let _ = slint::invoke_from_event_loop(move || {
                                let Some(ui) = w.upgrade() else { return };
                                let mut st = s.lock().unwrap();
                                if let Some(t) = st.tasks.iter_mut().find(|t| t.id == alt_task_id) {
                                    t.state = 2;
                                    t.detail = "Sicherheitsstopp: Eigene ungesendete Commits vorhanden (Divergenzschutz)".into();
                                }
                                st.snaps.insert(a_id, p_snap);
                                apply(&ui, &mut st);
                            });
                            continue;
                        }

                        let pull_res = git.fetch(&alt_path).and_then(|_| git.pull(&alt_path, PullMode::FfOnly));
                        let alt_snap = git.snapshot(&alt_id, &alt_path);
                        let (w, s, p_res, a_snap, a_id) = (weak.clone(), state_clone.clone(), pull_res, alt_snap, alt_id);
                        let _ = slint::invoke_from_event_loop(move || {
                            let Some(ui) = w.upgrade() else { return };
                            let mut st = s.lock().unwrap();
                            if let Some(t) = st.tasks.iter_mut().find(|t| t.id == alt_task_id) {
                                match &p_res {
                                    Ok(out) => {
                                        t.state = 1;
                                        t.detail = if out.trim().is_empty() { "Bereits aktuell".into() } else { out.trim().to_string() };
                                    }
                                    Err(e) => {
                                        t.state = 2;
                                        t.detail = format!("Hinweis: {e}");
                                    }
                                }
                            }
                            st.snaps.insert(a_id, a_snap);
                            apply(&ui, &mut st);
                        });
                    }
                }
            }
        }

        // 2. Pull
        if do_pull {
            let pull_targets: Vec<(String, PathBuf, String)> = {
                let st = state_clone.lock().unwrap();
                let mut list = Vec::new();
                for r in &st.shared.repos {
                    for l in st.shared.locations_of(&r.id) {
                        if let Resolved::Local(p) = resolved(&st, l) {
                            if p.is_dir() {
                                if let Some(snap) = st.snaps.get(&l.id) {
                                    let dirty = snap.dirty_files + snap.untracked;
                                    if snap.behind > 0 && dirty == 0 && snap.ahead == 0 {
                                        let label = if l.label.is_empty() { r.name.clone() } else { format!("{} ({})", r.name, l.label) };
                                        list.push((l.id.clone(), p, label));
                                    }
                                }
                            }
                        }
                    }
                }
                list
            };

            for (loc_id, path, label) in pull_targets {
                let task_id = {
                    let mut st = state_clone.lock().unwrap();
                    let id = st.next_task;
                    st.next_task += 1;
                    st.tasks.push(TaskEntry {
                        id,
                        title: format!("pull: {label}"),
                        state: 0,
                        detail: "hole Updates…".into(),
                    });
                    if let Some(ui) = weak.upgrade() {
                        apply(&ui, &mut st);
                    }
                    id
                };

                let res = git.fetch(&path).and_then(|_| git.pull(&path, PullMode::FfOnly));
                let snap = git.snapshot(&loc_id, &path);
                let (w, s, r_val, s_val, l_id) = (weak.clone(), state_clone.clone(), res, snap, loc_id);
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = w.upgrade() else { return };
                    let mut st = s.lock().unwrap();
                    if let Some(t) = st.tasks.iter_mut().find(|t| t.id == task_id) {
                        match &r_val {
                            Ok(out) => {
                                t.state = 1;
                                t.detail = if out.trim().is_empty() { "Bereits aktuell".into() } else { out.trim().to_string() };
                            }
                            Err(e) => {
                                t.state = 2;
                                t.detail = format!("Fehler: {e}");
                            }
                        }
                    }
                    st.snaps.insert(l_id, s_val);
                    apply(&ui, &mut st);
                });
            }
        }

        // 3. Clone Missing in configured Base Paths
        if do_clone_missing {
            struct MissingClone {
                repo_id: String,
                repo_name: String,
                target_path: String,
                source: String,
            }

            let missing_clones: Vec<MissingClone> = {
                let st = state_clone.lock().unwrap();
                let bp = &st.shared.base_paths;
                let mut list = Vec::new();

                let mut active_bps: Vec<&str> = Vec::new();
                if os != Os::Windows {
                    if let Some(su) = &bp.shared_unix {
                        let t = su.trim();
                        if !t.is_empty() && std::path::Path::new(t).exists() {
                            active_bps.push(t);
                        }
                    }
                    if os == Os::Linux {
                        if let Some(ll) = &bp.local_linux {
                            let t = ll.trim();
                            if !t.is_empty() && std::path::Path::new(t).exists() {
                                active_bps.push(t);
                            }
                        }
                    }
                } else {
                    if let Some(sw) = &bp.shared_windows {
                        let t = sw.trim();
                        if !t.is_empty() && std::path::Path::new(t).exists() {
                            active_bps.push(t);
                        }
                    }
                    if let Some(lw) = &bp.local_windows {
                        let t = lw.trim();
                        if !t.is_empty() && std::path::Path::new(t).exists() {
                            active_bps.push(t);
                        }
                    }
                }

                for r in &st.shared.repos {
                    let clone_source = if !r.remote_url.trim().is_empty() {
                        Some(r.remote_url.trim().to_string())
                    } else {
                        st.shared.locations_of(&r.id).find_map(|l| match resolved(&st, l) {
                            Resolved::Local(p) if p.is_dir() => Some(p.display().to_string()),
                            _ => None,
                        })
                    };

                    let Some(src) = clone_source else { continue };

                    for base in &active_bps {
                        let sep = if os == Os::Windows { "\\" } else { "/" };
                        let target_path = format!("{base}{sep}{}", r.name);
                        let exists = st.shared.locations_of(&r.id).any(|l| match resolved(&st, l) {
                            Resolved::Local(p) => p == std::path::Path::new(&target_path),
                            _ => {
                                if os == Os::Windows {
                                    l.paths.windows.as_deref() == Some(&target_path)
                                } else {
                                    l.paths.linux.as_deref() == Some(&target_path)
                                }
                            }
                        });
                        if !exists {
                            list.push(MissingClone {
                                repo_id: r.id.clone(),
                                repo_name: r.name.clone(),
                                target_path,
                                source: src.clone(),
                            });
                        }
                    }
                }
                list
            };

            let mut any_linked = false;
            for item in missing_clones {
                let target_p = std::path::Path::new(&item.target_path);
                // Wenn Verzeichnis bereits existiert und ein Git Repo ist: direkt verlinken
                if target_p.join(".git").exists() || target_p.is_dir() {
                    any_linked = true;
                    let (w, s, rid, tp) = (weak.clone(), state_clone.clone(), item.repo_id.clone(), item.target_path.clone());
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(ui) = w.upgrade() else { return };
                        let mut st = s.lock().unwrap();
                        link_location(&mut st.shared, os, &rid, &tp, "");
                        save_shared(&mut st);
                        apply(&ui, &mut st);
                    });
                    continue;
                }

                let task_id = {
                    let mut st = state_clone.lock().unwrap();
                    let id = st.next_task;
                    st.next_task += 1;
                    st.tasks.push(TaskEntry {
                        id,
                        title: format!("clone: {} -> {}", item.repo_name, item.target_path),
                        state: 0,
                        detail: "klone in Basispfad…".into(),
                    });
                    if let Some(ui) = weak.upgrade() {
                        apply(&ui, &mut st);
                    }
                    id
                };

                let res = git.clone_repo(&item.source, target_p).map_err(|e| e.to_string());
                let res_ok = res.is_ok();
                if res_ok {
                    any_linked = true;
                }
                let (w, s, rid, tp, r_val) = (weak.clone(), state_clone.clone(), item.repo_id.clone(), item.target_path.clone(), res);
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = w.upgrade() else { return };
                    let mut st = s.lock().unwrap();
                    if let Some(t) = st.tasks.iter_mut().find(|t| t.id == task_id) {
                        match &r_val {
                            Ok(out) => {
                                t.state = 1;
                                t.detail = if out.trim().is_empty() { "ok".into() } else { out.trim().to_string() };
                            }
                            Err(e) => {
                                t.state = 2;
                                t.detail = e.clone();
                            }
                        }
                    }
                    if res_ok {
                        link_location(&mut st.shared, os, &rid, &tp, "");
                        save_shared(&mut st);
                    }
                    apply(&ui, &mut st);
                });
            }

            if any_linked && do_commit_push {
                let repo_org_paths: Vec<(String, PathBuf)> = {
                    let st = state_clone.lock().unwrap();
                    st.shared.locations.iter()
                        .filter(|l| l.repo_id == "repoorganizer")
                        .filter_map(|l| match resolved(&st, l) {
                            Resolved::Local(p) if p.is_dir() => Some((l.id.clone(), p)),
                            _ => None,
                        })
                        .collect()
                };
                for (rid, rp) in repo_org_paths {
                    let s = git.snapshot(&rid, &rp);
                    if s.dirty_files + s.untracked > 0 {
                        let _ = git.stage_all(&rp);
                        let _ = git.commit(&rp, "Smart-Sync: Basispfade in repos.toml aktualisiert");
                        let _ = git.push(&rp);
                    }
                }
            }
        }

        // Abschließender Refresh
        let (w, s) = (weak.clone(), state_clone.clone());
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = w.upgrade() {
                refresh(&ui, &s);
            }
        });
    });
}

fn suggest_clone_dest(st: &State, repo_id: &str) -> String {
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
            if std::path::Path::new(shared_win).exists() {
                return format!(r"{shared_win}\{repo_name}");
            }
        }
    } else if let Some(shared_unix) = &bp.shared_unix {
        if std::path::Path::new(shared_unix).exists() {
            return format!("{shared_unix}/{repo_name}");
        }
    }

    // 3. Lokaler Basispfad für aktuelles Betriebssystem laut Hauptkonfig
    if st.os == Os::Windows {
        if let Some(local_win) = &bp.local_windows {
            if std::path::Path::new(local_win).exists() {
                return format!(r"{local_win}\{repo_name}");
            }
        }
    } else if let Some(local_linux) = &bp.local_linux {
        if std::path::Path::new(local_linux).exists() {
            return format!("{local_linux}/{repo_name}");
        }
    }

    // 4. Fallbacks (Hardcoded Defaults)
    if std::path::Path::new("/r/git").exists() {
        return format!("/r/git/{repo_name}");
    }
    if st.os == Os::Windows && std::path::Path::new(r"R:\git").exists() {
        return format!(r"R:\git\{repo_name}");
    }
    if let Ok(home) = std::env::var("HOME") {
        let projekte = format!("{home}/Projekte");
        if std::path::Path::new(&projekte).exists() {
            return format!("{projekte}/{repo_name}");
        }
    }

    format!("./{repo_name}")
}

fn name_from_path(path: &str) -> String {
    let trimmed = path.trim().trim_end_matches(&['/', '\\'][..]);
    let seg = trimmed.rsplit(&['/', '\\'][..]).next().unwrap_or(trimmed);
    seg.strip_suffix(".git").unwrap_or(seg).trim().to_string()
}

fn add_repo(st: &mut State, name: &str, path: &str, remote_url: &str) -> String {
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
            CliGit::new(st.local.git_path.as_deref()).remote_origin(std::path::Path::new(p))
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

fn file_mtime(p: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(p).ok()?.modified().ok()
}

fn save_shared(st: &mut State) {
    match st.shared.save(&st.shared_path) {
        Ok(()) => {
            st.error.clear();
            st.shared_mtime = file_mtime(&st.shared_path);
        }
        Err(e) => st.error = e.to_string(),
    }
}

fn save_local(st: &mut State) {
    if let Err(e) = st.local.save(&st.local_path) {
        st.error = e.to_string();
    }
}

fn open_external(path: &std::path::Path, tool: &str) {
    use std::process::Command;
    let p = path.to_string_lossy().into_owned();
    let _ = match tool {
        "explorer" => {
            if cfg!(windows) {
                Command::new("explorer").arg(&p).spawn()
            } else if cfg!(target_os = "macos") {
                Command::new("open").arg(&p).spawn()
            } else {
                Command::new("xdg-open").arg(&p).spawn()
            }
        }
        "terminal" => {
            if cfg!(windows) {
                Command::new("cmd").args(["/C", "start", "powershell", "-NoExit", "-Command", &format!("Set-Location -LiteralPath '{p}'")]).spawn()
            } else if cfg!(target_os = "macos") {
                Command::new("open").args(["-a", "Terminal", &p]).spawn()
            } else {
                Command::new("x-terminal-emulator").current_dir(path).spawn()
            }
        }
        "code" => {
            if cfg!(windows) {
                Command::new("cmd").args(["/C", "code", &p]).spawn()
            } else {
                Command::new("code").arg(&p).spawn()
            }
        }
        _ => return,
    };
}

fn open_url(url: &str) {
    use std::process::Command;
    let _ = if cfg!(windows) {
        Command::new("cmd").args(["/C", "start", "", url]).spawn()
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg(url).spawn()
    } else {
        Command::new("xdg-open").arg(url).spawn()
    };
}

#[cfg(target_os = "linux")]
fn ensure_linux_dialog_ready() {
    extern "C" {
        fn dlopen(filename: *const std::os::raw::c_char, flag: std::os::raw::c_int) -> *mut std::ffi::c_void;
    }
    const RTLD_NOW: std::os::raw::c_int = 2;
    const RTLD_GLOBAL: std::os::raw::c_int = 256;

    unsafe {
        let name = std::ffi::CString::new("libdbus-1.so.3").unwrap();
        let handle = dlopen(name.as_ptr(), RTLD_NOW | RTLD_GLOBAL);
        if !handle.is_null() {
            return;
        }
    }

    if let Ok(entries) = std::fs::read_dir("/nix/store") {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.contains("dbus") && name.ends_with("-lib") {
                    let so = path.join("lib").join("libdbus-1.so.3");
                    if so.exists() {
                        if let Ok(c_so) = std::ffi::CString::new(so.to_string_lossy().as_bytes()) {
                            unsafe {
                                let handle = dlopen(c_so.as_ptr(), RTLD_NOW | RTLD_GLOBAL);
                                if !handle.is_null() {
                                    return;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn main() -> Result<(), slint::PlatformError> {
    #[cfg(target_os = "linux")]
    ensure_linux_dialog_ready();

    if std::env::args().nth(1).as_deref() == Some("status") {
        let lp = LocalConfig::default_path().unwrap_or_else(|| PathBuf::from("local.toml"));
        let local = LocalConfig::load(&lp).unwrap_or_default();
        let sp = SharedConfig::program_dir_path();
        let shared = SharedConfig::load(&sp).unwrap_or_default();
        let git = CliGit::new(local.git_path.as_deref());
        let os = Os::current();
        for r in &shared.repos {
            println!("=== {} ({}) ===", r.name, r.id);
            for l in shared.locations_of(&r.id) {
                match resolve(l, os, &shared.mappings) {
                    Resolved::Local(p) => {
                        let s = git.snapshot(&l.id, &p);
                        let has_remote = s.remote_url.as_ref().map(|u| !u.is_empty()).unwrap_or(false);
                        let (rel_str, _) = present::remote_relation_badge(s.ahead, s.behind, has_remote);
                        let tree_str = present::working_tree_text(s.dirty_files + s.untracked, s.stash_count);
                        println!("  [{}] {} -> {} | {}", l.id, p.display(), rel_str, tree_str);
                    }
                    Resolved::OtherOs => println!("  [{}] (auf anderem System)", l.id),
                }
            }
        }
        return Ok(());
    }
    let local_path = LocalConfig::default_path().unwrap_or_else(|| PathBuf::from("local.toml"));
    let mut error = String::new();
    let mut local = LocalConfig::load(&local_path).unwrap_or_else(|e| {
        error = e.to_string();
        LocalConfig::default()
    });
    let shared_path = SharedConfig::program_dir_path();
    let shared = if shared_path.exists() {
        SharedConfig::load(&shared_path).unwrap_or_else(|e| {
            error = e.to_string();
            SharedConfig::default()
        })
    } else {
        // Einmalige Migration aus dem alten AppData-Pfad, falls vorhanden
        let legacy = local.shared_path(&local_path);
        if legacy.exists() {
            let cfg = SharedConfig::load(&legacy).unwrap_or_default();
            let _ = cfg.save(&shared_path);
            cfg
        } else {
            SharedConfig::default()
        }
    };

    // Git-User automatisch erkennen, falls noch nicht in local.toml gesetzt
    if local.git_user.trim().is_empty() {
        let detected = shared
            .repos
            .iter()
            .find_map(|r| detect_user_from_url(&r.remote_url))
            .or_else(|| CliGit::new(local.git_path.as_deref()).global_git_user());
        if let Some(u) = detected {
            local.git_user = u;
            let _ = local.save(&local_path);
        }
    }

    let ui = MainWindow::new()?;
    ui.window().set_size(slint::LogicalSize::new(local.window_width as f32, local.window_height as f32));
    let scan_win = ScanWindow::new()?;
    scan_win.window().set_size(slint::LogicalSize::new(920.0, 640.0));
    let settings_win = SettingsWindow::new()?;
    settings_win.window().set_size(slint::LogicalSize::new(980.0, 700.0));

    let state: Shared = Arc::new(Mutex::new(State {
        local,
        local_path,
        stats: cache::activity_path().map(|p| cache::load_activity(&p)).unwrap_or_default(),
        stats_busy: false,
        stats_path: cache::activity_path(),
        drift: HashMap::new(),
        health: HashMap::new(),
        health_busy: false,
        view: String::new(),
        cache_path: cache::default_path(),
        scan_items: Vec::new(),
        scanning: false,
        scan_progress: String::new(),
        cancel: Arc::new(AtomicBool::new(false)),
        tasks: Vec::new(),
        next_task: 1,
        shared,
        shared_mtime: file_mtime(&shared_path),
        shared_path: shared_path.clone(),
        snaps: cache::default_path().map(|p| cache::load(&p)).unwrap_or_default().into_iter().map(|s| (s.location_id.clone(), s)).collect(),
        os: Os::current(),
        filter: String::new(),
        selected: String::new(),
        error,
        task: format!("Config: {}", shared_path.display()),
        scan_win: Some(scan_win.as_weak()),
        settings_win: Some(settings_win.as_weak()),
    }));

    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_refresh(move || {
            if let Some(ui) = weak.upgrade() {
                refresh(&ui, &state);
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_select(move |id: SharedString| {
            if let Some(ui) = weak.upgrade() {
                {
                    let mut st = state.lock().unwrap();
                    st.selected = id.to_string();
                    apply(&ui, &mut st);
                }
                if ui.get_detail_tab() == 3 {
                    load_stats(&ui, &state, &id, false);
                }
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_filter_changed(move |t: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.lock().unwrap();
                st.filter = t.to_string();
                apply(&ui, &mut st);
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_column_filter_changed(move || {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.lock().unwrap();
                apply(&ui, &mut st);
            }
        });
    }
    {
        ui.on_derive_name(move |path: SharedString| name_from_path(&path).into());
    }
    {
        let state = state.clone();
        ui.on_derive_url(move |name: SharedString, path: SharedString| {
            let st = state.lock().unwrap();
            let p = path.trim();
            if !p.is_empty() {
                let git = CliGit::new(st.local.git_path.as_deref());
                if let Some(existing) = git.remote_origin(std::path::Path::new(p)) {
                    return existing.into();
                }
            }
            let eff_name = if !name.trim().is_empty() {
                name.trim().to_string()
            } else {
                name_from_path(p)
            };
            st.local.derive_remote_url(&eff_name).into()
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_add_repo(move |name: SharedString, path: SharedString, url: SharedString| {
            if let Some(ui) = weak.upgrade() {
                {
                    let mut st = state.lock().unwrap();
                    add_repo(&mut st, &name, &path, &url);
                    apply(&ui, &mut st);
                }
                refresh(&ui, &state);
            }
        });
    }

    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_location_action(move |loc: SharedString, kind: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let jobs = jobs_for(&state.lock().unwrap(), Some(&loc), None);
                run_actions(&ui, &state, jobs, &kind, String::new());
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_clean_repo_builds(move |repo_id: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let jobs = jobs_for(&state.lock().unwrap(), None, Some(&repo_id));
                run_actions(&ui, &state, jobs, "clean", String::new());
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_clean_all_builds(move || {
            if let Some(ui) = weak.upgrade() {
                let jobs = jobs_for(&state.lock().unwrap(), None, None);
                run_actions(&ui, &state, jobs, "clean", String::new());
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_commit(move |loc: SharedString, msg: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let jobs = jobs_for(&state.lock().unwrap(), Some(&loc), None);
                run_actions(&ui, &state, jobs, "commit", msg.to_string());
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_commit_and_push(move |loc: SharedString, msg: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let jobs = jobs_for(&state.lock().unwrap(), Some(&loc), None);
                run_actions(&ui, &state, jobs, "commit_push", msg.to_string());
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_do_quick_commit_push(move |msg: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let target_loc = ui.get_quick_commit_target_id().to_string();
                if !target_loc.is_empty() {
                    let jobs = jobs_for(&state.lock().unwrap(), Some(&target_loc), None);
                    run_actions(&ui, &state, jobs, "commit_push", msg.to_string());
                }
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_do_quick_commit(move |msg: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let target_loc = ui.get_quick_commit_target_id().to_string();
                if !target_loc.is_empty() {
                    let jobs = jobs_for(&state.lock().unwrap(), Some(&target_loc), None);
                    run_actions(&ui, &state, jobs, "commit", msg.to_string());
                }
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_run_smart_sync(move |do_pull, do_commit_push, msg, do_clone_missing, do_auto_sync| {
            if let Some(ui) = weak.upgrade() {
                run_smart_sync(&ui, &state, do_pull, do_commit_push, msg.to_string(), do_clone_missing, do_auto_sync);
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_publish_remote(move |repo: SharedString, url: SharedString, do_push: bool| {
            if let Some(ui) = weak.upgrade() {
                let u = url.trim().to_string();
                if u.is_empty() {
                    return;
                }
                let jobs = {
                    let mut st = state.lock().unwrap();
                    if st.local.git_user.trim().is_empty() {
                        if let Some(usr) = detect_user_from_url(&u) {
                            st.local.git_user = usr;
                            save_local(&mut st);
                        }
                    }
                    if let Some(r) = st.shared.repos.iter_mut().find(|r| r.id == repo.as_str()) {
                        r.remote_url = u.clone();
                    }
                    save_shared(&mut st);
                    apply(&ui, &mut st);
                    jobs_for(&st, None, Some(&repo))
                };
                run_actions(&ui, &state, jobs, if do_push { "set_remote_push" } else { "set_remote" }, u);
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_publish_github(move |repo: SharedString, private: bool| {
            if let Some(ui) = weak.upgrade() {
                let (name, jobs) = {
                    let st = state.lock().unwrap();
                    let name = st.shared.repos.iter().find(|r| r.id == repo.as_str()).map(|r| r.name.clone()).unwrap_or_else(|| repo.to_string());
                    let mut jobs = jobs_for(&st, None, Some(&repo));
                    jobs.truncate(1);
                    (name, jobs)
                };
                run_actions(&ui, &state, jobs, if private { "publish_gh_priv" } else { "publish_gh_pub" }, name);
            }
        });
    }
    {
        ui.on_open_url(move |url: SharedString| {
            open_url(&url);
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_fetch_all(move |repo: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let jobs = jobs_for(&state.lock().unwrap(), None, Some(&repo));
                run_actions(&ui, &state, jobs, "fetch", String::new());
            }
        });
    }

    // Separate Fenster öffnen: Scan-Assistent & Einstellungen (inkl. Theme-Synchronisation)
    {
        let (sw, state) = (scan_win.as_weak(), state.clone());
        ui.on_open_scan(move || {
            if let Some(w) = sw.upgrade() {
                let theme = state.lock().unwrap().local.theme.clone();
                w.invoke_apply_theme(theme.as_str().into());
                let sz = w.window().size();
                if sz.width < 860 || sz.height < 580 {
                    w.window().set_size(slint::LogicalSize::new(920.0, 640.0));
                }
                let _ = w.show();
            }
        });
    }
    {
        let (cfgw, state) = (settings_win.as_weak(), state.clone());
        ui.on_open_settings(move || {
            if let Some(w) = cfgw.upgrade() {
                let st = state.lock().unwrap();
                sync_settings_fields(&w, &st);
                w.invoke_apply_theme(st.local.theme.as_str().into());
                let sz = w.window().size();
                if sz.width < 920 || sz.height < 640 {
                    w.window().set_size(slint::LogicalSize::new(980.0, 700.0));
                }
                let _ = w.show();
            }
        });
    }

    // ScanWindow Callbacks
    {
        let sw = scan_win.as_weak();
        scan_win.on_close_window(move || {
            if let Some(w) = sw.upgrade() {
                let _ = w.hide();
            }
        });
    }
    {
        let sw = scan_win.as_weak();
        scan_win.on_browse_roots(move || {
            let handle = sw.clone();
            let current_roots = sw.upgrade().map(|w| w.get_scan_roots().to_string()).unwrap_or_default();
            std::thread::spawn(move || {
                let mut dialog = rfd::FileDialog::new().set_title("Wurzelordner für Scan wählen");
                let last_path = current_roots.split(';').map(|s| s.trim()).filter(|s| !s.is_empty()).last();
                if let Some(lp) = last_path {
                    let pb = PathBuf::from(lp);
                    if pb.is_dir() {
                        dialog = dialog.set_directory(&pb);
                    } else if let Some(parent) = pb.parent() {
                        if parent.is_dir() {
                            dialog = dialog.set_directory(parent);
                        }
                    }
                } else if let Ok(cwd) = std::env::current_dir() {
                    dialog = dialog.set_directory(&cwd);
                }
                if let Some(folder) = dialog.pick_folder() {
                    let path_str = folder.to_string_lossy().to_string();
                    let _ = handle.upgrade_in_event_loop(move |w| {
                        let current = w.get_scan_roots().to_string();
                        let current_trimmed = current.trim();
                        let new_text = if current_trimmed.is_empty() {
                            path_str
                        } else {
                            let parts: Vec<&str> = current_trimmed.split(';').map(|s| s.trim()).collect();
                            if parts.contains(&path_str.as_str()) {
                                current
                            } else {
                                let base = current.trim_end();
                                if base.ends_with(';') {
                                    format!("{} {}", base, path_str)
                                } else {
                                    format!("{}; {}", base, path_str)
                                }
                            }
                        };
                        w.set_scan_roots(new_text.into());
                    });
                }
            });
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        scan_win.on_scan_start(move |roots: SharedString, depth: SharedString| {
            if let Some(ui) = weak.upgrade() {
                start_scan(&ui, &state, &roots, &depth);
            }
        });
    }
    {
        let state = state.clone();
        scan_win.on_scan_cancel(move || state.lock().unwrap().cancel.store(true, Ordering::Relaxed));
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        scan_win.on_scan_toggle(move |i: i32| {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.lock().unwrap();
                if let Some(it) = st.scan_items.get_mut(i as usize) {
                    it.checked = !it.checked;
                }
                apply(&ui, &mut st);
            }
        });
    }
    {
        let (weak, sw, state) = (ui.as_weak(), scan_win.as_weak(), state.clone());
        scan_win.on_scan_adopt(move || {
            if let Some(ui) = weak.upgrade() {
                {
                    let mut st = state.lock().unwrap();
                    let os = st.os;
                    let items: Vec<Adopt> = st
                        .scan_items
                        .iter()
                        .filter(|i| i.checked && !matches!(i.class, ScanClass::Known { .. }))
                        .map(|i| Adopt { path: i.path.clone(), remote: i.remote.clone(), class: i.class.clone() })
                        .collect();
                    let n = adopt(&mut st.shared, os, &items);
                    save_shared(&mut st);
                    st.scan_items.clear();
                    st.scan_progress = format!("{n} Standorte übernommen");
                    st.task = st.scan_progress.clone();
                    apply(&ui, &mut st);
                }
                if let Some(w) = sw.upgrade() {
                    let _ = w.hide();
                }
                refresh(&ui, &state);
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_clone_repo(move |url: SharedString, path: SharedString| {
            if let Some(ui) = weak.upgrade() {
                run_clone(&ui, &state, url.trim().to_string(), path.trim().to_string(), None);
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_clone_location(move |repo: SharedString, path: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let url = state.lock().unwrap().shared.repos.iter().find(|r| r.id == repo.as_str()).map(|r| r.remote_url.clone()).unwrap_or_default();
                if !url.is_empty() {
                    run_clone(&ui, &state, url, path.trim().to_string(), Some(repo.to_string()));
                }
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_clone_offline_location(move |loc_id: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let (repo_id, dest, url) = {
                    let st = state.lock().unwrap();
                    let loc = st.shared.locations.iter().find(|l| l.id == loc_id.as_str());
                    let repo_id = loc.map(|l| l.repo_id.clone()).unwrap_or_default();
                    let repo = st.shared.repos.iter().find(|r| r.id == repo_id);
                    let url = repo.map(|r| r.remote_url.clone()).unwrap_or_default();
                    let dest = loc.and_then(|l| {
                        match resolved(&st, l) {
                            Resolved::Local(p) => Some(p.display().to_string()),
                            Resolved::OtherOs => None,
                        }
                    }).unwrap_or_else(|| suggest_clone_dest(&st, &repo_id));
                    (repo_id, dest, url)
                };
                if !url.is_empty() && !dest.is_empty() {
                    run_clone(&ui, &state, url, dest, Some(repo_id));
                }
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_create_in_base_path(move |repo_id: SharedString, target_path: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let rid = repo_id.to_string();
                let dest = target_path.trim().to_string();
                if rid.is_empty() || dest.is_empty() {
                    return;
                }
                let (url, existing_local) = {
                    let st = state.lock().unwrap();
                    let repo = st.shared.repos.iter().find(|r| r.id == rid);
                    let url = repo.map(|r| r.remote_url.clone()).unwrap_or_default();
                    let existing_local = st.shared.locations_of(&rid).find_map(|l| match resolved(&st, l) {
                        Resolved::Local(p) if p.exists() => Some(p.display().to_string()),
                        _ => None,
                    });
                    (url, existing_local)
                };

                let clone_source = if !url.is_empty() {
                    url
                } else if let Some(local_src) = existing_local {
                    local_src
                } else {
                    return;
                };

                run_clone(&ui, &state, clone_source, dest, Some(rid));
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_browse_clone_dest(move || {
            let Some(ui) = weak.upgrade() else { return };
            crate::ensure_linux_dialog_ready();
            let start = {
                let _st = state.lock().unwrap();
                let cur = ui.get_selected_suggested_dest().to_string();
                if !cur.is_empty() && std::path::Path::new(&cur).exists() {
                    PathBuf::from(cur)
                } else if std::path::Path::new("/r/git").exists() {
                    PathBuf::from("/r/git")
                } else {
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                }
            };
            let pick = rfd::FileDialog::new()
                .set_directory(&start)
                .set_title("Zielordner zum Klonen auswählen")
                .pick_folder();
            if let Some(folder) = pick {
                ui.set_selected_suggested_dest(folder.to_string_lossy().to_string().into());
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_browse_new_location(move || {
            let Some(ui) = weak.upgrade() else { return };
            crate::ensure_linux_dialog_ready();
            let start = {
                let _st = state.lock().unwrap();
                let cur = ui.get_new_location_path().to_string();
                if !cur.is_empty() && std::path::Path::new(&cur).exists() {
                    PathBuf::from(cur)
                } else if std::path::Path::new("/r/git").exists() {
                    PathBuf::from("/r/git")
                } else {
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                }
            };
            let pick = rfd::FileDialog::new()
                .set_directory(&start)
                .set_title("Standort-Ordner auswählen")
                .pick_folder();
            if let Some(folder) = pick {
                ui.set_new_location_path(folder.to_string_lossy().to_string().into());
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_link_location(move |repo: SharedString, path: SharedString| {
            if let Some(ui) = weak.upgrade() {
                {
                    let mut st = state.lock().unwrap();
                    let os = st.os;
                    link_location(&mut st.shared, os, &repo, path.trim(), "");
                    save_shared(&mut st);
                    apply(&ui, &mut st);
                }
                refresh(&ui, &state);
            }
        });
    }

    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_load_stats(move |repo: SharedString, force: bool| {
            if let Some(ui) = weak.upgrade() {
                load_stats(&ui, &state, &repo, force);
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_select_view(move |key: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.lock().unwrap();
                st.view = key.to_string();
                apply(&ui, &mut st);
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_add_view(move |name: SharedString, tags: SharedString, status: SharedString, days: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let csv = |s: &str| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect::<Vec<_>>();
                let mut st = state.lock().unwrap();
                let v = SavedView { name: name.trim().to_string(), tags: csv(&tags), status: csv(&status), older_than_days: days.trim().parse().ok() };
                st.shared.views.retain(|x| x.name != v.name);
                st.view = format!("c:{}", v.name);
                st.shared.views.push(v);
                save_shared(&mut st);
                apply(&ui, &mut st);
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_delete_view(move |name: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.lock().unwrap();
                st.shared.views.retain(|x| x.name != name.as_str());
                if st.view == format!("c:{name}") {
                    st.view.clear();
                }
                save_shared(&mut st);
                apply(&ui, &mut st);
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_health_check(move |repo: SharedString| {
            if let Some(ui) = weak.upgrade() {
                run_health(&ui, &state, &repo);
            }
        });
    }
    {
        let state = state.clone();
        ui.on_open_in(move |loc: SharedString, tool: SharedString| {
            let st = state.lock().unwrap();
            if let Some(l) = st.shared.locations.iter().find(|l| l.id == loc.as_str()) {
                if let Resolved::Local(p) = resolved(&st, l) {
                    open_external(&p, &tool);
                }
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_delete_location(move |loc: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.lock().unwrap();
                st.shared.locations.retain(|l| l.id != loc.as_str());
                st.snaps.remove(loc.as_str());
                save_shared(&mut st);
                apply(&ui, &mut st);
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_toggle_favorite(move |repo: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.lock().unwrap();
                if let Some(r) = st.shared.repos.iter_mut().find(|r| r.id == repo.as_str()) {
                    r.favorite = !r.favorite;
                }
                save_shared(&mut st);
                apply(&ui, &mut st);
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_save_repo_meta(move |repo: SharedString, remote: SharedString, tags: SharedString, notes: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let rem = remote.trim().to_string();
                let jobs = {
                    let mut st = state.lock().unwrap();
                    if let Some(r) = st.shared.repos.iter_mut().find(|r| r.id == repo.as_str()) {
                        r.remote_url = rem.clone();
                        r.tags = tags.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                        r.notes = notes.trim().to_string();
                    }
                    save_shared(&mut st);
                    st.task = "Repo-Metadaten gespeichert".into();
                    apply(&ui, &mut st);
                    if !rem.is_empty() { jobs_for(&st, None, Some(&repo)) } else { Vec::new() }
                };
                if !jobs.is_empty() {
                    run_actions(&ui, &state, jobs, "set_remote", rem);
                }
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_delete_repo(move |repo: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.lock().unwrap();
                st.shared.repos.retain(|r| r.id != repo.as_str());
                st.shared.locations.retain(|l| l.repo_id != repo.as_str());
                if st.selected == repo.as_str() {
                    st.selected.clear();
                }
                save_shared(&mut st);
                apply(&ui, &mut st);
            }
        });
    }

    // SettingsWindow Callbacks
    {
        let cfgw = settings_win.as_weak();
        settings_win.on_close_window(move || {
            if let Some(w) = cfgw.upgrade() {
                let _ = w.hide();
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        settings_win.on_theme_changed(move |theme: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.lock().unwrap();
                st.local.theme = theme.trim().to_string();
                save_local(&mut st);
                apply_theme_all(&ui, &st);
            }
        });
    }
    {
        let (weak, cfgw, state) = (ui.as_weak(), settings_win.as_weak(), state.clone());
        settings_win.on_save_settings(move |theme: SharedString, git_user: SharedString, git: SharedString, refr: SharedString, timeout: SharedString, stale: SharedString, shared_win: SharedString, shared_unix: SharedString, local_linux: SharedString, local_win: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.lock().unwrap();
                st.local.theme = theme.trim().to_string();
                st.local.git_user = git_user.trim().to_string();
                let g = git.trim();
                st.local.git_path = if g.is_empty() { None } else { Some(g.to_string()) };
                if let Ok(v) = refr.trim().parse() {
                    st.local.refresh_interval_secs = v;
                }
                if let Ok(v) = timeout.trim().parse() {
                    st.local.reach_timeout_secs = v;
                }
                if let Ok(v) = stale.trim().parse() {
                    st.local.stale_days = v;
                }
                let sw = shared_win.trim();
                st.shared.base_paths.shared_windows = if sw.is_empty() { None } else { Some(sw.to_string()) };
                let su = shared_unix.trim();
                st.shared.base_paths.shared_unix = if su.is_empty() { None } else { Some(su.to_string()) };
                let ll = local_linux.trim();
                st.shared.base_paths.local_linux = if ll.is_empty() { None } else { Some(ll.to_string()) };
                let lw = local_win.trim();
                st.shared.base_paths.local_windows = if lw.is_empty() { None } else { Some(lw.to_string()) };

                save_local(&mut st);
                save_shared(&mut st);
                st.task = "Einstellungen gespeichert".into();
                if let Some(w) = cfgw.upgrade() {
                    sync_settings_fields(&w, &st);
                }
                apply(&ui, &mut st);
            }
        });
    }
    {
        let cfgw = settings_win.as_weak();
        settings_win.on_browse_base_path(move |field: SharedString| {
            let handle = cfgw.clone();
            let target = field.to_string();
            std::thread::spawn(move || {
                let dialog = rfd::FileDialog::new().set_title("Basispfad auswählen");
                if let Some(folder) = dialog.pick_folder() {
                    let path_str = folder.to_string_lossy().to_string();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = handle.upgrade() {
                            match target.as_str() {
                                "shared_unix" => w.set_cfg_shared_unix(path_str.into()),
                                "shared_win" => w.set_cfg_shared_win(path_str.into()),
                                "local_linux" => w.set_cfg_local_linux(path_str.into()),
                                "local_win" => w.set_cfg_local_win(path_str.into()),
                                _ => {}
                            }
                        }
                    });
                }
            });
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        settings_win.on_add_mapping(move |win: SharedString, unix: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.lock().unwrap();
                st.shared.mappings.push(PathMapping { windows: win.trim().to_string(), unix: unix.trim().to_string() });
                save_shared(&mut st);
                apply(&ui, &mut st);
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        settings_win.on_remove_mapping(move |idx: i32| {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.lock().unwrap();
                let i = idx as usize;
                if i < st.shared.mappings.len() {
                    st.shared.mappings.remove(i);
                    save_shared(&mut st);
                }
                apply(&ui, &mut st);
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        settings_win.on_toggle_health_check(move |id: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.lock().unwrap();
                let id_s = id.to_string();
                if let Some(pos) = st.local.health.disabled.iter().position(|x| x == &id_s) {
                    st.local.health.disabled.remove(pos);
                } else {
                    st.local.health.disabled.push(id_s);
                }
                save_local(&mut st);
                apply(&ui, &mut st);
            }
        });
    }

    // Hintergrund-Watcher: erkennt externe Änderungen an repos.toml + periodischer Auto-Refresh
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        std::thread::spawn(move || {
            let mut last_auto = Instant::now();
            use std::time::Instant;
            loop {
                std::thread::sleep(Duration::from_secs(5));
                let (changed, do_auto) = {
                    let mut st = state.lock().unwrap();
                    let cur = file_mtime(&st.shared_path);
                    let ext_changed = cur.is_some() && cur != st.shared_mtime;
                    if ext_changed {
                        if let Ok(cfg) = SharedConfig::load(&st.shared_path) {
                            st.shared = cfg;
                            st.shared_mtime = cur;
                            st.task = "repos.toml extern geändert – neu geladen".into();
                        }
                    }
                    let iv = st.local.refresh_interval_secs;
                    let auto = iv > 0 && last_auto.elapsed() >= Duration::from_secs(iv);
                    if auto {
                        last_auto = Instant::now();
                    }
                    (ext_changed, auto)
                };
                if changed || do_auto {
                    let (w, s) = (weak.clone(), state.clone());
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = w.upgrade() {
                            refresh(&ui, &s);
                        }
                    });
                }
            }
        });
    }

    {
        let mut st = state.lock().unwrap();
        if st.selected.is_empty() {
            if let Some(r) = st.shared.repos.iter().find(|r| r.id == "repoorganizer") {
                st.selected = r.id.clone();
            } else if let Some(r) = st.shared.repos.first() {
                st.selected = r.id.clone();
            }
        }
        sync_settings_fields(&settings_win, &st);
        apply(&ui, &mut st);
    }
    refresh(&ui, &state);
    ui.run()
}





