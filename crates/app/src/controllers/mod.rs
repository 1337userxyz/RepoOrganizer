pub mod git;
pub mod location;
pub mod repo;
pub mod scan;
pub mod settings;
pub mod view;

use crate::state::Shared;
use crate::{MainWindow, ScanWindow, SettingsWindow, SmartSyncWindow};

pub fn register_all(
    ui: &MainWindow,
    state: &Shared,
    scan_win: &ScanWindow,
    settings_win: &SettingsWindow,
    smart_sync_win: &SmartSyncWindow,
) {
    git::register(ui, state, smart_sync_win);
    repo::register(ui, state);
    location::register(ui, state);
    view::register(ui, state);
    scan::register(ui, state, scan_win);
    settings::register(ui, state, settings_win);
}
