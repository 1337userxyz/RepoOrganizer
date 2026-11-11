use std::path::{Path, PathBuf};

use repoorganizer_core::paths::{Os, Resolved};
use repoorganizer_core::scan::link_location;
use repoorganizer_git::{CliGit, GitBackend, PullMode};
use slint::ComponentHandle;

use crate::ops::git::refresh;
use crate::state::{resolved, save_shared, Shared, State, TaskEntry};
use crate::ui_sync::apply;
use crate::{MainWindow, SmartSyncDetailRow};

#[derive(Clone)]
pub struct PullTarget {
    pub loc_id: String,
    pub path: PathBuf,
    pub label: String,
    pub behind: u32,
    pub branch: String,
}

#[derive(Clone)]
pub struct PushTarget {
    pub repo_id: String,
    pub loc_id: String,
    pub path: PathBuf,
    pub label: String,
    pub dirty_count: u32,
    pub ahead_count: u32,
    pub files_summary: String,
}

#[derive(Clone)]
pub struct SkippedPushTarget {
    pub label: String,
    pub primary_label: String,
}

#[derive(Clone)]
pub struct MissingClone {
    pub repo_id: String,
    pub repo_name: String,
    pub target_path: String,
    pub source: String,
    pub exists_on_disk: bool,
}

#[derive(Clone)]
pub struct AltSyncTarget {
    pub repo_id: String,
    pub loc_id: String,
    pub path: PathBuf,
    pub label: String,
    pub reason: String,
}

pub struct SmartSyncPlan {
    pub pull_targets: Vec<PullTarget>,
    pub push_targets: Vec<PushTarget>,
    pub skipped_push_targets: Vec<SkippedPushTarget>,
    pub missing_clones: Vec<MissingClone>,
    pub alt_after_push_targets: Vec<AltSyncTarget>,
    pub alt_standalone_targets: Vec<AltSyncTarget>,
    pub dirty_count: usize,
    pub ahead_count: usize,
    pub pull_details: Vec<SmartSyncDetailRow>,
    pub push_details: Vec<SmartSyncDetailRow>,
    pub clone_details: Vec<SmartSyncDetailRow>,
    pub alt_after_push_details: Vec<SmartSyncDetailRow>,
    pub alt_standalone_details: Vec<SmartSyncDetailRow>,
}

