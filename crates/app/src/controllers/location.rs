use kit_picker::PickRequest;
use repoorganizer_core::paths::Resolved;
use repoorganizer_core::scan::link_location;
use slint::{ComponentHandle, SharedString};

use crate::ops::{refresh, run_clone};
use crate::state::{open_external, open_url, resolved, save_shared, suggest_clone_dest, Shared};
use crate::ui_sync::apply;
use crate::MainWindow;

pub fn register(ui: &MainWindow, state: &Shared) {
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
        let weak = ui.as_weak();
        ui.on_browse_clone_dest(move || {
            let cur = weak.upgrade().map(|u| u.get_selected_suggested_dest().to_string()).unwrap_or_default();
            let mut req = PickRequest::folder()
                .title("Zielordner zum Klonen auswählen")
                .context("clone_dest");
            if !cur.is_empty() {
                req = req.start_hint(cur);
            }
            req.pick_into(weak.clone(), |ui, folder| {
                ui.set_selected_suggested_dest(folder.to_string_lossy().to_string().into());
            });
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_browse_new_location(move || {
            let cur = weak.upgrade().map(|u| u.get_new_location_path().to_string()).unwrap_or_default();
            let mut req = PickRequest::folder()
                .title("Standort-Ordner auswählen")
                .context("location_path");
            if !cur.is_empty() {
                req = req.start_hint(cur);
            }
            req.pick_into(weak.clone(), |ui, folder| {
                ui.set_new_location_path(folder.to_string_lossy().to_string().into());
            });
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
        ui.on_open_url(move |url: SharedString| {
            open_url(&url);
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
        let state = state.clone();
        ui.on_open_file(move |loc: SharedString, file_path: SharedString| {
            let st = state.lock().unwrap();
            if let Some(l) = st.shared.locations.iter().find(|l| l.id == loc.as_str()) {
                if let Resolved::Local(p) = resolved(&st, l) {
                    let full = p.join(file_path.as_str());
                    let _ = kit_launcher::open_path(&full);
                }
            }
        });
    }
    {
        let state = state.clone();
        ui.on_open_file_in_code(move |loc: SharedString, file_path: SharedString| {
            let st = state.lock().unwrap();
            if let Some(l) = st.shared.locations.iter().find(|l| l.id == loc.as_str()) {
                if let Resolved::Local(p) = resolved(&st, l) {
                    let full = p.join(file_path.as_str());
                    let _ = kit_launcher::open_in_editor(&full, Some("code"));
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
}
