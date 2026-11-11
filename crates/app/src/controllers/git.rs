use slint::{ComponentHandle, SharedString};

use crate::ops::{jobs_for, refresh, run_actions, run_smart_sync};
use crate::state::Shared;
use crate::ui_sync::{apply, init_smart_sync_options};
use crate::{MainWindow, SmartSyncWindow};

pub fn register(ui: &MainWindow, state: &Shared, smart_sync_win: &SmartSyncWindow) {
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
        ui.on_location_action(move |loc: SharedString, kind: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let jobs = jobs_for(&state.lock().unwrap(), Some(&loc), None);
                run_actions(&ui, &state, jobs, &kind, String::new());
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
        let (weak, ssw, state) = (ui.as_weak(), smart_sync_win.as_weak(), state.clone());
        ui.on_open_smart_sync(move || {
            if let (Some(ui), Some(w)) = (weak.upgrade(), ssw.upgrade()) {
                let mut st = state.lock().unwrap();
                if !st.smart_sync_running {
                    init_smart_sync_options(&w, &st);
                }
                apply(&ui, &mut st);
                let sz = w.window().size();
                if sz.width < 760 || sz.height < 540 {
                    w.window().set_size(slint::LogicalSize::new(860.0, 660.0));
                }
                let _ = w.show();
            }
        });
    }
    {
        let ssw = smart_sync_win.as_weak();
        smart_sync_win.on_close_window(move || {
            if let Some(w) = ssw.upgrade() {
                let _ = w.hide();
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        smart_sync_win.on_run_smart_sync(move |do_pull, do_commit_push, msg, do_clone_missing, do_auto_sync| {
            if let Some(ui) = weak.upgrade() {
                run_smart_sync(&ui, &state, do_pull, do_commit_push, msg.to_string(), do_clone_missing, do_auto_sync);
            }
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
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_pull_repo(move |repo: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let jobs = jobs_for(&state.lock().unwrap(), None, Some(&repo));
                run_actions(&ui, &state, jobs, "pull", String::new());
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_push_repo(move |repo: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let jobs = jobs_for(&state.lock().unwrap(), None, Some(&repo));
                run_actions(&ui, &state, jobs, "push", String::new());
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_reset_repo(move |repo: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let jobs = jobs_for(&state.lock().unwrap(), None, Some(&repo));
                run_actions(&ui, &state, jobs, "reset", String::new());
            }
        });
    }
    {
        let (weak, _state) = (ui.as_weak(), state.clone());
        ui.on_commit_repo(move |repo: SharedString| {
            if let Some(ui) = weak.upgrade() {
                ui.invoke_select(repo);
                if ui.get_detail_panel_height() < 140.0 {
                    ui.set_detail_panel_height(140.0);
                }
                ui.set_detail_tab(0);
            }
        });
    }
}
