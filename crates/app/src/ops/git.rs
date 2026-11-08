use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use repoorganizer_core::cache;
use repoorganizer_core::model::{Snapshot, Status};
use repoorganizer_core::paths::Resolved;
use repoorganizer_core::reach::{self, Reachability};
use repoorganizer_core::scan::{adopt, classify, link_location, Adopt};
use repoorganizer_git::{guard_pull, guard_push, snapshots_parallel, CliGit, GitBackend, PullMode};
use slint::ComponentHandle;

use crate::ops::drift::compute_drift;
use crate::state::{resolved, save_shared, Shared, State, TaskEntry};
use crate::ui_sync::apply;
use crate::MainWindow;

/// Erreichbarkeit (mit Timeout, parallel) prüfen und Git-Snapshots im Hintergrund laden.
pub fn refresh(ui: &MainWindow, state: &Shared) {
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
pub fn run_actions(ui: &MainWindow, state: &Shared, jobs: Vec<(String, PathBuf, String)>, kind: &str, msg: String) {
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
pub fn jobs_for(st: &State, loc_id: Option<&str>, repo_id: Option<&str>) -> Vec<(String, PathBuf, String)> {
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

/// Klont im Hintergrund und trägt den neuen Standort danach ein (`repo_id` None: automatisch zuordnen).
pub fn run_clone(ui: &MainWindow, state: &Shared, url: String, dest: String, repo_id: Option<String>) {
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
        let res = git.clone_repo(&url, Path::new(&dest)).map_err(|e| e.to_string());
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
                            let class = classify(&st.shared, os, Path::new(&dest), Some(&url));
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
