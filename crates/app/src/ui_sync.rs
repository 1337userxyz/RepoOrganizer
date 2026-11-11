use kit_slint::ToSlintModel;
use repoorganizer_core::health::{overall, ALL_CHECKS};
use repoorganizer_core::level::Level;
use repoorganizer_core::model::{Repo, Snapshot, Status};
use repoorganizer_core::paths::{Os, Resolved};
use repoorganizer_core::scan::ScanClass;
use repoorganizer_core::stats::summarize;
use repoorganizer_core::views::BUILTIN;
use repoorganizer_git::{guard_pull, guard_push};
use slint::{ModelRc, SharedString, VecModel};

use crate::ops::plan_smart_sync;
use crate::state::*;
use crate::{
    present, CheckRow, DirtyFileRow, DirtyLocFilterItem, HeatItem, InfoRow, LocationRow, MainWindow, MappingRow,
    MissingBasePathRow, RepoRow, ScanRow, SettingsWindow, SmartSyncLogRow, SmartSyncWindow, TaskRow, ViewRow,
};

pub fn apply_theme_all(ui: &MainWindow, st: &State) {
    let t: SharedString = st.local.theme.as_str().into();
    ui.invoke_apply_theme(t.clone());
    if let Some(sw) = st.scan_win.as_ref().and_then(|w| w.upgrade()) {
        sw.invoke_apply_theme(t.clone());
    }
    if let Some(cfgw) = st.settings_win.as_ref().and_then(|w| w.upgrade()) {
        cfgw.invoke_apply_theme(t.clone());
    }
    if let Some(ssw) = st.smart_sync_win.as_ref().and_then(|w| w.upgrade()) {
        ssw.invoke_apply_theme(t);
    }
}

