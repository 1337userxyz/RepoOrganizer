//! Erreichbarkeits-Check mit Timeout in eigenem Thread (Netzlaufwerke, tote Mounts).
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reachability {
    NotARepo,
    Repo,
    Unavailable,
    TimedOut,
}

pub fn check(path: &Path, timeout: Duration) -> Reachability {
    let (tx, rx) = mpsc::channel();
    let p: PathBuf = path.to_path_buf();
    thread::spawn(move || {
        let r = if !p.is_dir() {
            Reachability::Unavailable
        } else if p.join(".git").exists() {
            Reachability::Repo
        } else {
            Reachability::NotARepo
        };
        let _ = tx.send(r);
    });
    rx.recv_timeout(timeout).unwrap_or(Reachability::TimedOut)
}
