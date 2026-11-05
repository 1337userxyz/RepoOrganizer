//! Scan-Auswertung: URL-Normalisierung, Klassifikation und Übernahme von Funden.
use crate::config::SharedConfig;
use crate::model::{Location, OsPaths, Repo, Role};
use crate::paths::{resolve, Os, Resolved};
use std::collections::HashMap;
use std::path::Path;

/// Normalisiert Remote-URLs (ssh/https, `.git`-Suffix, Groß-/Kleinschreibung) zu `host/pfad`.
pub fn normalize_url(url: &str) -> String {
    let u = url.trim();
    let rest = if let Some((_, r)) = u.split_once("://") {
        // scheme://[user@]host[:port]/path
        let r = r.split_once('@').map(|(_, h)| h).filter(|_| !r.split('/').next().unwrap_or("").is_empty() && r.split('/').next().unwrap_or("").contains('@')).unwrap_or(r);
        let (host, path) = r.split_once('/').unwrap_or((r, ""));
        let host = host.split(':').next().unwrap_or(host);
        format!("{host}/{path}")
    } else if let Some((hostpart, path)) = u.split_once(':').filter(|(h, _)| !h.contains('/') && h.len() > 1) {
        // scp-artig: [user@]host:path
        let host = hostpart.split_once('@').map(|(_, h)| h).unwrap_or(hostpart);
        format!("{host}/{path}")
    } else {
        u.replace('\\', "/")
    };
    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    rest.trim_start_matches('/').to_lowercase()
}

pub fn slug(name: &str) -> String {
    let s: String = name.trim().to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "repo".into() } else { s }
}

