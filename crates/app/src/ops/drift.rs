use std::collections::HashMap;
use std::path::PathBuf;

use repoorganizer_core::drift::{drift_messages, DriftLoc, Message};
use repoorganizer_core::model::{Snapshot, Status};
use repoorganizer_git::CliGit;

use crate::state::{now, offline_state};

pub fn compute_drift(
    git: &CliGit,
    groups: &[(String, Vec<(String, String, Option<PathBuf>)>)],
    snaps: &HashMap<String, Snapshot>,
) -> HashMap<String, Vec<Message>> {
    let mut out = HashMap::new();
    for (repo_id, locs) in groups {
        if locs.len() < 2 {
            continue;
        }
        let dl: Vec<DriftLoc> = locs
            .iter()
            .map(|(id, label, path)| {
                let s = snaps.get(id);
                DriftLoc {
                    label: label.clone(),
                    reachable: path.is_some()
                        && s.map(|s| !offline_state(&s.state) && !matches!(s.state, Status::Error(_)))
                            .unwrap_or(false),
                    stale_at: s.and_then(|s| s.last_checked),
                    branch: s.and_then(|s| s.branch.clone()),
                    head_sha: s.and_then(|s| s.head_sha.clone()),
                    dirty: s.map(|s| s.dirty_files + s.untracked > 0).unwrap_or(false),
                }
            })
            .collect();
        let mut rel = |i: usize, j: usize| match (&locs[i].2, &locs[j].2) {
            (Some(a), Some(b)) => git.compare_locations(a, b),
            _ => None,
        };
        let msgs = drift_messages(&dl, &mut rel, now());
        if !msgs.is_empty() {
            out.insert(repo_id.clone(), msgs);
        }
    }
    out
}
