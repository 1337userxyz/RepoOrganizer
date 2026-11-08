//! Git-CLI-Backend: Status, Ahead/Behind, Last-Fetch/Pull.
use repoorganizer_core::model::{Snapshot, Status};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

mod parse;
pub mod facts;
pub mod scan;
pub use parse::{parse_status_v2, StatusInfo};

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("Git konnte nicht gestartet werden: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("git {args} fehlgeschlagen: {stderr}")]
    Failed { args: String, stderr: String },
}

/// Austauschbares Backend (CLI heute, evtl. libgit2 später).
pub trait GitBackend: Send + Sync {
    fn status(&self, repo: &Path) -> Result<StatusInfo, GitError>;
    fn snapshot(&self, location_id: &str, repo: &Path) -> Snapshot;
}

pub struct CliGit {
    pub git_path: PathBuf,
}

impl Default for CliGit {
    fn default() -> Self {
        Self { git_path: "git".into() }
    }
}

impl CliGit {
    pub fn new(git_path: Option<&str>) -> Self {
        Self { git_path: git_path.map(PathBuf::from).unwrap_or_else(|| "git".into()) }
    }

    fn run(&self, repo: &Path, args: &[&str]) -> Result<String, GitError> {
        let out = Command::new(&self.git_path).args(["-c", "safe.directory=*"]).arg("-C").arg(repo).args(args).output()?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(GitError::Failed { args: args.join(" "), stderr: String::from_utf8_lossy(&out.stderr).trim().into() })
        }
    }

    /// Commits, die `a` gegenüber `b` voraus bzw. zurück ist: (links, rechts).
    pub fn left_right(&self, repo: &Path, a: &str, b: &str) -> Result<(u32, u32), GitError> {
        let out = self.run(repo, &["rev-list", "--left-right", "--count", &format!("{a}...{b}")])?;
        parse::parse_left_right(&out).ok_or(GitError::Failed { args: "rev-list".into(), stderr: format!("unerwartete Ausgabe: {out}") })
    }

    pub fn stash_count(&self, repo: &Path) -> u32 {
        self.run(repo, &["stash", "list"]).map(|s| s.lines().count() as u32).unwrap_or(0)
    }

    /// mtime von `.git/FETCH_HEAD` (Unix-Sekunden).
    pub fn last_fetch(&self, repo: &Path) -> Option<i64> {
        let dir = self.run(repo, &["rev-parse", "--git-dir"]).ok()?;
        let dir = repo.join(dir.trim());
        mtime(&dir.join("FETCH_HEAD"))
    }

    /// Autorzeit des letzten Commits (Unix-Sekunden).
    pub fn last_commit(&self, repo: &Path) -> Option<i64> {
        self.run(repo, &["log", "-1", "--format=%ct"]).ok()?.trim().parse().ok()
    }

    /// Gesamtanzahl Commits auf HEAD.
    pub fn commit_count(&self, repo: &Path) -> u32 {
        self.run(repo, &["rev-list", "--count", "HEAD"]).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
    }

    /// Anzahl getrackter Dateien im Arbeitsbaum.
    pub fn file_count(&self, repo: &Path) -> u32 {
        self.run(repo, &["ls-files"]).map(|s| s.lines().count() as u32).unwrap_or(0)
    }

    /// Liefert alle uncommitteten/untracked/geänderten Dateien mit Status und letzter Änderungszeit.
    pub fn dirty_files(&self, repo: &Path) -> Vec<repoorganizer_core::model::DirtyFile> {
        let Ok(out) = self.run(repo, &["status", "--porcelain=v1", "-u"]) else {
            return Vec::new();
        };
        parse::parse_porcelain_dirty(&out, repo)
    }

    /// Zeitpunkt des letzten Pulls aus dem Reflog von HEAD (Eintrag `pull`).
    pub fn last_pull(&self, repo: &Path) -> Option<i64> {
        let out = self.run(repo, &["reflog", "show", "--date=unix", "--format=%gd|%gs", "HEAD"]).ok()?;
        parse::parse_last_pull(&out)
    }
}

/// Wie `pull` Änderungen integriert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PullMode {
    #[default]
    FfOnly,
    Rebase,
    Merge,
}