fn norm_path(p: &str) -> String {
    let s = p.replace('\\', "/");
    let s = s.trim_end_matches('/');
    if cfg!(windows) { s.to_lowercase() } else { s.to_string() }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanClass {
    /// Pfad ist bereits als Standort bekannt.
    Known { location_id: String },
    /// Remote passt zu einem vorhandenen Repo → weiterer Standort.
    ExtraLocation { repo_id: String },
    /// Unbekannt → neues Repo vorschlagen.
    New,
}

pub fn classify(cfg: &SharedConfig, os: Os, path: &Path, remote: Option<&str>) -> ScanClass {
    let p = norm_path(&path.to_string_lossy());
    for l in &cfg.locations {
        if let Resolved::Local(lp) = resolve(l, os, &cfg.mappings) {
            if norm_path(&lp.to_string_lossy()) == p {
                return ScanClass::Known { location_id: l.id.clone() };
            }
        }
    }
    if let Some(r) = remote.filter(|r| !r.is_empty()) {
        let n = normalize_url(r);
        if let Some(repo) = cfg.repos.iter().find(|x| !x.remote_url.is_empty() && normalize_url(&x.remote_url) == n) {
            return ScanClass::ExtraLocation { repo_id: repo.id.clone() };
        }
    }
    ScanClass::New
}

#[derive(Debug, Clone)]
pub struct Adopt {
    pub path: String,
    pub remote: Option<String>,
    pub class: ScanClass,
}

fn unique(taken: impl Fn(&str) -> bool, base: &str) -> String {
    let mut id = base.to_string();
    let mut n = 2;
    while taken(&id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

fn set_path(paths: &mut OsPaths, os: Os, p: String) {
    match os {
        Os::Windows => paths.windows = Some(p),
        Os::Linux => paths.linux = Some(p),
        Os::MacOs => paths.macos = Some(p),
    }
}

fn dir_name(path: &str) -> String {
    let s = path.replace('\\', "/");
    s.trim_end_matches('/').rsplit('/').next().unwrap_or("repo").to_string()
}

fn add_location(cfg: &mut SharedConfig, os: Os, repo_id: &str, path: &str, label: String) {
    let id = unique(|i| cfg.locations.iter().any(|l| l.id == i), &format!("{repo_id}-1"));
    let mut paths = OsPaths::default();
    set_path(&mut paths, os, path.to_string());
    let role = if cfg.locations.iter().any(|l| l.repo_id == repo_id) { Role::Backup } else { Role::Primary };
    cfg.locations.push(Location { id, repo_id: repo_id.into(), label, paths, role, auto_fetch: false });
}

/// Übernimmt Funde in die Config; gleiche Remotes eines Batches werden zu einem Repo gruppiert.
/// Gibt die Anzahl neu angelegter Standorte zurück.
pub fn adopt(cfg: &mut SharedConfig, os: Os, items: &[Adopt]) -> usize {
    let mut count = 0;
    let mut batch: HashMap<String, String> = HashMap::new();
    for it in items {
        match &it.class {
            ScanClass::Known { .. } => {}
            ScanClass::ExtraLocation { repo_id } => {
                add_location(cfg, os, repo_id, &it.path, dir_name(&it.path));
                count += 1;
            }
            ScanClass::New => {
                let key = it.remote.as_deref().filter(|r| !r.is_empty()).map(normalize_url);
                let repo_id = match key.as_ref().and_then(|k| batch.get(k)) {
                    Some(id) => id.clone(),
                    None => {
                        let name = dir_name(&it.path);
                        let id = unique(|i| cfg.repos.iter().any(|r| r.id == i), &slug(&name));
                        cfg.repos.push(Repo {
                            id: id.clone(),
                            name,
                            remote_url: it.remote.clone().unwrap_or_default(),
                            tags: vec![],
                            notes: String::new(),
                            favorite: false,
                        });
                        if let Some(k) = key {
                            batch.insert(k, id.clone());
                        }
                        id
                    }
                };
                add_location(cfg, os, &repo_id, &it.path, dir_name(&it.path));
                count += 1;
            }
        }
    }
    count
}

/// Verknüpft einen Ordner mit einem bestehenden Repo.
pub fn link_location(cfg: &mut SharedConfig, os: Os, repo_id: &str, path: &str, label: &str) {
    let label = if label.is_empty() { dir_name(path) } else { label.to_string() };
    add_location(cfg, os, repo_id, path, label);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_normalize_equal() {
        let all = [
            "git@GitHub.com:Foo/Bar.git",
            "https://github.com/foo/bar.git",
            "https://user@github.com/Foo/Bar/",
            "ssh://git@github.com:22/foo/bar.git",
            "https://github.com/foo/bar",
        ];
        for u in all {
            assert_eq!(normalize_url(u), "github.com/foo/bar", "{u}");
        }
        assert_ne!(normalize_url("https://github.com/foo/baz"), "github.com/foo/bar");
    }

    fn cfg() -> SharedConfig {
        let mut c = SharedConfig::default();
        c.repos.push(Repo { id: "bar".into(), name: "Bar".into(), remote_url: "https://github.com/foo/bar.git".into(), tags: vec![], notes: String::new(), favorite: false });
        let mut paths = OsPaths::default();
        set_path(&mut paths, Os::current(), "/x/bar".into());
        c.locations.push(Location { id: "bar-1".into(), repo_id: "bar".into(), label: String::new(), paths, role: Role::Primary, auto_fetch: false });
        c
    }

    #[test]
    fn classify_all_kinds() {
        let c = cfg();
        let os = Os::current();
        assert_eq!(classify(&c, os, Path::new("/x/bar/"), None), ScanClass::Known { location_id: "bar-1".into() });
        assert_eq!(classify(&c, os, Path::new("/y/bar2"), Some("git@github.com:foo/bar.git")), ScanClass::ExtraLocation { repo_id: "bar".into() });
        assert_eq!(classify(&c, os, Path::new("/y/other"), Some("git@github.com:foo/other.git")), ScanClass::New);
        assert_eq!(classify(&c, os, Path::new("/y/none"), None), ScanClass::New);
    }

    #[test]
    fn adopt_groups_batch() {
        let mut c = cfg();
        let os = Os::current();
        let mk = |p: &str, r: &str, class| Adopt { path: p.into(), remote: Some(r.into()), class };
        let items = [
            mk("/a/new", "git@h:o/new.git", ScanClass::New),
            mk("/b/new", "https://h/o/new", ScanClass::New),
            mk("/y/bar2", "https://github.com/foo/bar", ScanClass::ExtraLocation { repo_id: "bar".into() }),
            mk("/x/bar", "x", ScanClass::Known { location_id: "bar-1".into() }),
        ];
        assert_eq!(adopt(&mut c, os, &items), 3);
        assert_eq!(c.repos.len(), 2);
        assert_eq!(c.locations_of("new").count(), 2);
        assert_eq!(c.locations_of("bar").count(), 2);
        let ids: std::collections::HashSet<_> = c.locations.iter().map(|l| l.id.clone()).collect();
        assert_eq!(ids.len(), c.locations.len());
    }
}
