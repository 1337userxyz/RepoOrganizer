use slint::{ComponentHandle, SharedString};

use crate::ops::{jobs_for, refresh, run_actions, run_smart_sync};
use crate::state::Shared;
use crate::MainWindow;

pub fn register(ui: &MainWindow, state: &Shared) {
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
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_run_smart_sync(move |do_pull, do_commit_push, msg, do_clone_missing, do_auto_sync| {
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
}
