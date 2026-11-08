//! Pfad-Auflösung je Betriebssystem inkl. optionaler Mappings und automatischer Pfad-Erkennung.
use crate::model::Location;
pub use kit_paths::{auto_map_from, folder_or_parent, map_from, norm, Os, PathMapping};
use std::path::{PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// Pfad für das aktuelle OS.
    Local(PathBuf),
    /// Kein Pfad für dieses OS hinterlegt (nur andere Systeme).
    OtherOs,
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