/// Pull ist bei `Diverged`/`Dirty` blockiert (mit Hinweistext).
pub fn guard_pull(s: &Status) -> Result<(), String> {
    match s {
        Status::Diverged => Err("Pull blockiert: Standort ist divergiert (erst manuell auflösen).".into()),
        Status::Dirty => Err("Pull blockiert: uncommittete Änderungen vorhanden.".into()),
        Status::Unavailable | Status::NotARepo | Status::Error(_) => Err("Standort nicht verfügbar.".into()),
        _ => Ok(()),
    }
}

pub fn guard_push(s: &Status) -> Result<(), String> {
    match s {
        Status::Diverged => Err("Push blockiert: Standort ist divergiert.".into()),
        Status::Unavailable | Status::NotARepo | Status::Error(_) => Err("Standort nicht verfügbar.".into()),
        _ => Ok(()),
    }
}

impl CliGit {
    pub fn fetch(&self, repo: &Path) -> Result<String, GitError> {
        self.run_all(repo, &["fetch", "--prune"])
    }

    pub fn pull(&self, repo: &Path, mode: PullMode) -> Result<String, GitError> {
        let flag = match mode {
            PullMode::FfOnly => "--ff-only",
            PullMode::Rebase => "--rebase",
            PullMode::Merge => "--no-rebase",
        };
        self.run_all(repo, &["pull", flag])
    }

    pub fn push(&self, repo: &Path) -> Result<String, GitError> {
        self.run_all(repo, &["push", "-u", "origin", "HEAD"])
    }

    pub fn stage(&self, repo: &Path, paths: &[&str]) -> Result<String, GitError> {
        let mut a = vec!["add", "--"];
        a.extend(paths);
        self.run_all(repo, &a)
    }

    pub fn stage_all(&self, repo: &Path) -> Result<String, GitError> {
        self.run_all(repo, &["add", "-A"])
    }

    pub fn unstage(&self, repo: &Path, paths: &[&str]) -> Result<String, GitError> {
        let mut a = vec!["restore", "--staged", "--"];
        a.extend(paths);
        self.run_all(repo, &a)
    }

    pub fn commit(&self, repo: &Path, message: &str) -> Result<String, GitError> {
        if message.trim().is_empty() {
            return Err(GitError::Failed { args: "commit".into(), stderr: "Commit-Nachricht fehlt".into() });
        }
        self.run_all(repo, &["commit", "-m", message])
    }

    pub fn stash(&self, repo: &Path) -> Result<String, GitError> {
        self.run_all(repo, &["stash", "push", "--include-untracked"])
    }

    pub fn stash_pop(&self, repo: &Path) -> Result<String, GitError> {
        self.run_all(repo, &["stash", "pop"])
    }

    pub fn switch_branch(&self, repo: &Path, branch: &str) -> Result<String, GitError> {
        self.run_all(repo, &["switch", branch])
    }

    pub fn trust_directory(&self, repo: &Path) -> Result<String, GitError> {
        let p = repo.to_string_lossy().replace('\\', "/");
        let out = Command::new(&self.git_path).args(["config", "--global", "--add", "safe.directory", &p]).output()?;
        combine(out, "config safe.directory")
    }

    /// Übernimmt den Windows/NTFS-Dateibesitz für den aktuellen Benutzer via `icacls`.
    pub fn take_ownership(&self, repo: &Path) -> Result<String, GitError> {
        if cfg!(windows) {
            let p = repo.to_string_lossy().to_string();
            let user = std::env::var("USERNAME").unwrap_or_else(|_| "CURRENT_USER".into());
            let out = Command::new("icacls")
                .args([&p, "/setowner", &user, "/T", "/C", "/Q"])
                .output()?;
            combine(out, "icacls /setowner")
        } else {
            let out = Command::new("chown")
                .args(["-R", ":", &repo.to_string_lossy()])
                .output()?;
            combine(out, "chown")
        }
    }

    pub fn remote_origin(&self, repo: &Path) -> Option<String> {
        let u = self.run(repo, &["config", "--get", "remote.origin.url"]).ok()?;
        let t = u.trim();
        if t.is_empty() { None } else { Some(t.to_string()) }
    }

    /// Liest global konfigurierten Git-/GitHub-Benutzernamen aus (`github.user` oder `user.name`).
    pub fn global_git_user(&self) -> Option<String> {
        for key in ["github.user", "user.username", "user.name"] {
            if let Ok(out) = Command::new(&self.git_path).args(["config", "--global", "--get", key]).output() {
                if out.status.success() {
                    let val = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if !val.is_empty() && !val.contains(' ') {
                        return Some(val);
                    }
                }
            }
        }
        None
    }

