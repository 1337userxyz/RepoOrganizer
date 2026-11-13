use kit_picker::PickRequest;
use repoorganizer_core::paths::PathMapping;
use slint::{ComponentHandle, SharedString};

use crate::state::{save_local, save_shared, Shared};
use crate::ui_sync::{apply, apply_theme_all, sync_settings_fields};
use crate::{AppTheme, MainWindow, SettingsWindow};

pub fn register(ui: &MainWindow, state: &Shared, settings_win: &SettingsWindow) {
    // Open Settings dialog from MainWindow
    {
        let (cfgw, state) = (settings_win.as_weak(), state.clone());
        ui.on_open_settings(move || {
            if let Some(w) = cfgw.upgrade() {
                let st = state.lock().unwrap();
                sync_settings_fields(&w, &st);
                w.global::<AppTheme>().invoke_apply(st.local.theme.as_str().into());
                let sz = w.window().size();
                if sz.width < 920 || sz.height < 640 {
                    w.window().set_size(slint::LogicalSize::new(980.0, 700.0));
                }
                let _ = w.show();
            }
        });
    }

    // SettingsWindow Callbacks
    {
        let cfgw = settings_win.as_weak();
        settings_win.on_close_window(move || {
            if let Some(w) = cfgw.upgrade() {
                let _ = w.hide();
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        settings_win.on_theme_changed(move |theme: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.lock().unwrap();
                st.local.theme = theme.trim().to_string();
                save_local(&mut st);
                apply_theme_all(&ui, &st);
            }
        });
    }
    {
        let (weak, cfgw, state) = (ui.as_weak(), settings_win.as_weak(), state.clone());
        settings_win.on_save_settings(move |theme: SharedString, git_user: SharedString, git: SharedString, refr: SharedString, timeout: SharedString, stale: SharedString, shared_win: SharedString, shared_unix: SharedString, local_linux: SharedString, local_win: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.lock().unwrap();
                st.local.theme = theme.trim().to_string();
                st.local.git_user = git_user.trim().to_string();
                let g = git.trim();
                st.local.git_path = if g.is_empty() { None } else { Some(g.to_string()) };
                if let Ok(v) = refr.trim().parse() {
                    st.local.refresh_interval_secs = v;
                }
                if let Ok(v) = timeout.trim().parse() {
                    st.local.reach_timeout_secs = v;
                }
                if let Ok(v) = stale.trim().parse() {
                    st.local.stale_days = v;
                }
                let sw = shared_win.trim();
                st.shared.base_paths.shared_windows = if sw.is_empty() { None } else { Some(sw.to_string()) };
                let su = shared_unix.trim();
                st.shared.base_paths.shared_unix = if su.is_empty() { None } else { Some(su.to_string()) };
                let ll = local_linux.trim();
                st.shared.base_paths.local_linux = if ll.is_empty() { None } else { Some(ll.to_string()) };
                let lw = local_win.trim();
                st.shared.base_paths.local_windows = if lw.is_empty() { None } else { Some(lw.to_string()) };

                save_local(&mut st);
                save_shared(&mut st);
                st.task = "Einstellungen gespeichert".into();
                if let Some(w) = cfgw.upgrade() {
                    sync_settings_fields(&w, &st);
                }
                apply(&ui, &mut st);
            }
        });
    }
    {
        let cfgw = settings_win.as_weak();
        settings_win.on_browse_base_path(move |field: SharedString| {
            let target = field.to_string();
            PickRequest::folder()
                .title("Basispfad auswählen")
                .context("base_path")
                .pick_into(cfgw.clone(), move |w, folder| {
                    let path_str: SharedString = folder.to_string_lossy().to_string().into();
                    match target.as_str() {
                        "shared_unix" => w.set_cfg_shared_unix(path_str),
                        "shared_win" => w.set_cfg_shared_win(path_str),
                        "local_linux" => w.set_cfg_local_linux(path_str),
                        "local_win" => w.set_cfg_local_win(path_str),
                        _ => {}
                    }
                });
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        settings_win.on_add_mapping(move |win: SharedString, unix: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.lock().unwrap();
                st.shared.mappings.push(PathMapping { windows: win.trim().to_string(), unix: unix.trim().to_string() });
                save_shared(&mut st);
                apply(&ui, &mut st);
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        settings_win.on_remove_mapping(move |idx: i32| {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.lock().unwrap();
                let i = idx as usize;
                if i < st.shared.mappings.len() {
                    st.shared.mappings.remove(i);
                    save_shared(&mut st);
                }
                apply(&ui, &mut st);
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        settings_win.on_toggle_health_check(move |id: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.lock().unwrap();
                let id_s = id.to_string();
                if let Some(pos) = st.local.health.disabled.iter().position(|x| x == &id_s) {
                    st.local.health.disabled.remove(pos);
                } else {
                    st.local.health.disabled.push(id_s);
                }
                save_local(&mut st);
                apply(&ui, &mut st);
            }
        });
    }
}