pub fn apply(ui: &MainWindow, st: &mut State) {
    let now_t = now();
    apply_theme_all(ui, st);

    // Alle vorkommenden Branches für die Branch-ComboBox sammeln
    let mut all_branches: Vec<String> = st.snaps.values().filter_map(|s| s.branch.clone()).filter(|b| !b.is_empty()).collect();
    all_branches.sort();
    all_branches.dedup();
    let mut b_opts: Vec<SharedString> = vec!["Branch: Alle".into()];
    b_opts.extend(all_branches.iter().map(|b| SharedString::from(b.as_str())));
    ui.set_branch_options(b_opts.to_slint_model());

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
        let dirty_locs = snaps.iter().filter(|s| s.dirty_files + s.untracked > 0).count();
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

        let build_bytes: u64 = st
            .shared
            .locations_of(&r.id)
            .filter_map(|l| match resolved(st, l) {
                Resolved::Local(_) => st.snaps.get(&l.id).filter(|s| !matches!(s.state, Status::Unavailable | Status::NotARepo)),
                Resolved::OtherOs => None,
            })
            .map(|s| s.build_size_bytes)
            .sum();
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
            sync_info: present::working_tree_text_locs(dirty, stashes, dirty_locs).into(),
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
    ui.set_repos(rows.to_slint_model());

    // Smart-Sync Zähler & Details berechnen
    let mut dirty_repos = 0;
    let mut top_dirty_repo: Option<(String, String, u32)> = None;

    for r in &st.shared.repos {
        for l in st.shared.locations_of(&r.id) {
            if let Resolved::Local(p) = resolved(st, l) {
                if let Some(s) = st.snaps.get(&l.id) {
                    let dirty = s.dirty_files + s.untracked;
                    if dirty > 0 {
                        dirty_repos += 1;
                        let desc = format!("{} ({}) – {} uncommittete Datei(en)", r.name, p.display(), dirty);
                        if top_dirty_repo.as_ref().map(|(_, _, d)| dirty > *d).unwrap_or(true) {
                            top_dirty_repo = Some((l.id.clone(), desc, dirty));
                        }
                    }
                }
            }
        }
    }

    let plan = plan_smart_sync(st);
    let sync_behind = plan.pull_targets.len();
    let sync_dirty = plan.dirty_count;
    let sync_ahead = plan.ahead_count;
    let sync_missing_bp = plan.missing_clones.len();
    let sync_alt_after_push = plan.alt_after_push_targets.len();
    let sync_alt_standalone = plan.alt_standalone_targets.len();
    let total_sync_actions = sync_behind + sync_dirty + sync_ahead + sync_missing_bp + sync_alt_after_push + sync_alt_standalone;

    ui.set_sync_behind_count(sync_behind as i32);
    ui.set_sync_dirty_count(sync_dirty as i32);
    ui.set_sync_ahead_count(sync_ahead as i32);
    ui.set_sync_missing_bp_count(sync_missing_bp as i32);
    ui.set_sync_total_actions(total_sync_actions as i32);

    if let Some(ssw) = st.smart_sync_win.as_ref().and_then(|w| w.upgrade()) {
        ssw.set_sync_behind_count(sync_behind as i32);
        ssw.set_sync_dirty_count(sync_dirty as i32);
        ssw.set_sync_ahead_count(sync_ahead as i32);
        ssw.set_sync_missing_bp_count(sync_missing_bp as i32);
        ssw.set_sync_alt_after_push_count(sync_alt_after_push as i32);
        ssw.set_sync_alt_standalone_count(sync_alt_standalone as i32);

        if sync_behind == 0 {
            ssw.set_sync_opt_pull(false);
        }
        if sync_dirty + sync_ahead == 0 {
            ssw.set_sync_opt_commit_push(false);
        }
        if sync_missing_bp == 0 {
            ssw.set_sync_opt_clone_missing(false);
        }
        let eff_commit_push = ssw.get_sync_opt_commit_push() && (sync_dirty + sync_ahead > 0);
        let active_alt = sync_alt_standalone + if eff_commit_push { sync_alt_after_push } else { 0 };
        if active_alt == 0 {
            ssw.set_sync_opt_auto_sync(false);
        }

        ssw.set_sync_pull_details(plan.pull_details.to_slint_model());
        ssw.set_sync_push_details(plan.push_details.to_slint_model());
        ssw.set_sync_clone_details(plan.clone_details.to_slint_model());
        ssw.set_sync_alt_after_push_details(plan.alt_after_push_details.to_slint_model());
        ssw.set_sync_alt_standalone_details(plan.alt_standalone_details.to_slint_model());

        ssw.set_sync_running(st.smart_sync_running);
        ssw.set_sync_ticker_title(st.smart_sync_ticker_title.as_str().into());
        ssw.set_sync_ticker_detail(st.smart_sync_ticker_detail.as_str().into());
        ssw.set_sync_step_current(st.smart_sync_step_current);
        ssw.set_sync_step_total(st.smart_sync_step_total);
        ssw.set_sync_ticker_log(ModelRc::new(VecModel::from(
            st.smart_sync_log
                .iter()
                .map(|t| SmartSyncLogRow {
                    state: t.state,
                    action: t.action.as_str().into(),
                    repo: t.repo.as_str().into(),
                    path: t.path.as_str().into(),
                    detail: t.detail.lines().last().unwrap_or("").into(),
                })
                .collect::<Vec<_>>(),
        )));
    }

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
    // Ansichten mit Zählern
    let mut vrows = vec![ViewRow { key: "".into(), name: "Alle Repos".into(), count: st.shared.repos.len() as i32, selected: st.view.is_empty(), custom: false }];
    let mut keys: Vec<(String, String, bool)> = BUILTIN.iter().map(|(k, n)| (format!("b:{k}"), n.to_string(), false)).collect();
    keys.extend(st.shared.views.iter().map(|v| (format!("c:{}", v.name), v.name.clone(), true)));
    let mut selected_idx: i32 = 0;
    for (i, (key, name, custom)) in keys.into_iter().enumerate() {
        let count = st.shared.repos.iter().filter(|r| view_ok(st, &key, &ctx_for(st, r, now_t))).count() as i32;
        let is_sel = st.view == key;
        if is_sel {
            selected_idx = (i + 1) as i32;
        }
        vrows.push(ViewRow { selected: is_sel, key: key.into(), name: name.into(), count, custom });
    }
    let view_options: Vec<SharedString> = vrows.iter().map(|v| format!("{} ({})", v.name, v.count).into()).collect();
    let is_custom = vrows.get(selected_idx as usize).map(|v| v.custom).unwrap_or(false);
    ui.set_views(vrows.to_slint_model());
    ui.set_view_options(view_options.to_slint_model());
    ui.set_selected_view_index(selected_idx);
    ui.set_active_view_is_custom(is_custom);
    ui.set_active_view(st.view.as_str().into());
    let drows: Vec<InfoRow> = st
        .drift
        .get(&st.selected)
        .map(|m| m.iter().map(|m| InfoRow { label: "".into(), text: m.text.as_str().into(), advice: "".into(), kind: level_kind(m.level) }).collect())
        .unwrap_or_default();
    ui.set_drift_rows(drows.to_slint_model());
    let hf = st.health.get(&st.selected);
    ui.set_health_rows(
        hf.map(|v| {
            v.iter()
                .map(|(label, f)| InfoRow { label: label.as_str().into(), text: f.text.as_str().into(), advice: f.advice.as_str().into(), kind: level_kind(f.level) })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
        .to_slint_model(),
    );
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
        let mut all_dirty_rows: Vec<DirtyFileRow> = Vec::new();
        let mut filter_items: Vec<DirtyLocFilterItem> = Vec::new();
        let mut dirty_loc_count = 0;
        let mut unique_dirty_paths = std::collections::HashSet::new();

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
            let is_unavailable = other_os || matches!(state, Status::Unavailable | Status::NotARepo);
            let b_bytes = if is_unavailable { 0 } else { snap.map(|s| s.build_size_bytes).unwrap_or(0) };
            let can_clean = !is_unavailable && b_bytes > 0;
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

            let lbl = if l.label.is_empty() { l.id.clone() } else { l.label.clone() };

            if !other_os {
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

            let loc_details = if dirty > 0 && !other_os {
                if let Some(details) = snap.map(|s| s.dirty_details.clone()).filter(|d| !d.is_empty()) {
                    details
                } else if let Resolved::Local(p) = resolved(st, l) {
                    let git = repoorganizer_git::CliGit::new(st.local.git_path.as_deref());
                    git.dirty_files(&p)
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };

            let loc_dirty_rows: Vec<DirtyFileRow> = loc_details.iter().map(|df| {
                unique_dirty_paths.insert(df.path.clone());
                DirtyFileRow {
                    path: df.path.as_str().into(),
                    status: df.status.as_str().into(),
                    status_code: df.status_code.as_str().into(),
                    kind: df.kind,
                    mtime_text: present::format_mtime(df.mtime, t).into(),
                    size_text: present::format_file_size(df.size_bytes).into(),
                    loc_label: lbl.as_str().into(),
                    loc_id: l.id.as_str().into(),
                    loc_path: path.as_str().into(),
                }
            }).collect();

            if !loc_dirty_rows.is_empty() {
                dirty_loc_count += 1;
                filter_items.push(DirtyLocFilterItem {
                    id: l.id.as_str().into(),
                    label: lbl.as_str().into(),
                    count: loc_dirty_rows.len() as i32,
                });
                all_dirty_rows.extend(loc_dirty_rows.iter().cloned());
            }

            let last_dirty_time = loc_details
                .iter()
                .filter_map(|d| d.mtime)
                .max()
                .map(|m| present::format_mtime(Some(m), t))
                .unwrap_or_else(|| if dirty > 0 { "vor kurzem".into() } else { "".into() });

            locs.push(LocationRow {
                id: l.id.as_str().into(),
                can_pull,
                can_push,
                can_commit,
                can_clean,
                build_size: b_size.into(),
                label: lbl.as_str().into(),
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
                last_dirty_time: last_dirty_time.into(),
                other_os,
                dirty_files: loc_dirty_rows.to_slint_model(),
            });
        }

        let headline = if all_dirty_rows.is_empty() {
            "".to_string()
        } else if dirty_loc_count > 1 {
            format!(
                "✎ {} uncommittete Datei{} an {} Standorten (insgesamt {} Dateistände) in {}",
                unique_dirty_paths.len(),
                if unique_dirty_paths.len() == 1 { "" } else { "en" },
                dirty_loc_count,
                all_dirty_rows.len(),
                r.name
            )
        } else {
            let loc_lbl = all_dirty_rows.first().map(|f| f.loc_label.as_str()).unwrap_or("");
            format!(
                "✎ {} uncommittete Datei{} in {} ({loc_lbl})",
                all_dirty_rows.len(),
                if all_dirty_rows.len() == 1 { "" } else { "en" },
                r.name
            )
        };

        let dirty_summary = if !all_dirty_rows.is_empty() {
            let all_details: Vec<_> = st.shared.locations_of(&r.id)
                .filter_map(|l| st.snaps.get(&l.id))
                .flat_map(|s| s.dirty_details.iter().cloned())
                .collect();
            present::dirty_summary(&all_details, t)
        } else {
            "".into()
        };

        let current_filter = ui.get_dirty_loc_filter().to_string();
        let display_rows: Vec<DirtyFileRow> = if current_filter == "all" || current_filter.is_empty() {
            all_dirty_rows.clone()
        } else {
            let filtered: Vec<_> = all_dirty_rows.iter().filter(|f| f.loc_id == current_filter.as_str()).cloned().collect();
            if filtered.is_empty() {
                all_dirty_rows.clone()
            } else {
                filtered
            }
        };

        ui.set_selected_dirty_files(display_rows.to_slint_model());
        ui.set_selected_dirty_summary(dirty_summary.into());
        ui.set_selected_dirty_headline(headline.into());
        ui.set_dirty_loc_options(filter_items.to_slint_model());

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
        ui.set_selected_dirty_files(ModelRc::default());
        ui.set_selected_dirty_summary("".into());
        ui.set_selected_dirty_headline("".into());
        ui.set_dirty_loc_options(ModelRc::default());
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

pub fn sync_settings_fields(cfgw: &SettingsWindow, st: &State) {
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

pub fn init_smart_sync_options(ssw: &SmartSyncWindow, st: &State) {
    let plan = plan_smart_sync(st);
    let has_pull = !plan.pull_targets.is_empty();
    let has_push = (plan.dirty_count + plan.ahead_count) > 0;
    let has_alt = !plan.alt_standalone_targets.is_empty() || (has_push && !plan.alt_after_push_targets.is_empty());

    ssw.set_sync_opt_pull(has_pull);
    ssw.set_sync_opt_commit_push(has_push);
    if plan.missing_clones.is_empty() {
        ssw.set_sync_opt_clone_missing(false);
    }
    ssw.set_sync_opt_auto_sync(has_alt);
}
