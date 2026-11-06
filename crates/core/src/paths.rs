//! Pfad-Auflösung je Betriebssystem inkl. optionaler Mappings und automatischer Pfad-Erkennung.
use crate::model::Location;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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
pub fn map_from(os_from: Os, os_to: Os, path: &str, maps: &[PathMapping]) -> Option<String> {
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

/// Versucht ohne explizites Mapping intelligente Pfad-Konvertierungen:
/// 1. Slashes umkehren (`\` <-> `/`)
/// 2. Windows-Laufwerksbuchstaben zu Unix (`R:\git\...` -> `/r/git/...`, `/mnt/r/git/...`, `/media/r/git/...`)
/// 3. Unix-Pfade zu Windows-Laufwerksbuchstaben (`/r/git/...`, `/mnt/r/git/...` -> `R:\git\...`)
pub fn auto_map_from(os_from: Os, os_to: Os, path: &str) -> Option<PathBuf> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return None;
    }

    match (os_from, os_to) {
        (Os::Windows, Os::Linux) | (Os::Windows, Os::MacOs) => {
            // Prüfen: beginnt mit Laufwerksbuchstabe z. B. "R:\" oder "R:/"
            let mut chars = trimmed.chars();
            let c1 = chars.next()?;
            let c2 = chars.next()?;
            if c1.is_ascii_alphabetic() && c2 == ':' {
                let drive_lower = c1.to_ascii_lowercase();
                let drive_upper = c1.to_ascii_uppercase();
                let rest = trimmed[2..].replace('\\', "/");
                let rest_clean = rest.trim_start_matches('/');

                let candidates = [
                    format!("/{drive_lower}/{rest_clean}"),
                    format!("/mnt/{drive_lower}/{rest_clean}"),
                    format!("/media/{drive_lower}/{rest_clean}"),
                    format!("/{drive_upper}/{rest_clean}"),
                ];

                // 1. Wenn ein Pfad tatsächlich existiert -> direkte Auflösung!
                for cand in &candidates {
                    if Path::new(cand).exists() {
                        return Some(PathBuf::from(cand));
                    }
                }

                return None;
            }

            // Slashes umkehren falls Pfad existiert
            let flipped = trimmed.replace('\\', "/");
            if Path::new(&flipped).exists() {
                return Some(PathBuf::from(flipped));
            }

            None
        }
        (Os::Linux, Os::Windows) | (Os::MacOs, Os::Windows) => {
            let flipped = trimmed.replace('/', "\\");
            if Path::new(&flipped).exists() {
                return Some(PathBuf::from(flipped));
            }

            // z. B. /mnt/r/..., /media/r/... oder /r/...
            let rest_after_mnt = trimmed.strip_prefix("/mnt/").or_else(|| trimmed.strip_prefix("/media/"));
            let (drive_char, rest_path) = if let Some(after_mnt) = rest_after_mnt {
                let mut parts = after_mnt.splitn(2, '/');
                let d = parts.next()?;
                if d.len() == 1 && d.chars().next()?.is_ascii_alphabetic() {
                    (d.chars().next()?, parts.next().unwrap_or(""))
                } else {
                    return None;
                }
            } else if trimmed.starts_with('/') {
                let mut parts = trimmed[1..].splitn(2, '/');
                let d = parts.next()?;
                if d.len() == 1 && d.chars().next()?.is_ascii_alphabetic() {
                    (d.chars().next()?, parts.next().unwrap_or(""))
                } else {
                    return None;
                }
            } else {
                return None;
            };

            let drive_upper = drive_char.to_ascii_uppercase();
            let win_rest = rest_path.replace('/', "\\");
            let win_path = format!("{drive_upper}:\\{win_rest}");
            if Path::new(&win_path).exists() {
                return Some(PathBuf::from(win_path));
            }

            None
        }
        _ => {
            let flipped = if os_to == Os::Windows {
                trimmed.replace('/', "\\")
            } else {
                trimmed.replace('\\', "/")
            };
            if Path::new(&flipped).exists() {
                Some(PathBuf::from(flipped))
            } else {
                None
            }
        }
    }
}

pub fn path_for<'a>(loc: &'a Location, os: Os) -> Option<&'a String> {
    let p = match os {
        Os::Windows => loc.paths.windows.as_ref(),
        Os::Linux => loc.paths.linux.as_ref(),
        Os::MacOs => loc.paths.macos.as_ref().or(loc.paths.linux.as_ref()),
    };
    p.or(loc.paths.path.as_ref())
}

pub fn resolve(loc: &Location, os: Os, maps: &[PathMapping]) -> Resolved {
    if let Some(p) = path_for(loc, os) {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Resolved::Local(pb);
        }
        // Prüfen, ob Slashes oder Laufwerksbuchstaben auf dem aktuellen System korrigiert werden können
        if let Some(auto) = auto_map_from(match os { Os::Windows => Os::Linux, _ => Os::Windows }, os, p) {
            if auto.exists() {
                return Resolved::Local(auto);
            }
        }
        return Resolved::Local(pb);
    }
    let others = [Os::Windows, Os::Linux, Os::MacOs];
    for o in others.into_iter().filter(|o| *o != os) {
        if let Some(p) = path_for(loc, o) {
            if let Some(mapped) = map_from(o, os, p, maps) {
                return Resolved::Local(PathBuf::from(mapped));
            }
            if let Some(auto) = auto_map_from(o, os, p) {
                return Resolved::Local(auto);
            }
        }
    }
    Resolved::OtherOs
}
