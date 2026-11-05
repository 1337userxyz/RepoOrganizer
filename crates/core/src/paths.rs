//! Pfad-Auflösung je Betriebssystem inkl. optionaler Mappings.
use crate::model::Location;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Linux,
    MacOs,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Linux
        }
    }
}

/// Präfix-Mapping, z. B. `R:\` ↔ `/mnt/r/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathMapping {
    pub windows: String,
    pub unix: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// Pfad für das aktuelle OS.
    Local(PathBuf),
    /// Kein Pfad für dieses OS hinterlegt (nur andere Systeme).
    OtherOs,
}

fn norm(s: &str) -> String {
    s.replace('\\', "/").to_lowercase()
}

/// Wendet Mappings an, um einen Pfad eines anderen OS ins Ziel-OS zu übersetzen.
fn map_from(os_from: Os, os_to: Os, path: &str, maps: &[PathMapping]) -> Option<String> {
    for m in maps {
        let (from, to) = match (os_from, os_to) {
            (Os::Windows, Os::Windows) | (Os::Windows, _) => (&m.windows, &m.unix),
            (_, Os::Windows) => (&m.unix, &m.windows),
            _ => continue,
        };
        let (np, nf) = (norm(path), norm(from));
        if np.starts_with(&nf) {
            let rest = &path[from.len()..];
            let rest = if os_to == Os::Windows {
                rest.replace('/', "\\")
            } else {
                rest.replace('\\', "/")
            };
            return Some(format!("{to}{rest}"));
        }
    }
    None
}

pub fn path_for(loc: &Location, os: Os) -> Option<&String> {
    match os {
        Os::Windows => loc.paths.windows.as_ref(),
        Os::Linux => loc.paths.linux.as_ref(),
        Os::MacOs => loc.paths.macos.as_ref().or(loc.paths.linux.as_ref()),
    }
}

pub fn resolve(loc: &Location, os: Os, maps: &[PathMapping]) -> Resolved {
    if let Some(p) = path_for(loc, os) {
        return Resolved::Local(PathBuf::from(p));
    }
    let others = [Os::Windows, Os::Linux, Os::MacOs];
    for o in others.into_iter().filter(|o| *o != os) {
        if let Some(p) = path_for(loc, o) {
            if let Some(mapped) = map_from(o, os, p, maps) {
                return Resolved::Local(PathBuf::from(mapped));
            }
        }
    }
    Resolved::OtherOs
}
