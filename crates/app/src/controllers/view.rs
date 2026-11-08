use repoorganizer_core::config::SavedView;
use slint::{ComponentHandle, SharedString};

use crate::state::{save_shared, Shared};
use crate::ui_sync::apply;
use crate::MainWindow;

pub fn register(ui: &MainWindow, state: &Shared) {
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_filter_changed(move |t: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.lock().unwrap();
                st.filter = t.to_string();
                apply(&ui, &mut st);
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_column_filter_changed(move || {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.lock().unwrap();
                apply(&ui, &mut st);
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_select_view(move |key: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.lock().unwrap();
                st.view = key.to_string();
                apply(&ui, &mut st);
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_add_view(move |name: SharedString, tags: SharedString, status: SharedString, days: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let csv = |s: &str| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect::<Vec<_>>();
                let mut st = state.lock().unwrap();
                let v = SavedView { name: name.trim().to_string(), tags: csv(&tags), status: csv(&status), older_than_days: days.trim().parse().ok() };
                st.shared.views.retain(|x| x.name != v.name);
                st.view = format!("c:{}", v.name);
                st.shared.views.push(v);
                save_shared(&mut st);
                apply(&ui, &mut st);
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_delete_view(move |name: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.lock().unwrap();
                st.shared.views.retain(|x| x.name != name.as_str());
                if st.view == format!("c:{name}") {
                    st.view.clear();
                }
                save_shared(&mut st);
                apply(&ui, &mut st);
            }
        });
    }
}
