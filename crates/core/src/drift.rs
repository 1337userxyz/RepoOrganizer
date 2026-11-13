//! Drift-Erkennung: Vergleich der Standorte eines Repos.
use crate::level::Level;

#[derive(Debug, Clone)]
pub struct DriftLoc {
    pub label: String,
    pub reachable: bool,
    /// Zeitpunkt des letzten bekannten Snapshots (bei Offline-Standorten).
    pub stale_at: Option<i64>,
    pub branch: Option<String>,
    pub head_sha: Option<String>,
    pub dirty: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub level: Level,
    pub text: String,
}

/// `rel(i, j)` liefert (Commits in i, die j fehlen; Commits in j, die i fehlen).
pub fn drift_messages(locs: &[DriftLoc], rel: &mut dyn FnMut(usize, usize) -> Option<(u32, u32)>, _now: i64) -> Vec<Message> {
    let mut out = Vec::new();
    let mut msg = |level, text: String| out.push(Message { level, text });
    for i in 0..locs.len() {
        for j in (i + 1)..locs.len() {
            let (a, b) = (&locs[i], &locs[j]);
            if !a.reachable || !b.reachable {
                continue;
            }
            let (Some(sa), Some(sb)) = (&a.head_sha, &b.head_sha) else { continue };
            if sa == sb {
                continue;
            }
            if let (Some(ba), Some(bb)) = (&a.branch, &b.branch) {
                if ba != bb {
                    msg(Level::Warn, format!("{} ({ba}) und {} ({bb}) stehen auf verschiedenen Branches", a.label, b.label));
                    continue;
                }
            }
            match rel(i, j) {
                Some((0, n)) if n > 0 => {
                    msg(Level::Warn, format!("{} ist {n} Commits hinter {}", a.label, b.label));
                    if a.dirty {
                        msg(Level::Warn, format!("Dirty-Standort {} verhindert Sync", a.label));
                    }
                }
                Some((n, 0)) if n > 0 => {
                    msg(Level::Warn, format!("{} ist {n} Commits hinter {}", b.label, a.label));
                    if b.dirty {
                        msg(Level::Warn, format!("Dirty-Standort {} verhindert Sync", b.label));
                    }
                }
                Some((x, y)) if x > 0 && y > 0 => msg(Level::Error, format!("{} und {} divergiert", a.label, b.label)),
                _ => msg(Level::Warn, format!("Vergleich {} ↔ {} nicht möglich", a.label, b.label)),
            }
        }
    }
    out
}
