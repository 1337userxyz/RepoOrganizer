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
