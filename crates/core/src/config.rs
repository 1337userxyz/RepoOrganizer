//! Geteilte (`repos.toml`) und lokale (`local.toml`) Konfiguration.
use crate::model::{Location, Repo};
use crate::paths::PathMapping;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const CURRENT_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("I/O-Fehler bei {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("Parse-Fehler in {path}: {source}")]
    Parse { path: PathBuf, source: toml::de::Error },
    #[error("Serialisierungsfehler: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("Config-Version {found} ist neuer als unterstützt ({supported})")]
    TooNew { found: u32, supported: u32 },
}

/// Gespeicherter Filter einer smarten Ansicht (geteilt).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedView {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub status: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub older_than_days: Option<u32>,
}

/// Konfigurierbare Standard-Basispfade (geteilt oder je Betriebssystem).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BasePaths {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_windows: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_unix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_linux: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_windows: Option<String>,
}

impl BasePaths {
    pub fn is_empty(&self) -> bool {
        self.shared_windows.is_none() && self.shared_unix.is_none() && self.local_linux.is_none() && self.local_windows.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedConfig {
    pub version: u32,
    #[serde(default, rename = "mapping", skip_serializing_if = "Vec::is_empty")]
    pub mappings: Vec<PathMapping>,
    #[serde(default, skip_serializing_if = "BasePaths::is_empty")]
    pub base_paths: BasePaths,
    #[serde(default, rename = "repo", skip_serializing_if = "Vec::is_empty")]
    pub repos: Vec<Repo>,
    #[serde(default, rename = "location", skip_serializing_if = "Vec::is_empty")]
    pub locations: Vec<Location>,
    #[serde(default, rename = "view", skip_serializing_if = "Vec::is_empty")]
    pub views: Vec<SavedView>,
}

impl Default for SharedConfig {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            mappings: vec![],
            base_paths: BasePaths::default(),
            repos: vec![],
            locations: vec![],
            views: vec![],
        }
    }
}

impl SharedConfig {
    /// Stabile Sortierung für saubere Diffs.
    pub fn normalize(&mut self) {
        self.repos.sort_by(|a, b| a.id.cmp(&b.id));
        self.locations.sort_by(|a, b| (&a.repo_id, &a.id).cmp(&(&b.repo_id, &b.id)));
        self.views.sort_by(|a, b| a.name.cmp(&b.name));
        for r in &mut self.repos {
            r.tags.sort();
            r.tags.dedup();
        }
    }

    pub fn locations_of<'a>(&'a self, repo_id: &'a str) -> impl Iterator<Item = &'a Location> {
        self.locations.iter().filter(move |l| l.repo_id == repo_id)
    }

    /// Stabiler Pfad im Programmverzeichnis (`repos.toml`).
    pub fn program_dir_path() -> PathBuf {
        if let Ok(cwd) = std::env::current_dir() {
            if cwd.join("repos.toml").exists() || cwd.join("Cargo.toml").exists() {
                return cwd.join("repos.toml");
            }
        }
        if let Ok(exe) = std::env::current_exe() {
            for anc in exe.ancestors().skip(1) {
                if anc.join("repos.toml").exists() || anc.join("Cargo.toml").exists() {
                    return anc.join("repos.toml");
                }
            }
            if let Some(dir) = exe.parent() {
                return dir.join("repos.toml");
            }
        }
        PathBuf::from("repos.toml")
    }

    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = read(path)?;
        let cfg: SharedConfig = toml::from_str(&text).map_err(|source| ConfigError::Parse { path: path.into(), source })?;
        if cfg.version > CURRENT_VERSION {
            return Err(ConfigError::TooNew { found: cfg.version, supported: CURRENT_VERSION });
        }
        Ok(cfg)
    }

    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let mut c = self.clone();
        c.version = CURRENT_VERSION;
        c.normalize();
        write_atomic(path, &toml::to_string_pretty(&c)?)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalConfig {
    pub theme: String,
    pub git_path: Option<String>,
    pub git_user: String,
    pub refresh_interval_secs: u64,
    pub reach_timeout_secs: u64,
    pub window_width: u32,
    pub window_height: u32,
    /// Ort der geteilten `repos.toml`.
    pub shared_config: Option<PathBuf>,
    pub health: crate::health::HealthSettings,
    pub stale_days: u32,
}

impl Default for LocalConfig {
    fn default() -> Self {
        Self {
            theme: "system".into(),
            git_path: None,
            git_user: String::new(),
            refresh_interval_secs: 300,
            reach_timeout_secs: 3,
            window_width: 1280,
            window_height: 800,
            shared_config: None,
            health: crate::health::HealthSettings::default(),
            stale_days: 30,
        }
    }
}

impl LocalConfig {
    /// `local.toml` im OS-Config-Verzeichnis (Bootstrap).
    pub fn default_path() -> Option<PathBuf> {
        kit_paths::appdirs::config_dir("RepoOrganizer").map(|d| d.join("local.toml"))
    }

    /// Fehlt die Datei, werden Defaults geliefert.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = read(path)?;
        toml::from_str(&text).map_err(|source| ConfigError::Parse { path: path.into(), source })
    }

    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        write_atomic(path, &toml::to_string_pretty(self)?)
    }

    /// Ort der geteilten Config; Fallback: neben `local.toml`.
    pub fn shared_path(&self, local_path: &Path) -> PathBuf {
        self.shared_config.clone().unwrap_or_else(|| local_path.with_file_name("repos.toml"))
    }

    /// Leitet aus `git_user` (z. B. `mein-user` oder `https://github.com/mein-user`) und Repo-Namen die Remote-URL ab.
    pub fn derive_remote_url(&self, repo_name: &str) -> String {
        derive_remote_url(&self.git_user, repo_name)
    }
}

/// Leitet aus einem Git-Benutzernamen (oder einer Basis-URL) und dem Repository-Namen automatisch die Remote-URL ab.
pub fn derive_remote_url(git_user: &str, repo_name: &str) -> String {
    let u = git_user.trim().trim_end_matches('/');
    let n = repo_name.trim().trim_end_matches(".git");
    if u.is_empty() || n.is_empty() {
        return String::new();
    }
    if u.starts_with("http://") || u.starts_with("https://") || u.starts_with("ssh://") || u.starts_with("git@") {
        format!("{u}/{n}.git")
    } else if u.contains('/') {
        format!("https://{u}/{n}.git")
    } else {
        format!("https://github.com/{u}/{n}.git")
    }
}

/// Versucht den Git-User aus bereits vorhandenen Remote-URLs zu erkennen (z. B. `https://github.com/<user>/<repo>.git`).
pub fn detect_user_from_url(url: &str) -> Option<String> {
    let u = url.trim().trim_end_matches('/').trim_end_matches(".git");
    if let Some(rest) = u.strip_prefix("https://").or_else(|| u.strip_prefix("http://")) {
        let parts: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
        if parts.len() >= 3 {
            return Some(parts[1].to_string());
        }
    } else if let Some((_, path)) = u.split_once(':') {
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if parts.len() >= 2 {
            return Some(parts[0].to_string());
        }
    }
    None
}

fn read(path: &Path) -> Result<String, ConfigError> {
    std::fs::read_to_string(path).map_err(|source| ConfigError::Io { path: path.into(), source })
}

/// Atomar schreiben: Temp-Datei + Rename.
pub fn write_atomic(path: &Path, content: &str) -> Result<(), ConfigError> {
    let io = |source| ConfigError::Io { path: path.into(), source };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, content).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(io)
}

