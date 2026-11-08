use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

use repoorganizer_core::reach::{self, Reachability};
use repoorganizer_core::scan::{classify, ScanClass};
use repoorganizer_git::scan::{scan, ScanOptions};
use repoorganizer_git::CliGit;
use slint::ComponentHandle;

use crate::state::{ScanItem, Shared};
use crate::ui_sync::apply;
use crate::MainWindow;

/// Scan im Hintergrund starten; Fortschritt gedrosselt melden.
pub fn start_scan(ui: &MainWindow, state: &Shared, roots: &str, depth: &str) {
    let roots: Vec<PathBuf> = roots.split(|c| c == ';' || c == '\n').map(str::trim).filter(|s| !s.is_empty()).map(PathBuf::from).collect();
    if roots.is_empty() {
        return;
    }
    let (git_path, timeout, cancel) = {
        let mut st = state.lock().unwrap();
        if st.scanning {
            return;
        }
        st.scanning = true;
        st.scan_items.clear();
        st.scan_progress = "Starte Scan…".into();
        st.cancel.store(false, Ordering::Relaxed);
        apply(ui, &mut st);
        (st.local.git_path.clone(), st.local.reach_timeout_secs, st.cancel.clone())
    };
    let opts = ScanOptions { roots, max_depth: depth.trim().parse().unwrap_or(4), ..Default::default() };
    let (weak, state) = (ui.as_weak(), state.clone());
    std::thread::spawn(move || {
        let t = Duration::from_secs(timeout.max(1));
        let mut skipped = Vec::new();
        let roots: Vec<PathBuf> = opts
            .roots
            .iter()
            .filter(|r| match reach::check(r, t) {
                Reachability::NotARepo | Reachability::Repo => true,
                _ => {
                    skipped.push(r.display().to_string());
                    false
                }
            })
            .cloned()
            .collect();
        let opts = ScanOptions { roots, ..opts };
        let git = CliGit::new(git_path.as_deref());
        let (w2, s2) = (weak.clone(), state.clone());
        let found = scan(&opts, &git, &cancel, &mut |n, dir| {
            if n % 25 == 0 {
                let (w, s, msg) = (w2.clone(), s2.clone(), format!("{n} Ordner geprüft – {}", dir.display()));
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = w.upgrade() {
                        let mut st = s.lock().unwrap();
                        st.scan_progress = msg;
                        apply(&ui, &mut st);
                    }
                });
            }
        });
        let cancelled = cancel.load(Ordering::Relaxed);
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else { return };
            let mut st = state.lock().unwrap();
            let os = st.os;
            let items: Vec<ScanItem> = found
                .into_iter()
                .map(|f| {
                    let class = classify(&st.shared, os, &f.path, f.remote.as_deref());
                    let checked = !matches!(class, ScanClass::Known { .. });
                    ScanItem { path: f.path.display().to_string(), remote: f.remote, class, checked }
                })
                .collect();
            st.scan_progress = format!(
                "{} {} Repos gefunden{}",
                if cancelled { "Abgebrochen:" } else { "Fertig:" },
                items.len(),
                if skipped.is_empty() { String::new() } else { format!(" – nicht erreichbar: {}", skipped.join(", ")) }
            );
            st.scan_items = items;
            st.scanning = false;
            apply(&ui, &mut st);
        });
    });
}
