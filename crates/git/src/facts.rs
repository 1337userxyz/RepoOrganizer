//! Sammeln der Fakten für Health-Checks und Drift-Vergleich.
use crate::{parse_status_v2, CliGit};
use repoorganizer_core::health::Facts;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct FactOptions {
    pub check_remote: bool,
    pub secret_scan: bool,
    /// Dateien ab dieser Größe werden als „groß“ gemeldet.
    pub large_file_min_bytes: u64,
    pub remote_timeout: Duration,
}

impl Default for FactOptions {
    fn default() -> Self {
        Self { check_remote: false, secret_scan: false, large_file_min_bytes: 50 * 1024 * 1024, remote_timeout: Duration::from_secs(10) }
    }
}

/// Führt einen Befehl mit Timeout aus; `Some(success)` oder `None` bei Start-/Timeout-Fehler.
fn run_timeout(mut cmd: Command, timeout: Duration) -> Option<bool> {
    let mut child = cmd.env("GIT_TERMINAL_PROMPT", "0").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) => return Some(st.success()),
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Gesamtgröße eines Verzeichnisbaums (Bytes), Symlinks werden nicht verfolgt.
fn dir_size(p: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(p) else { return 0 };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

fn mtime(p: &Path) -> Option<i64> {
    Some(std::fs::metadata(p).ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64)
}

impl CliGit {
    /// Vergleicht HEAD von `a` und `b` lokal: (Commits in a, die b fehlen; Commits in b, die a fehlen).
    /// Holt dafür die Objekte von `b` nach `a` (ohne FETCH_HEAD/Refs zu verändern).
    pub fn compare_locations(&self, a: &Path, b: &Path) -> Option<(u32, u32)> {
        let sha_b = self.run(b, &["rev-parse", "HEAD"]).ok()?.trim().to_string();
        let b_str = b.to_string_lossy().into_owned();
        self.run(a, &["fetch", "--quiet", "--no-write-fetch-head", &b_str, "HEAD"]).ok()?;
        self.left_right(a, "HEAD", &sha_b).ok()
    }

    /// Commits der letzten `days` Tage über alle Branches (ohne Merges) inkl. Zeilenstatistik.
    pub fn collect_activity(&self, repo: &Path, days: u32) -> Vec<repoorganizer_core::stats::CommitInfo> {
        let since = format!("--since={days}.days");
        let out = self.run(repo, &["log", "--all", "--no-merges", &since, "--numstat", "--format=%x01%ct%x01%an"]).unwrap_or_default();
        crate::parse::parse_log_numstat(&out)
    }

    /// Größe des `.git`-Verzeichnisses in Bytes.
    pub fn git_size(&self, repo: &Path) -> u64 {
        self.run(repo, &["rev-parse", "--absolute-git-dir"]).map(|d| dir_size(Path::new(d.trim()))).unwrap_or(0)
    }

    pub fn collect_facts(&self, repo: &Path, o: &FactOptions) -> Facts {
        let mut f = Facts::default();
        if let Ok(out) = self.run(repo, &["status", "--porcelain=v2", "--branch"]) {
            let s = parse_status_v2(&out);
            f.detached = s.branch.is_none() && s.head_sha.is_some();
            f.has_upstream = s.upstream.is_some();
            f.ahead = s.ahead;
            f.conflicts = s.conflicts;
            f.branch = s.branch;
        }
        if let Ok(out) = self.run(repo, &["for-each-ref", "--format=%(refname:short)|%(upstream:short)", "refs/heads"]) {
            f.branches_without_upstream = out.lines().filter(|l| l.split_once('|').map(|(_, u)| u.is_empty()).unwrap_or(false)).count() as u32;
        }
        f.has_remote = self.run(repo, &["remote"]).map(|s| !s.trim().is_empty()).unwrap_or(false);
        f.last_fetch = self.last_fetch(repo);
        f.last_pull = self.last_pull(repo);
        f.last_commit = self.last_commit(repo);

        if let Ok(out) = self.run(repo, &["status", "--porcelain"]) {
            f.oldest_dirty = out
                .lines()
                .filter_map(|l| {
                    let p = l.get(3..)?;
                    let p = p.rsplit(" -> ").next()?.trim_matches('"');
                    mtime(&repo.join(p))
                })
                .min();
        }
        if let Ok(out) = self.run(repo, &["ls-files", "-z"]) {
            for p in out.split('\0').filter(|p| !p.is_empty()).take(200_000) {
                if let Ok(m) = std::fs::metadata(repo.join(p)) {
                    if m.len() >= o.large_file_min_bytes {
                        f.large_files.push((p.to_string(), m.len()));
                    }
                }
            }
            f.large_files.sort_by(|a, b| b.1.cmp(&a.1));
        }
        if let Ok(dir) = self.run(repo, &["rev-parse", "--absolute-git-dir"]) {
            f.git_size_bytes = dir_size(Path::new(dir.trim()));
        }
        let arts = repoorganizer_core::clean::find_artifacts(repo);
        f.build_size_bytes = arts.total_bytes;
        f.build_artifacts_count = arts.items.len() as u32;
        if let Ok(rd) = std::fs::read_dir(repo) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_lowercase();
                f.has_gitignore |= n == ".gitignore";
                f.has_readme |= n.starts_with("readme");
            }
        }
        if let Ok(dir) = self.run(repo, &["rev-parse", "--git-dir"]) {
            let d = repo.join(dir.trim());
            f.in_progress = if d.join("MERGE_HEAD").exists() {
                Some("Merge".into())
            } else if d.join("rebase-merge").exists() || d.join("rebase-apply").exists() {
                Some("Rebase".into())
            } else if d.join("CHERRY_PICK_HEAD").exists() {
                Some("Cherry-Pick".into())
            } else if d.join("REVERT_HEAD").exists() {
                Some("Revert".into())
            } else {
                None
            };
        }
        if o.check_remote && f.has_remote {
            let mut c = Command::new(&self.git_path);
            c.arg("-C").arg(repo).args(["ls-remote", "--exit-code", "origin", "HEAD"]);
            f.remote_ok = Some(run_timeout(c, o.remote_timeout).unwrap_or(false));
        }
        if o.secret_scan {
            let mut c = Command::new("gitleaks");
            c.args(["detect", "--no-banner", "--exit-code", "1", "-s"]).arg(repo);
            // Startfehler (nicht installiert) → None; Fund → Exit-Code 1 → Some(true)
            f.secrets = run_timeout(c, Duration::from_secs(120)).map(|ok| !ok);
        }
        f
    }
}