    pub fn set_remote(&self, repo: &Path, url: &str) -> Result<String, GitError> {
        let url = url.trim();
        if url.is_empty() {
            return Err(GitError::Failed { args: "remote".into(), stderr: "Remote-URL fehlt".into() });
        }
        if self.remote_origin(repo).is_some() {
            self.run_all(repo, &["remote", "set-url", "origin", url])
        } else {
            self.run_all(repo, &["remote", "add", "origin", url])
        }
    }

    pub fn set_remote_and_push(&self, repo: &Path, url: &str) -> Result<String, GitError> {
        self.set_remote(repo, url)?;
        match self.run_all(repo, &["push", "-u", "origin", "HEAD"]) {
            Ok(out) => Ok(out),
            Err(GitError::Failed { stderr, .. }) if stderr.contains("Repository not found") || stderr.contains("not found") => {
                if url.contains("github.com") {
                    let trimmed = url.trim().trim_end_matches(".git").trim_end_matches('/');
                    let name = trimmed.rsplit('/').next().unwrap_or("");
                    if !name.is_empty() {
                        let _ = self.create_github_repo_api(name, false)?;
                        return self.run_all(repo, &["push", "-u", "origin", "HEAD"]);
                    }
                }
                Err(GitError::Failed { args: "push".into(), stderr })
            }
            Err(e) => Err(e),
        }
    }

    /// Fragt Git Credential Manager nach Zugangsdaten für github.com (Liefert Personal Access Token / OAuth Token).
    pub fn github_token(&self) -> Option<String> {
        use std::io::Write;
        let mut child = Command::new(&self.git_path)
            .args(["credential", "fill"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(b"protocol=https\nhost=github.com\n\n");
        }
        let out = child.wait_with_output().ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            if let Some(pw) = line.strip_prefix("password=") {
                let token = pw.trim();
                if !token.is_empty() {
                    return Some(token.to_string());
                }
            }
        }
        None
    }

    /// Erstellt ein GitHub-Repository direkt über die GitHub REST API (benötigt kein `gh` CLI).
    pub fn create_github_repo_api(&self, name: &str, private: bool) -> Result<String, GitError> {
        let token = self.github_token().ok_or_else(|| GitError::Failed {
            args: "github api".into(),
            stderr: "Kein GitHub-Token im Git Credential Manager gefunden. Bitte Git-Anmeldedaten für github.com hinterlegen oder GitHub CLI (gh) installieren.".into(),
        })?;
        let payload = format!(r#"{{"name":"{}","private":{}}}"#, name.trim(), private);
        let out = Command::new("curl")
            .args([
                "-s",
                "-X", "POST",
                "https://api.github.com/user/repos",
                "-H", &format!("Authorization: Bearer {token}"),
                "-H", "User-Agent: RepoOrganizer",
                "-H", "Accept: application/vnd.github.v3+json",
                "-H", "Content-Type: application/json",
                "-d", &payload,
            ])
            .output()?;
        let resp = String::from_utf8_lossy(&out.stdout).to_string();
        if resp.contains("\"id\":") || resp.contains("already exists") {
            Ok(resp)
        } else {
            let err_msg = if let Some(msg_start) = resp.find("\"message\":\"") {
                let rest = &resp[msg_start + 11..];
                rest.split('"').next().unwrap_or(&resp)
            } else {
                &resp
            };
            Err(GitError::Failed {
                args: "github api create repo".into(),
                stderr: format!("GitHub API Fehler: {err_msg}"),
            })
        }
    }

    pub fn publish_github(&self, repo: &Path, name: &str, private: bool) -> Result<String, GitError> {
        let vis = if private { "--private" } else { "--public" };
        // 1. Zuerst gh probieren, falls installiert
        let gh_res = Command::new("gh")
            .current_dir(repo)
            .args(["repo", "create", name.trim(), vis, "--source=.", "--remote=origin", "--push"])
            .output();

        if let Ok(out) = gh_res {
            if out.status.success() {
                return combine(out, "gh repo create");
            }
        }

        // 2. Fallback: Über GitHub REST API erstellen (mittels Git Credential Manager Token)
        let _ = self.create_github_repo_api(name, private)?;
        let user = self.global_git_user().unwrap_or_else(|| "1337userxyz".into());
        let url = format!("https://github.com/{user}/{}.git", name.trim());
        self.set_remote(repo, &url)?;
        self.run_all(repo, &["push", "-u", "origin", "HEAD"])
    }

    pub fn clone_repo(&self, url: &str, dest: &Path) -> Result<String, GitError> {
        let out = Command::new(&self.git_path).args(["-c", "safe.directory=*"]).arg("clone").arg(url).arg(dest).output()?;
        combine(out, "clone")
    }

    /// Wie `run`, liefert aber stdout+stderr (git schreibt Fortschritt nach stderr).
    fn run_all(&self, repo: &Path, args: &[&str]) -> Result<String, GitError> {
        let out = Command::new(&self.git_path).args(["-c", "safe.directory=*"]).arg("-C").arg(repo).args(args).output()?;
        combine(out, &args.join(" "))
    }
}

fn combine(out: std::process::Output, args: &str) -> Result<String, GitError> {
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)).trim().to_string();
    if out.status.success() {
        Ok(text)
    } else {
        Err(GitError::Failed { args: args.into(), stderr: text })
    }
}