pub fn plan_smart_sync(st: &State) -> SmartSyncPlan {
    let mut pull_targets = Vec::new();
    let mut push_targets = Vec::new();
    let mut skipped_push_targets = Vec::new();
    let mut missing_clones = Vec::new();
    let mut alt_after_push_targets = Vec::new();
    let mut alt_standalone_targets = Vec::new();

    let mut dirty_count = 0;
    let mut ahead_count = 0;

    // 1. Push-Ziele, Pull-Ziele und Alternativpfade pro Repository bestimmen
    for r in &st.shared.repos {
        let mut push_candidates = Vec::new();
        let mut local_locs = Vec::new();

        for l in st.shared.locations_of(&r.id) {
            if let Resolved::Local(p) = resolved(st, l) {
                if p.is_dir() {
                    let label = if l.label.is_empty() {
                        r.name.clone()
                    } else if l.label == r.name {
                        r.name.clone()
                    } else {
                        format!("{} ({})", r.name, l.label)
                    };
                    let snap = st.snaps.get(&l.id);
                    let d_count = snap.map(|s| s.dirty_files + s.untracked).unwrap_or(0);
                    let a_count = snap.map(|s| s.ahead).unwrap_or(0);
                    let b_count = snap.map(|s| s.behind).unwrap_or(0);
                    let branch = snap.and_then(|s| s.branch.clone()).unwrap_or_default();
                    let head_sha = snap.and_then(|s| s.head_sha.clone());
                    let last_commit = snap.and_then(|s| s.last_commit).unwrap_or(0);

                    if b_count > 0 && d_count == 0 && a_count == 0 {
                        pull_targets.push(PullTarget {
                            loc_id: l.id.clone(),
                            path: p.clone(),
                            label: label.clone(),
                            behind: b_count,
                            branch: branch.clone(),
                        });
                    }

                    if d_count > 0 || a_count > 0 {
                        let files_summary = if d_count > 0 {
                            let details = snap.map(|s| &s.dirty_details).filter(|d| !d.is_empty());
                            if let Some(list) = details {
                                let max_show = 8;
                                let shown: Vec<String> = list
                                    .iter()
                                    .take(max_show)
                                    .map(|df| format!("[{}] {}", df.status_code, df.path))
                                    .collect();
                                let extra = if list.len() > max_show {
                                    format!("  (+{} weitere)", list.len() - max_show)
                                } else {
                                    String::new()
                                };
                                format!("📄 {}{}", shown.join("  ·  "), extra)
                            } else {
                                format!("📄 {} geänderte/neue Datei(en) im Arbeitsverzeichnis", d_count)
                            }
                        } else {
                            String::new()
                        };

                        let is_primary = matches!(l.role, repoorganizer_core::model::Role::Primary);
                        push_candidates.push((
                            PushTarget {
                                repo_id: r.id.clone(),
                                loc_id: l.id.clone(),
                                path: p.clone(),
                                label: label.clone(),
                                dirty_count: d_count,
                                ahead_count: a_count,
                                files_summary,
                            },
                            is_primary,
                        ));
                    }

                    local_locs.push((l.id.clone(), p, label, d_count, a_count, b_count, branch, head_sha, last_commit));
                }
            }
        }

        if !push_candidates.is_empty() {
            push_candidates.sort_by(|a, b| b.1.cmp(&a.1));
            let (chosen, _) = push_candidates.remove(0);
            if chosen.dirty_count > 0 {
                dirty_count += 1;
            } else {
                ahead_count += 1;
            }
            let chosen_loc_id = chosen.loc_id.clone();
            let chosen_label = chosen.label.clone();
            push_targets.push(chosen);

            for (skip, _) in push_candidates {
                skipped_push_targets.push(SkippedPushTarget {
                    label: skip.label,
                    primary_label: chosen_label.clone(),
                });
            }

            // Saubere Alternativpfade desselben Repos vormerken, die nach dem Push aktualisiert werden
            for (alt_id, alt_path, alt_label, alt_dirty, alt_ahead, _, _, _, _) in &local_locs {
                if *alt_id != chosen_loc_id && *alt_dirty == 0 && *alt_ahead == 0 {
                    alt_after_push_targets.push(AltSyncTarget {
                        repo_id: r.id.clone(),
                        loc_id: alt_id.clone(),
                        path: alt_path.clone(),
                        label: alt_label.clone(),
                        reason: format!("nach Push von {}", chosen_label),
                    });
                }
            }
        } else if local_locs.len() >= 2 {
            // Prüfen, ob lokale Standorte desselben Repos bereits unterschiedlichen Commit-Stand haben
            let mut sorted_refs = local_locs.clone();
            sorted_refs.sort_by(|a, b| b.8.cmp(&a.8));
            if let Some((ref_id, _, ref_label, _, _, _, ref_branch, Some(ref_sha), _)) = sorted_refs.first() {
                for (alt_id, alt_path, alt_label, alt_dirty, alt_ahead, alt_behind, alt_branch, alt_sha, _) in &local_locs {
                    if alt_id != ref_id
                        && *alt_dirty == 0
                        && *alt_ahead == 0
                        && *alt_behind == 0
                        && alt_branch == ref_branch
                    {
                        if let Some(s_alt) = alt_sha {
                            if s_alt != ref_sha {
                                alt_standalone_targets.push(AltSyncTarget {
                                    repo_id: r.id.clone(),
                                    loc_id: alt_id.clone(),
                                    path: alt_path.clone(),
                                    label: alt_label.clone(),
                                    reason: format!("angleichen an {}", ref_label),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    // 2. Fehlende Basispfade berechnen
    let bp = &st.shared.base_paths;
    let os = st.os;
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
            st.shared.locations_of(&r.id).find_map(|l| match resolved(st, l) {
                Resolved::Local(p) if p.is_dir() => Some(p.display().to_string()),
                _ => None,
            })
        };

        let Some(src) = clone_source else { continue };

        for base in &active_bps {
            let sep = if os == Os::Windows { "\\" } else { "/" };
            let target_path = format!("{base}{sep}{}", r.name);
            let exists = st.shared.locations_of(&r.id).any(|l| match resolved(st, l) {
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
                let target_p = Path::new(&target_path);
                let exists_on_disk = target_p.join(".git").exists() || target_p.is_dir();
                missing_clones.push(MissingClone {
                    repo_id: r.id.clone(),
                    repo_name: r.name.clone(),
                    target_path,
                    source: src.clone(),
                    exists_on_disk,
                });
            }
        }
    }

    // Detailzeilen für die UI bauen
    let pull_details = pull_targets
        .iter()
        .map(|t| SmartSyncDetailRow {
            title: t.label.as_str().into(),
            path: t.path.display().to_string().into(),
            info: format!(
                "↓ {} Commit{}{}",
                t.behind,
                if t.behind == 1 { "" } else { "s" },
                if t.branch.is_empty() { String::new() } else { format!(" · ⎇ {}", t.branch) }
            )
            .into(),
            files: "".into(),
            kind: 1,
        })
        .collect();

    let push_details = push_targets
        .iter()
        .map(|t| {
            let info = if t.dirty_count > 0 && t.ahead_count > 0 {
                format!("✎ {} Datei(en) · ↑ {} Commit(s)", t.dirty_count, t.ahead_count)
            } else if t.dirty_count > 0 {
                format!("✎ {} uncommittete Datei{}", t.dirty_count, if t.dirty_count == 1 { "" } else { "en" })
            } else {
                format!("↑ {} Commit{} ungesendet", t.ahead_count, if t.ahead_count == 1 { "" } else { "s" })
            };
            SmartSyncDetailRow {
                title: t.label.as_str().into(),
                path: t.path.display().to_string().into(),
                info: info.into(),
                files: t.files_summary.as_str().into(),
                kind: if t.dirty_count > 0 { 2 } else { 1 },
            }
        })
        .collect();

    let clone_details = missing_clones
        .iter()
        .map(|c| SmartSyncDetailRow {
            title: c.repo_name.as_str().into(),
            path: format!("→ {}  (von {})", c.target_path, c.source).into(),
            info: if c.exists_on_disk {
                "Ordner verknüpfen".into()
            } else {
                "Klonen".into()
            },
            files: "".into(),
            kind: 1,
        })
        .collect();

    let alt_after_push_details = alt_after_push_targets
        .iter()
        .map(|a| SmartSyncDetailRow {
            title: a.label.as_str().into(),
            path: a.path.display().to_string().into(),
            info: a.reason.as_str().into(),
            files: "".into(),
            kind: 0,
        })
        .collect();

    let alt_standalone_details = alt_standalone_targets
        .iter()
        .map(|a| SmartSyncDetailRow {
            title: a.label.as_str().into(),
            path: a.path.display().to_string().into(),
            info: a.reason.as_str().into(),
            files: "".into(),
            kind: 0,
        })
        .collect();

    SmartSyncPlan {
        pull_targets,
        push_targets,
        skipped_push_targets,
        missing_clones,
        alt_after_push_targets,
        alt_standalone_targets,
        dirty_count,
        ahead_count,
        pull_details,
        push_details,
        clone_details,
        alt_after_push_details,
        alt_standalone_details,
    }
}

fn update_ticker(
    weak: &slint::Weak<MainWindow>,
    state: &Shared,
    step_cur: i32,
    step_tot: i32,
    title: impl Into<String>,
    detail: impl Into<String>,
) {
    let title_s = title.into();
    let detail_s = detail.into();
    let (w, s) = (weak.clone(), state.clone());
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = w.upgrade() else { return };
        let mut st = s.lock().unwrap();
        st.smart_sync_step_current = step_cur;
        st.smart_sync_step_total = step_tot;
        st.smart_sync_ticker_title = title_s.clone();
        st.smart_sync_ticker_detail = detail_s.clone();
        st.task = if detail_s.is_empty() {
            title_s
        } else {
            format!("{title_s} — {detail_s}")
        };
        apply(&ui, &mut st);
    });
}

fn push_ticker_log(
    weak: &slint::Weak<MainWindow>,
    state: &Shared,
    title: impl Into<String>,
    state_code: i32,
    detail: impl Into<String>,
) -> u64 {
    let title_s = title.into();
    let detail_s = detail.into();
    let mut st = state.lock().unwrap();
    let id = st.next_task;
    st.next_task += 1;
    st.tasks.push(TaskEntry {
        id,
        title: title_s.clone(),
        state: state_code,
        detail: detail_s.clone(),
    });
    st.smart_sync_log.push(TaskEntry {
        id,
        title: title_s,
        state: state_code,
        detail: detail_s,
    });
    if let Some(ui) = weak.upgrade() {
        apply(&ui, &mut st);
    }
    id
}

pub fn run_smart_sync(
    ui: &MainWindow,
    state: &Shared,
    do_pull: bool,
    do_commit_push: bool,
    commit_msg: String,
    do_clone_missing: bool,
    do_auto_sync: bool,
) {
    let (git_path, os, plan) = {
        let mut st = state.lock().unwrap();
        if st.smart_sync_running {
            return;
        }
        let plan = plan_smart_sync(&st);
        let total_steps = (if do_commit_push { plan.push_targets.len() } else { 0 })
            + (if do_auto_sync {
                plan.alt_standalone_targets.len()
                    + (if do_commit_push { plan.alt_after_push_targets.len() } else { 0 })
            } else {
                0
            })
            + (if do_pull { plan.pull_targets.len() } else { 0 })
            + (if do_clone_missing { plan.missing_clones.len() } else { 0 })
            + 1; // +1 für abschließenden Refresh

        st.smart_sync_running = true;
        st.smart_sync_step_current = 0;
        st.smart_sync_step_total = total_steps as i32;
        st.smart_sync_ticker_title = "⏳ Smart-Sync gestartet…".into();
        st.smart_sync_ticker_detail = "Bereite ausgewählte Synchronisationsschritte vor…".into();
        st.smart_sync_log.clear();
        apply(ui, &mut st);
        (st.local.git_path.clone(), st.os, plan)
    };

    let eff_msg = if commit_msg.trim().is_empty() {
        "Smart-Sync: Änderungen gesichert".to_string()
    } else {
        commit_msg.trim().to_string()
    };

    let (weak, state_clone) = (ui.as_weak(), state.clone());
    std::thread::spawn(move || {
        let git = CliGit::new(git_path.as_deref());
        let mut step_cur: i32 = 0;
        let step_tot: i32 = {
            let st = state_clone.lock().unwrap();
            st.smart_sync_step_total
        };
        let mut ok_count = 0usize;
        let mut warn_count = 0usize;

        // Helper zum Synchronisieren eines Alternativpfads
        let sync_alt_target = |alt: &AltSyncTarget, step: i32, step_tot: i32, ok_cnt: &mut usize, warn_cnt: &mut usize| {
            update_ticker(
                &weak,
                &state_clone,
                step,
                step_tot,
                format!("⏳ [{step}/{step_tot}] Alternativpfad angleichen: {}", alt.label),
                format!("Prüfe Arbeitsverzeichnis in {}…", alt.path.display()),
            );

            let alt_task_id = push_ticker_log(
                &weak,
                &state_clone,
                format!("Auto-Sync: {}", alt.label),
                0,
                format!("prüfe & aktualisiere ({})…", alt.path.display()),
            );

            let pre_snap = git.snapshot(&alt.loc_id, &alt.path);
            let alt_dirty = pre_snap.dirty_files + pre_snap.untracked;

            if alt_dirty > 0 {
                *warn_cnt += 1;
                let (w, s, p_snap, a_id) = (weak.clone(), state_clone.clone(), pre_snap, alt.loc_id.clone());
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = w.upgrade() else { return };
                    let mut st = s.lock().unwrap();
                    let msg = format!("Sicherheitsstopp: {alt_dirty} uncommittete Datei(en) vorhanden – kein Überschreiben");
                    if let Some(t) = st.tasks.iter_mut().find(|t| t.id == alt_task_id) {
                        t.state = 2;
                        t.detail = msg.clone();
                    }
                    if let Some(t) = st.smart_sync_log.iter_mut().find(|t| t.id == alt_task_id) {
                        t.state = 2;
                        t.detail = msg;
                    }
                    st.snaps.insert(a_id, p_snap);
                    apply(&ui, &mut st);
                });
                return;
            }

            if pre_snap.ahead > 0 {
                *warn_cnt += 1;
                let (w, s, p_snap, a_id) = (weak.clone(), state_clone.clone(), pre_snap, alt.loc_id.clone());
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = w.upgrade() else { return };
                    let mut st = s.lock().unwrap();
                    let msg = "Sicherheitsstopp: Eigene ungesendete Commits vorhanden (Divergenzschutz)".to_string();
                    if let Some(t) = st.tasks.iter_mut().find(|t| t.id == alt_task_id) {
                        t.state = 2;
                        t.detail = msg.clone();
                    }
                    if let Some(t) = st.smart_sync_log.iter_mut().find(|t| t.id == alt_task_id) {
                        t.state = 2;
                        t.detail = msg;
                    }
                    st.snaps.insert(a_id, p_snap);
                    apply(&ui, &mut st);
                });
                return;
            }

            update_ticker(
                &weak,
                &state_clone,
                step,
                step_tot,
                format!("⏳ [{step}/{step_tot}] Alternativpfad angleichen: {}", alt.label),
                format!("git fetch & pull --ff-only in {}…", alt.path.display()),
            );

            let pull_res = git.fetch(&alt.path).and_then(|_| git.pull(&alt.path, PullMode::FfOnly));
            if pull_res.is_ok() {
                *ok_cnt += 1;
            } else {
                *warn_cnt += 1;
            }
            let alt_snap = git.snapshot(&alt.loc_id, &alt.path);
            let (w, s, p_res, a_snap, a_id) = (weak.clone(), state_clone.clone(), pull_res, alt_snap, alt.loc_id.clone());
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = w.upgrade() else { return };
                let mut st = s.lock().unwrap();
                let (st_code, detail_msg) = match &p_res {
                    Ok(out) => (
                        1,
                        if out.trim().is_empty() {
                            "Bereits aktuell".to_string()
                        } else {
                            out.trim().lines().last().unwrap_or("Aktualisiert").to_string()
                        },
                    ),
                    Err(e) => (2, format!("Hinweis: {e}")),
                };
                if let Some(t) = st.tasks.iter_mut().find(|t| t.id == alt_task_id) {
                    t.state = st_code;
                    t.detail = detail_msg.clone();
                }
                if let Some(t) = st.smart_sync_log.iter_mut().find(|t| t.id == alt_task_id) {
                    t.state = st_code;
                    t.detail = detail_msg;
                }
                st.snaps.insert(a_id, a_snap);
                apply(&ui, &mut st);
            });
        };

        // 1. Commit & Push
        if do_commit_push {
            for skip in &plan.skipped_push_targets {
                warn_count += 1;
                push_ticker_log(
                    &weak,
                    &state_clone,
                    format!("Übersprungen: {}", skip.label),
                    2,
                    format!(
                        "Kollisionsschutz: Änderungen wurden bereits über '{}' gepusht. Bitte manuell prüfen.",
                        skip.primary_label
                    ),
                );
            }

            for target in &plan.push_targets {
                step_cur += 1;
                let dirty = target.dirty_count > 0;
                let kind = if dirty { "commit & push" } else { "push" };

                update_ticker(
                    &weak,
                    &state_clone,
                    step_cur,
                    step_tot,
                    format!("⏳ [{step_cur}/{step_tot}] {kind}: {}", target.label),
                    if dirty {
                        format!("Stage & Commit ({} Datei(en)) in {}…", target.dirty_count, target.path.display())
                    } else {
                        format!("Pushe {} Commit(s) aus {}…", target.ahead_count, target.path.display())
                    },
                );

                let task_id = push_ticker_log(
                    &weak,
                    &state_clone,
                    format!("{kind}: {}", target.label),
                    0,
                    format!("wird ausgeführt in {}…", target.path.display()),
                );

                let res = (|| -> Result<String, String> {
                    if dirty {
                        git.stage_all(&target.path).map_err(|e| e.to_string())?;
                        git.commit(&target.path, &eff_msg).map_err(|e| e.to_string())?;
                        update_ticker(
                            &weak,
                            &state_clone,
                            step_cur,
                            step_tot,
                            format!("⏳ [{step_cur}/{step_tot}] {kind}: {}", target.label),
                            format!("Commit erstellt – pushe zu Remote ({})…", target.path.display()),
                        );
                    }
                    git.push(&target.path).map_err(|e| e.to_string())
                })();

                let res_ok = res.is_ok();
                if res_ok {
                    ok_count += 1;
                } else {
                    warn_count += 1;
                }
                let snap = git.snapshot(&target.loc_id, &target.path);
                {
                    let (w, s, r_val, s_val, l_id) = (
                        weak.clone(),
                        state_clone.clone(),
                        res.clone(),
                        snap.clone(),
                        target.loc_id.clone(),
                    );
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(ui) = w.upgrade() else { return };
                        let mut st = s.lock().unwrap();
                        let (st_code, detail_msg) = match &r_val {
                            Ok(out) => (
                                1,
                                if out.trim().is_empty() {
                                    "Erfolgreich gepusht".to_string()
                                } else {
                                    out.trim().lines().last().unwrap_or("ok").to_string()
                                },
                            ),
                            Err(e) => (2, e.clone()),
                        };
                        if let Some(t) = st.tasks.iter_mut().find(|t| t.id == task_id) {
                            t.state = st_code;
                            t.detail = detail_msg.clone();
                        }
                        if let Some(t) = st.smart_sync_log.iter_mut().find(|t| t.id == task_id) {
                            t.state = st_code;
                            t.detail = detail_msg;
                        }
                        st.snaps.insert(l_id, s_val);
                        apply(&ui, &mut st);
                    });
                }

                // Push erfolgreich & do_auto_sync aktiv: saubere Alternativpfade dieses Repos automatisch angleichen
                if do_auto_sync {
                    for alt in plan.alt_after_push_targets.iter().filter(|a| a.repo_id == target.repo_id) {
                        step_cur += 1;
                        if res_ok {
                            sync_alt_target(alt, step_cur, step_tot, &mut ok_count, &mut warn_count);
                        } else {
                            warn_count += 1;
                            push_ticker_log(
                                &weak,
                                &state_clone,
                                format!("Auto-Sync übersprungen: {}", alt.label),
                                2,
                                "Vorheriger Push fehlgeschlagen – Alternativpfad unverändert.",
                            );
                        }
                    }
                }
            }
        }

        // 1b. Standalone Alternativpfade angleichen (falls vorhanden und Option aktiv)
        if do_auto_sync {
            for alt in &plan.alt_standalone_targets {
                step_cur += 1;
                sync_alt_target(alt, step_cur, step_tot, &mut ok_count, &mut warn_count);
            }
        }

        // 2. Pull
        if do_pull {
            for target in &plan.pull_targets {
                step_cur += 1;
                update_ticker(
                    &weak,
                    &state_clone,
                    step_cur,
                    step_tot,
                    format!("⏳ [{step_cur}/{step_tot}] Remote-Updates pullen: {}", target.label),
                    format!("Hole {} Commit(s) in {}…", target.behind, target.path.display()),
                );

                let task_id = push_ticker_log(
                    &weak,
                    &state_clone,
                    format!("pull: {}", target.label),
                    0,
                    format!("hole Updates in {}…", target.path.display()),
                );

                let res = git.fetch(&target.path).and_then(|_| git.pull(&target.path, PullMode::FfOnly));
                if res.is_ok() {
                    ok_count += 1;
                } else {
                    warn_count += 1;
                }
                let snap = git.snapshot(&target.loc_id, &target.path);
                let (w, s, r_val, s_val, l_id) = (weak.clone(), state_clone.clone(), res, snap, target.loc_id.clone());
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = w.upgrade() else { return };
                    let mut st = s.lock().unwrap();
                    let (st_code, detail_msg) = match &r_val {
                        Ok(out) => (
                            1,
                            if out.trim().is_empty() {
                                "Bereits aktuell".to_string()
                            } else {
                                out.trim().lines().last().unwrap_or("Aktualisiert").to_string()
                            },
                        ),
                        Err(e) => (2, format!("Fehler: {e}")),
                    };
                    if let Some(t) = st.tasks.iter_mut().find(|t| t.id == task_id) {
                        t.state = st_code;
                        t.detail = detail_msg.clone();
                    }
                    if let Some(t) = st.smart_sync_log.iter_mut().find(|t| t.id == task_id) {
                        t.state = st_code;
                        t.detail = detail_msg;
                    }
                    st.snaps.insert(l_id, s_val);
                    apply(&ui, &mut st);
                });
            }
        }

        // 3. Clone Missing in configured Base Paths
        if do_clone_missing {
            let mut any_linked = false;
            for item in &plan.missing_clones {
                step_cur += 1;
                let target_p = Path::new(&item.target_path);

                if item.exists_on_disk || target_p.join(".git").exists() || target_p.is_dir() {
                    any_linked = true;
                    ok_count += 1;
                    update_ticker(
                        &weak,
                        &state_clone,
                        step_cur,
                        step_tot,
                        format!("⏳ [{step_cur}/{step_tot}] Basispfad verknüpfen: {}", item.repo_name),
                        format!("Verknüpfe vorhandenen Ordner {}…", item.target_path),
                    );
                    push_ticker_log(
                        &weak,
                        &state_clone,
                        format!("link: {} -> {}", item.repo_name, item.target_path),
                        1,
                        "Vorhandenes Verzeichnis verknüpft",
                    );
                    let (w, s, rid, tp) = (
                        weak.clone(),
                        state_clone.clone(),
                        item.repo_id.clone(),
                        item.target_path.clone(),
                    );
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(ui) = w.upgrade() else { return };
                        let mut st = s.lock().unwrap();
                        link_location(&mut st.shared, os, &rid, &tp, "");
                        save_shared(&mut st);
                        apply(&ui, &mut st);
                    });
                    continue;
                }

                update_ticker(
                    &weak,
                    &state_clone,
                    step_cur,
                    step_tot,
                    format!("⏳ [{step_cur}/{step_tot}] Klonen: {}", item.repo_name),
                    format!("Klone von {} nach {}…", item.source, item.target_path),
                );

                let task_id = push_ticker_log(
                    &weak,
                    &state_clone,
                    format!("clone: {} -> {}", item.repo_name, item.target_path),
                    0,
                    "klone in Basispfad…",
                );

                let res = git.clone_repo(&item.source, target_p).map_err(|e| e.to_string());
                let res_ok = res.is_ok();
                if res_ok {
                    any_linked = true;
                    ok_count += 1;
                } else {
                    warn_count += 1;
                }
                let (w, s, rid, tp, r_val) = (
                    weak.clone(),
                    state_clone.clone(),
                    item.repo_id.clone(),
                    item.target_path.clone(),
                    res,
                );
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = w.upgrade() else { return };
                    let mut st = s.lock().unwrap();
                    let (st_code, detail_msg) = match &r_val {
                        Ok(out) => (
                            1,
                            if out.trim().is_empty() {
                                "Erfolgreich geklont & verknüpft".to_string()
                            } else {
                                out.trim().lines().last().unwrap_or("ok").to_string()
                            },
                        ),
                        Err(e) => (2, e.clone()),
                    };
                    if let Some(t) = st.tasks.iter_mut().find(|t| t.id == task_id) {
                        t.state = st_code;
                        t.detail = detail_msg.clone();
                    }
                    if let Some(t) = st.smart_sync_log.iter_mut().find(|t| t.id == task_id) {
                        t.state = st_code;
                        t.detail = detail_msg;
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
                    st.shared
                        .locations
                        .iter()
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

        // Abschließender Refresh & Ticker-Fertigstellung
        step_cur = step_tot;
        update_ticker(
            &weak,
            &state_clone,
            step_cur,
            step_tot,
            "⏳ Aktualisiere Repository-Status…",
            "Lese aktuelle Git-Snapshots aller Standorte…",
        );

        let (w, s) = (weak.clone(), state_clone.clone());
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = w.upgrade() else { return };
            {
                let mut st = s.lock().unwrap();
                st.smart_sync_running = false;
                st.smart_sync_step_current = step_tot;
                if warn_count == 0 {
                    st.smart_sync_ticker_title = "✅ Smart-Sync erfolgreich abgeschlossen".into();
                    st.smart_sync_ticker_detail = format!("{ok_count} Aktion(en) erfolgreich ausgeführt.");
                } else {
                    st.smart_sync_ticker_title = "⚠ Smart-Sync mit Hinweisen abgeschlossen".into();
                    st.smart_sync_ticker_detail = format!("{ok_count} erfolgreich · {warn_count} Hinweis(e)/Fehler (siehe Protokoll unten).");
                }
                apply(&ui, &mut st);
            }
            refresh(&ui, &s);
        });
    });
}
