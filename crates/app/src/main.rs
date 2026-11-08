mod controllers;
mod ops;
mod present;
mod state;
mod ui_sync;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use repoorganizer_core::cache;
use repoorganizer_core::config::{detect_user_from_url, LocalConfig, SharedConfig};
use repoorganizer_core::paths::{resolve, Os, Resolved};
use repoorganizer_git::{CliGit, GitBackend};
use slint::ComponentHandle;

use crate::ops::refresh;
use crate::state::{file_mtime, Shared, State};
use crate::ui_sync::{apply, sync_settings_fields};

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    let _ = kit_logging::init_logger(Some(PathBuf::from("repoorganizer.log")), 1000, log::LevelFilter::Info);
    kit_picker::ensure_dialog_ready();

    if std::env::args().nth(1).as_deref() == Some("status") {
        let lp = LocalConfig::default_path().unwrap_or_else(|| PathBuf::from("local.toml"));
        let local = LocalConfig::load(&lp).unwrap_or_default();
        let sp = SharedConfig::program_dir_path();
        let shared = SharedConfig::load(&sp).unwrap_or_default();
        let git = CliGit::new(local.git_path.as_deref());
        let os = Os::current();
        for r in &shared.repos {
            println!("=== {} ({}) ===", r.name, r.id);
            for l in shared.locations_of(&r.id) {
                match resolve(l, os, &shared.mappings) {
                    Resolved::Local(p) => {
                        let s = git.snapshot(&l.id, &p);
                        let has_remote = s.remote_url.as_ref().map(|u| !u.is_empty()).unwrap_or(false);
                        let (rel_str, _) = present::remote_relation_badge(s.ahead, s.behind, has_remote);
                        let tree_str = present::working_tree_text(s.dirty_files + s.untracked, s.stash_count);
                        println!("  [{}] {} -> {} | {}", l.id, p.display(), rel_str, tree_str);
                    }
                    Resolved::OtherOs => println!("  [{}] (auf anderem System)", l.id),
                }
            }
        }
        return Ok(());
    }
    let local_path = LocalConfig::default_path().unwrap_or_else(|| PathBuf::from("local.toml"));
    let mut error = String::new();
    let mut local = LocalConfig::load(&local_path).unwrap_or_else(|e| {
        error = e.to_string();
        LocalConfig::default()
    });
    let shared_path = SharedConfig::program_dir_path();
    let shared = if shared_path.exists() {
        SharedConfig::load(&shared_path).unwrap_or_else(|e| {
            error = e.to_string();
            SharedConfig::default()
        })
    } else {
        // Einmalige Migration aus dem alten AppData-Pfad, falls vorhanden
        let legacy = local.shared_path(&local_path);
        if legacy.exists() {
            let cfg = SharedConfig::load(&legacy).unwrap_or_default();
            let _ = cfg.save(&shared_path);
            cfg
        } else {
            SharedConfig::default()
        }
    };

    // Git-User automatisch erkennen, falls noch nicht in local.toml gesetzt
    if local.git_user.trim().is_empty() {
        let detected = shared
            .repos
            .iter()
            .find_map(|r| detect_user_from_url(&r.remote_url))
            .or_else(|| CliGit::new(local.git_path.as_deref()).global_git_user());
        if let Some(u) = detected {
            local.git_user = u;
            let _ = local.save(&local_path);
        }
    }

    let ui = MainWindow::new()?;
    ui.window().set_size(slint::LogicalSize::new(local.window_width as f32, local.window_height as f32));
    let scan_win = ScanWindow::new()?;
    scan_win.window().set_size(slint::LogicalSize::new(920.0, 640.0));
    let settings_win = SettingsWindow::new()?;
    settings_win.window().set_size(slint::LogicalSize::new(980.0, 700.0));

    let state: Shared = Arc::new(Mutex::new(State {
        local,
        local_path,
        stats: cache::activity_path().map(|p| cache::load_activity(&p)).unwrap_or_default(),
        stats_busy: false,
        stats_path: cache::activity_path(),
        drift: HashMap::new(),
        health: HashMap::new(),
        health_busy: false,
        view: String::new(),
        cache_path: cache::default_path(),
        scan_items: Vec::new(),
        scanning: false,
        scan_progress: String::new(),
        cancel: Arc::new(AtomicBool::new(false)),
        tasks: Vec::new(),
        next_task: 1,
        shared,
        shared_mtime: file_mtime(&shared_path),
        shared_path: shared_path.clone(),
        snaps: cache::default_path().map(|p| cache::load(&p)).unwrap_or_default().into_iter().map(|s| (s.location_id.clone(), s)).collect(),
        os: Os::current(),
        filter: String::new(),
        selected: String::new(),
        error,
        task: format!("Config: {}", shared_path.display()),
        scan_win: Some(scan_win.as_weak()),
        settings_win: Some(settings_win.as_weak()),
    }));

    // Register all controllers
    controllers::register_all(&ui, &state, &scan_win, &settings_win);

    // Hintergrund-Watcher: erkennt externe Änderungen an repos.toml + periodischer Auto-Refresh
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        std::thread::spawn(move || {
            let mut last_auto = Instant::now();
            loop {
                std::thread::sleep(Duration::from_secs(5));
                let (changed, do_auto) = {
                    let mut st = state.lock().unwrap();
                    let cur = file_mtime(&st.shared_path);
                    let ext_changed = cur.is_some() && cur != st.shared_mtime;
                    if ext_changed {
                        if let Ok(cfg) = SharedConfig::load(&st.shared_path) {
                            st.shared = cfg;
                            st.shared_mtime = cur;
                            st.task = "repos.toml extern geändert – neu geladen".into();
                        }
                    }
                    let iv = st.local.refresh_interval_secs;
                    let auto = iv > 0 && last_auto.elapsed() >= Duration::from_secs(iv);
                    if auto {
                        last_auto = Instant::now();
                    }
                    (ext_changed, auto)
                };
                if changed || do_auto {
                    let (w, s) = (weak.clone(), state.clone());
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = w.upgrade() {
                            refresh(&ui, &s);
                        }
                    });
                }
            }
        });
    }

    {
        let mut st = state.lock().unwrap();
        if st.selected.is_empty() {
            if let Some(r) = st.shared.repos.iter().find(|r| r.id == "repoorganizer") {
                st.selected = r.id.clone();
            } else if let Some(r) = st.shared.repos.first() {
                st.selected = r.id.clone();
            }
        }
        sync_settings_fields(&settings_win, &st);
        apply(&ui, &mut st);
    }
    refresh(&ui, &state);
    ui.run()
}
