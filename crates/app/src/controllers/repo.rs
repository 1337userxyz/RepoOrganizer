use std::path::Path;

use repoorganizer_core::config::detect_user_from_url;
use repoorganizer_git::CliGit;
use slint::{ComponentHandle, SharedString};

use crate::ops::{jobs_for, load_stats, refresh, run_actions, run_health};
use crate::state::{add_repo, name_from_path, save_local, save_shared, Shared};
use crate::ui_sync::apply;
use crate::MainWindow;

pub fn register(ui: &MainWindow, state: &Shared) {
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
        ui.on_derive_name(move |path: SharedString| name_from_path(&path).into());
    }
    {
        let state = state.clone();
        ui.on_derive_url(move |name: SharedString, path: SharedString| {
            let st = state.lock().unwrap();
            let p = path.trim();
            if !p.is_empty() {
                let git = CliGit::new(st.local.git_path.as_deref());
                if let Some(existing) = git.remote_origin(Path::new(p)) {
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
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_load_stats(move |repo: SharedString, force: bool| {
            if let Some(ui) = weak.upgrade() {
                load_stats(&ui, &state, &repo, force);
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
}
