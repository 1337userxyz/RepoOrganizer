use std::path::{Path, PathBuf};

use repoorganizer_core::paths::{Os, Resolved};
use repoorganizer_core::scan::link_location;
use repoorganizer_git::{CliGit, GitBackend, PullMode};
use slint::ComponentHandle;

use crate::ops::git::refresh;
use crate::state::{resolved, save_shared, Shared, TaskEntry};
use crate::ui_sync::apply;
use crate::MainWindow;

pub fn run_smart_sync(
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
                        if !t.is_empty() && Path::new(t).exists() {
                            active_bps.push(t);
                        }
                    }
                    if os == Os::Linux {
                        if let Some(ll) = &bp.local_linux {
                            let t = ll.trim();
                            if !t.is_empty() && Path::new(t).exists() {
                                active_bps.push(t);
                            }
                        }
                    }
                } else {
                    if let Some(sw) = &bp.shared_windows {
                        let t = sw.trim();
                        if !t.is_empty() && Path::new(t).exists() {
                            active_bps.push(t);
                        }
                    }
                    if let Some(lw) = &bp.local_windows {
                        let t = lw.trim();
                        if !t.is_empty() && Path::new(t).exists() {
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
                            Resolved::Local(p) => p == Path::new(&target_path),
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
                let target_p = Path::new(&item.target_path);
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
