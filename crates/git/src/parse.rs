//! Parser für `git status --porcelain=v2 --branch`, `rev-list`, Reflog.

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusInfo {
    pub head_sha: Option<String>,
    pub branch: Option<String>, // None = detached
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub changed: u32,
    pub untracked: u32,
    pub conflicts: u32,
}

pub fn parse_status_v2(out: &str) -> StatusInfo {
    let mut s = StatusInfo::default();
    for line in out.lines() {
        if let Some(r) = line.strip_prefix("# branch.oid ") {
            if r != "(initial)" {
                s.head_sha = Some(r.into());
            }
        } else if let Some(r) = line.strip_prefix("# branch.head ") {
            if r != "(detached)" {
                s.branch = Some(r.into());
            }
        } else if let Some(r) = line.strip_prefix("# branch.upstream ") {
            s.upstream = Some(r.into());
        } else if let Some(r) = line.strip_prefix("# branch.ab ") {
            for part in r.split_whitespace() {
                if let Some(a) = part.strip_prefix('+') {
                    s.ahead = a.parse().unwrap_or(0);
                } else if let Some(b) = part.strip_prefix('-') {
                    s.behind = b.parse().unwrap_or(0);
                }
            }
        } else if line.starts_with("1 ") || line.starts_with("2 ") {
            s.changed += 1;
        } else if line.starts_with("u ") {
            s.changed += 1;
            s.conflicts += 1;
        } else if line.starts_with("? ") {
            s.untracked += 1;
        }
    }
    s
}

/// Ausgabe von `rev-list --left-right --count`: "<links>\t<rechts>".
pub fn parse_left_right(out: &str) -> Option<(u32, u32)> {
    let mut it = out.split_whitespace();
    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
}

/// Neuester Reflog-Eintrag (Format `%gd|%gs`, neueste zuerst), dessen Beschreibung mit `pull` beginnt.
pub fn parse_last_pull(out: &str) -> Option<i64> {
    out.lines().find_map(|l| {
        let (gd, gs) = l.split_once('|')?;
        if !gs.starts_with("pull") {
            return None;
        }
        // gd = "HEAD@{1730000000}"
        gd.split_once("@{")?.1.strip_suffix('}')?.parse().ok()
    })
}

/// `git log --numstat --format=%x01%ct%x01%an`: Kopfzeile je Commit, danach numstat-Zeilen.
pub fn parse_log_numstat(out: &str) -> Vec<repoorganizer_core::stats::CommitInfo> {
    use repoorganizer_core::stats::CommitInfo;
    let mut res: Vec<CommitInfo> = Vec::new();
    for line in out.lines() {
        if let Some(h) = line.strip_prefix('\u{1}') {
            let mut it = h.splitn(2, '\u{1}');
            let (Some(ts), Some(author)) = (it.next().and_then(|t| t.parse().ok()), it.next()) else { continue };
            res.push(CommitInfo { ts, author: author.trim().to_string(), added: 0, deleted: 0 });
        } else if let Some(c) = res.last_mut() {
            let mut p = line.split('\t');
            if let (Some(a), Some(d)) = (p.next(), p.next()) {
                // Binärdateien: "-\t-\tpfad"
                c.added += a.parse::<u64>().unwrap_or(0);
                c.deleted += d.parse::<u64>().unwrap_or(0);
            }
        }
    }
    res
}

use repoorganizer_core::model::DirtyFile;
use std::path::Path;
use std::time::UNIX_EPOCH;

/// Parsed `git status --porcelain=v1 -u` und ermittelt Dateipfade, Status und mtime/Größe.
pub fn parse_porcelain_dirty(out: &str, repo: &Path) -> Vec<DirtyFile> {
    let mut res = Vec::new();
    for line in out.lines() {
        if line.len() < 4 {
            continue;
        }
        let xy = &line[0..2];
        let raw_path = &line[3..];
        let p_clean = raw_path.trim_matches('"');
        let target_path = if let Some(dest) = p_clean.rsplit(" -> ").next() {
            dest.trim_matches('"')
        } else {
            p_clean
        };
        if target_path.is_empty() {
            continue;
        }

        let (status, status_code, kind) = classify_dirty_status(xy);
        let full = repo.join(target_path);
        let (mtime, size_bytes) = if let Ok(meta) = std::fs::metadata(&full) {
            let mt = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64);
            (mt, Some(meta.len()))
        } else {
            (None, None)
        };

        res.push(DirtyFile {
            path: target_path.to_string(),
            status,
            status_code,
            kind,
            mtime,
            size_bytes,
        });
    }

    // Sortierung: Neueste Änderungen zuerst; falls kein mtime (z. B. gelöscht), alphabetisch
    res.sort_by(|a, b| {
        match (b.mtime, a.mtime) {
            (Some(tb), Some(ta)) if tb != ta => tb.cmp(&ta),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            _ => a.path.cmp(&b.path),
        }
    });

    res
}

pub fn classify_dirty_status(xy: &str) -> (String, String, i32) {
    if xy.starts_with("??") {
        ("Neu (untracked)".into(), "?".into(), 1)
    } else if xy.contains('U') || xy == "AA" || xy == "DD" {
        ("Merge-Konflikt".into(), "!".into(), 3)
    } else if xy == "D " {
        ("Gelöscht (staged)".into(), "+D".into(), 3)
    } else if xy == " D" {
        ("Gelöscht (Arbeitsbaum)".into(), "D".into(), 3)
    } else if xy.starts_with('A') {
        if xy.ends_with('M') {
            ("Neu vorgemerkt (staged + Arbeitsbaum geändert)".into(), "±A".into(), 2)
        } else {
            ("Neu (staged)".into(), "+A".into(), 0)
        }
    } else if xy.starts_with('R') {
        if xy.ends_with('M') {
            ("Umbenannt (staged + Arbeitsbaum geändert)".into(), "±R".into(), 2)
        } else {
            ("Umbenannt (staged)".into(), "+R".into(), 0)
        }
    } else if xy == "M " {
        ("Vorgemerkt (staged)".into(), "+M".into(), 0)
    } else if xy == " M" {
        ("Nicht vorgemerkt (Arbeitsbaum)".into(), "M".into(), 2)
    } else if xy == "MM" {
        ("Teilweise vorgemerkt (staged + Arbeitsbaum)".into(), "±M".into(), 2)
    } else if xy.contains('M') {
        if xy.starts_with('M') {
            ("Vorgemerkt (staged)".into(), "+M".into(), 0)
        } else {
            ("Nicht vorgemerkt (Arbeitsbaum)".into(), "M".into(), 2)
        }
    } else {
        let code = xy.trim().chars().next().map(|c| c.to_string()).unwrap_or_else(|| "M".into());
        (format!("Offen ({})", xy.trim()), code, 2)
    }
}
