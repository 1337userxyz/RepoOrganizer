//! Rekursiver Scan nach Git-Repos (Ordner- oder Datei-`.git` für Worktrees/Submodule).
use crate::CliGit;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub roots: Vec<PathBuf>,
    pub max_depth: usize,
    pub excludes: Vec<String>,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            roots: vec![],
            max_depth: 4,
            excludes: ["node_modules", "target", ".cache", "$RECYCLE.BIN", "System Volume Information"].iter().map(|s| s.to_string()).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    /// `origin`-URL, falls vorhanden.
    pub remote: Option<String>,
}

/// `progress` erhält (Anzahl besuchter Ordner, aktueller Ordner). Abbruch über `cancel`.
pub fn scan(opts: &ScanOptions, git: &CliGit, cancel: &AtomicBool, progress: &mut dyn FnMut(usize, &Path)) -> Vec<Found> {
    let mut found = Vec::new();
    let mut visited = 0usize;
    for root in &opts.roots {
        walk(root, 0, opts, cancel, &mut visited, progress, &mut found);
    }
    for f in &mut found {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        f.remote = git.run(&f.path, &["config", "--get", "remote.origin.url"]).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found.dedup_by(|a, b| a.path == b.path);
    found
}

fn walk(dir: &Path, depth: usize, o: &ScanOptions, cancel: &AtomicBool, visited: &mut usize, progress: &mut dyn FnMut(usize, &Path), out: &mut Vec<Found>) {
    if cancel.load(Ordering::Relaxed) {
        return;
    }
    *visited += 1;
    progress(*visited, dir);
    if dir.join(".git").exists() {
        out.push(Found { path: dir.to_path_buf(), remote: None });
        return; // nicht in Repos hinabsteigen
    }
    if depth >= o.max_depth {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        if !ft.is_dir() || ft.is_symlink() {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if o.excludes.iter().any(|x| x.eq_ignore_ascii_case(&name)) {
            continue;
        }
        walk(&e.path(), depth + 1, o, cancel, visited, progress, out);
    }
}