fn mtime(p: &Path) -> Option<i64> {
    let m = std::fs::metadata(p).ok()?.modified().ok()?;
    Some(m.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64)
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

impl GitBackend for CliGit {
    fn status(&self, repo: &Path) -> Result<StatusInfo, GitError> {
        let out = self.run(repo, &["status", "--porcelain=v2", "--branch"])?;
        Ok(parse_status_v2(&out))
    }

    fn snapshot(&self, location_id: &str, repo: &Path) -> Snapshot {
        let mut snap = Snapshot {
            location_id: location_id.into(),
            last_checked: Some(now()),
            last_fetch: None,
            last_pull: None,
            last_commit: None,
            commit_count: 0,
            file_count: 0,
            size_kb: 0,
            build_size_bytes: 0,
            remote_url: None,
            branch: None,
            ahead: 0,
            behind: 0,
            dirty_files: 0,
            untracked: 0,
            stash_count: 0,
            head_sha: None,
            state: Status::Unavailable,
            dirty_details: Vec::new(),
        };
        if !repo.is_dir() {
            return snap;
        }

        // Build-Artefakt-Größe immer ermitteln (auch wenn Git-Status scheitert oder Fehler hat):
        snap.build_size_bytes = repoorganizer_core::clean::find_artifacts(repo).total_bytes;

        if !repo.join(".git").exists() {
            snap.state = Status::NotARepo;
            return snap;
        }
        match self.status(repo) {
            Ok(s) => {
                snap.branch = s.branch.clone();
                snap.head_sha = s.head_sha.clone();
                snap.ahead = s.ahead;
                snap.behind = s.behind;
                snap.dirty_files = s.changed;
                snap.untracked = s.untracked;
                if snap.dirty_files + snap.untracked > 0 {
                    snap.dirty_details = self.dirty_files(repo);
                }
                snap.stash_count = self.stash_count(repo);
                snap.last_fetch = self.last_fetch(repo);
                snap.last_pull = self.last_pull(repo);
                snap.last_commit = self.last_commit(repo);
                snap.commit_count = self.commit_count(repo);
                snap.file_count = self.file_count(repo);
                snap.size_kb = self.git_size(repo) / 1024;
                snap.remote_url = self.remote_origin(repo);
                if s.upstream.is_none() && snap.remote_url.is_some() && snap.commit_count > 0 {
                    snap.ahead = snap.commit_count;
                }
                snap.state = if s.changed + s.untracked > 0 {
                    Status::Dirty
                } else {
                    match (snap.ahead, snap.behind) {
                        (0, 0) => Status::UpToDate,
                        (a, 0) => Status::Ahead(a),
                        (0, b) => Status::Behind(b),
                        _ => Status::Diverged,
                    }
                };
            }
            Err(e) => snap.state = Status::Error(e.to_string()),
        }
        snap
    }
}

/// Snapshots mehrerer Standorte parallel (je Standort ein Thread, max. `jobs` gleichzeitig).
pub fn snapshots_parallel(git: &dyn GitBackend, items: &[(String, PathBuf)], jobs: usize) -> Vec<Snapshot> {
    let jobs = jobs.max(1);
    let mut result = Vec::with_capacity(items.len());
    for chunk in items.chunks(jobs) {
        std::thread::scope(|s| {
            let handles: Vec<_> = chunk.iter().map(|(id, p)| s.spawn(move || git.snapshot(id, p))).collect();
            for h in handles {
                result.push(h.join().expect("Snapshot-Thread abgestürzt"));
            }
        });
    }
    result
}

#[cfg(test)]
mod tests;
