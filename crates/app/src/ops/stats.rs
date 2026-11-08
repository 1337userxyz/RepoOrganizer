use repoorganizer_core::cache::{self, RepoStats};
use repoorganizer_core::paths::Resolved;
use repoorganizer_core::stats::to_activity;
use repoorganizer_git::CliGit;
use slint::ComponentHandle;

use crate::state::{now, offline_state, resolved, Shared};
use crate::ui_sync::apply;
use crate::MainWindow;

/// Statistik (Commits, Zeilen, Autoren, Größe) des ersten erreichbaren Standorts laden.
pub fn load_stats(ui: &MainWindow, state: &Shared, repo_id: &str, force: bool) {
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
        let rs = RepoStats {
            fetched_at: now(),
            git_size: git.git_size(&path),
            activity: to_activity(&repo_id, &commits),
        };
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
