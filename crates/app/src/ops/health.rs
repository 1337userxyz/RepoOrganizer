use std::path::PathBuf;

use repoorganizer_core::health::{evaluate, Finding};
use repoorganizer_core::paths::Resolved;
use repoorganizer_git::facts::FactOptions;
use repoorganizer_git::CliGit;
use slint::ComponentHandle;

use crate::state::{now, offline_state, resolved, Shared};
use crate::ui_sync::apply;
use crate::MainWindow;

/// Health-Checks für ein Repo (oder alle bei leerer ID) im Hintergrund.
pub fn run_health(ui: &MainWindow, state: &Shared, repo_id: &str) {
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
        let opts = FactOptions {
            check_remote: settings.enabled("remote"),
            secret_scan: settings.enabled("secrets"),
            large_file_min_bytes: settings.large_file_mb * 1024 * 1024,
            ..Default::default()
        };
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
