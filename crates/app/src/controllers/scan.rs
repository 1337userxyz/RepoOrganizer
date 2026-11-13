use std::path::PathBuf;
use std::sync::atomic::Ordering;

use kit_picker::PickRequest;
use repoorganizer_core::scan::{adopt, Adopt, ScanClass};
use slint::{ComponentHandle, SharedString};

use crate::ops::{refresh, start_scan};
use crate::state::{save_shared, Shared};
use crate::ui_sync::apply;
use crate::{AppTheme, MainWindow, ScanWindow};

pub fn register(ui: &MainWindow, state: &Shared, scan_win: &ScanWindow) {
    // Open Scan dialog from MainWindow
    {
        let (sw, state) = (scan_win.as_weak(), state.clone());
        ui.on_open_scan(move || {
            if let Some(w) = sw.upgrade() {
                let theme = state.lock().unwrap().local.theme.clone();
                w.global::<AppTheme>().invoke_apply(theme.as_str().into());
                let sz = w.window().size();
                if sz.width < 860 || sz.height < 580 {
                    w.window().set_size(slint::LogicalSize::new(920.0, 640.0));
                }
                let _ = w.show();
            }
        });
    }

    // ScanWindow Callbacks
    {
        let sw = scan_win.as_weak();
        scan_win.on_close_window(move || {
            if let Some(w) = sw.upgrade() {
                let _ = w.hide();
            }
        });
    }
    {
        let sw = scan_win.as_weak();
        scan_win.on_browse_roots(move || {
            let current_roots = sw.upgrade().map(|w| w.get_scan_roots().to_string()).unwrap_or_default();
            let last_path = current_roots.split(';').map(|s| s.trim()).filter(|s| !s.is_empty()).last().map(PathBuf::from);

            let mut req = PickRequest::folder()
                .title("Wurzelordner für Scan wählen")
                .context("scan_roots");
            if let Some(hint) = last_path {
                req = req.start_hint(hint);
            }
            req.pick_into(sw.clone(), |w, folder| {
                let path_str = folder.to_string_lossy().to_string();
                let current = w.get_scan_roots().to_string();
                let current_trimmed = current.trim();
                let new_text = if current_trimmed.is_empty() {
                    path_str
                } else {
                    let parts: Vec<&str> = current_trimmed.split(';').map(|s| s.trim()).collect();
                    if parts.contains(&path_str.as_str()) {
                        current
                    } else {
                        let base = current.trim_end();
                        if base.ends_with(';') {
                            format!("{} {}", base, path_str)
                        } else {
                            format!("{}; {}", base, path_str)
                        }
                    }
                };
                w.set_scan_roots(new_text.into());
            });
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        scan_win.on_scan_start(move |roots: SharedString, depth: SharedString| {
            if let Some(ui) = weak.upgrade() {
                start_scan(&ui, &state, &roots, &depth);
            }
        });
    }
    {
        let state = state.clone();
        scan_win.on_scan_cancel(move || state.lock().unwrap().cancel.store(true, Ordering::Relaxed));
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        scan_win.on_scan_toggle(move |i: i32| {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.lock().unwrap();
                if let Some(it) = st.scan_items.get_mut(i as usize) {
                    it.checked = !it.checked;
                }
                apply(&ui, &mut st);
            }
        });
    }
    {
        let (weak, sw, state) = (ui.as_weak(), scan_win.as_weak(), state.clone());
        scan_win.on_scan_adopt(move || {
            if let Some(ui) = weak.upgrade() {
                {
                    let mut st = state.lock().unwrap();
                    let os = st.os;
                    let items: Vec<Adopt> = st
                        .scan_items
                        .iter()
                        .filter(|i| i.checked && !matches!(i.class, ScanClass::Known { .. }))
                        .map(|i| Adopt { path: i.path.clone(), remote: i.remote.clone(), class: i.class.clone() })
                        .collect();
                    let n = adopt(&mut st.shared, os, &items);
                    save_shared(&mut st);
                    st.scan_items.clear();
                    st.scan_progress = format!("{n} Standorte übernommen");
                    st.task = st.scan_progress.clone();
                    apply(&ui, &mut st);
                }
                if let Some(w) = sw.upgrade() {
                    let _ = w.hide();
                }
                refresh(&ui, &state);
            }
        });
    }
}
