//! Statistik: Aggregation von Commit-Aktivität (Wochen, Monate, Heatmap, Autoren, Zeilen).
use crate::model::Activity;
use std::collections::{BTreeMap, HashMap};

/// Ein Commit aus `git log --numstat`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub ts: i64,
    pub author: String,
    pub added: u64,
    pub deleted: u64,
}

const DAY: i64 = 86400;

/// Tage seit 1970-01-01 → (Jahr, Monat, Tag), proleptisch gregorianisch (UTC).
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d)
}

pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn date_string(day: i64) -> String {
    let (y, m, d) = civil_from_days(day);
    format!("{y:04}-{m:02}-{d:02}")
}

fn parse_date(s: &str) -> Option<i64> {
    let mut it = s.split('-');
    let (y, m, d) = (it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?);
    Some(days_from_civil(y, m, d))
}

/// Commits je Tag (UTC) zusammenfassen.
pub fn to_activity(repo_id: &str, commits: &[CommitInfo]) -> Vec<Activity> {
    let mut map: BTreeMap<String, Activity> = BTreeMap::new();
    for c in commits {
        let date = date_string(c.ts.div_euclid(DAY));
        let a = map.entry(date.clone()).or_insert_with(|| Activity { repo_id: repo_id.into(), date, commits: 0, authors: BTreeMap::new(), added: 0, deleted: 0 });
        a.commits += 1;
        a.added += c.added;
        a.deleted += c.deleted;
        *a.authors.entry(c.author.clone()).or_default() += 1;
    }
    map.into_values().collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bar {
    pub label: String,
    pub value: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeatCell {
    pub col: u32,
    pub row: u32,
    /// 0 = keine Commits, 1..=4 = zunehmende Aktivität
    pub level: u8,
    pub count: u32,
    pub date: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Summary {
    pub total_commits: u32,
    pub commits_30d: u32,
    pub added: u64,
    pub deleted: u64,
    pub last_activity: Option<String>,
    /// 12 Wochen, älteste zuerst
    pub weeks: Vec<Bar>,
    /// 12 Kalendermonate, ältester zuerst
    pub months: Vec<Bar>,
    /// 53 Spalten (Wochen, Montag zuerst) × 7 Zeilen; Zellen in der Zukunft fehlen.
    pub heat: Vec<HeatCell>,
    pub authors: Vec<Bar>,
}

pub const HEAT_COLS: u32 = 53;

pub fn summarize(acts: &[Activity], now: i64) -> Summary {
    let today = now.div_euclid(DAY);
    let mut by_day: HashMap<i64, u32> = HashMap::new();
    let mut s = Summary::default();
    let mut authors: BTreeMap<String, u32> = BTreeMap::new();
    for a in acts {
        let Some(d) = parse_date(&a.date) else { continue };
        *by_day.entry(d).or_default() += a.commits;
        s.total_commits += a.commits;
        s.added += a.added;
        s.deleted += a.deleted;
        if d > today - 30 && d <= today {
            s.commits_30d += a.commits;
        }
        for (n, c) in &a.authors {
            *authors.entry(n.clone()).or_default() += c;
        }
        if s.last_activity.as_ref().map(|l| a.date > *l).unwrap_or(true) {
            s.last_activity = Some(a.date.clone());
        }
    }
    let range = |from: i64, to: i64| -> u32 { by_day.iter().filter(|(d, _)| **d >= from && **d <= to).map(|(_, c)| *c).sum() };

    for w in (0..12).rev() {
        let end = today - 7 * w;
        let start = end - 6;
        let (_, m, d) = civil_from_days(start);
        s.weeks.push(Bar { label: format!("{d:02}.{m:02}"), value: range(start, end) });
    }

    let (cy, cm, _) = civil_from_days(today);
    for back in (0..12).rev() {
        let idx = cy * 12 + (cm as i64 - 1) - back;
        let (y, m) = (idx.div_euclid(12), idx.rem_euclid(12) as u32 + 1);
        let from = days_from_civil(y, m, 1);
        let next = if m == 12 { days_from_civil(y + 1, 1, 1) } else { days_from_civil(y, m + 1, 1) };
        s.months.push(Bar { label: format!("{m:02}/{:02}", y.rem_euclid(100)), value: range(from, next - 1) });
    }

    let weekday = (today + 3).rem_euclid(7); // Montag = 0
    let start = today - weekday - 52 * 7;
    let max = (start..=today).map(|d| by_day.get(&d).copied().unwrap_or(0)).max().unwrap_or(0).max(1);
    for col in 0..HEAT_COLS {
        for row in 0..7u32 {
            let day = start + col as i64 * 7 + row as i64;
            if day > today {
                continue;
            }
            let count = by_day.get(&day).copied().unwrap_or(0);
            let level = if count == 0 { 0 } else { ((count * 4).div_ceil(max)).clamp(1, 4) as u8 };
            s.heat.push(HeatCell { col, row, level, count, date: date_string(day) });
        }
    }

    let mut a: Vec<_> = authors.into_iter().collect();
    a.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(&y.0)));
    s.authors = a.into_iter().take(5).map(|(label, value)| Bar { label, value }).collect();
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(day: i64, author: &str, add: u64, del: u64) -> CommitInfo {
        CommitInfo { ts: day * DAY + 3600, author: author.into(), added: add, deleted: del }
    }

    #[test]
    fn civil_roundtrip() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(days_from_civil(2024, 2, 29), 19_782);
        for d in [-1000, 0, 11_000, 19_782, 25_000] {
            let (y, m, dd) = civil_from_days(d);
            assert_eq!(days_from_civil(y, m, dd), d);
        }
        assert_eq!(date_string(19_782), "2024-02-29");
    }

    #[test]
    fn aggregates() {
        let today = days_from_civil(2026, 11, 5); // Donnerstag
        let now = today * DAY + 100;
        let commits = vec![
            c(today, "anna", 10, 2),
            c(today, "anna", 5, 0),
            c(today - 3, "bob", 1, 1),
            c(today - 20, "anna", 0, 4),
            c(today - 100, "bob", 7, 0),
            c(today - 400, "old", 1, 1),
        ];
        let acts = to_activity("r", &commits);
        assert_eq!(acts.iter().find(|a| a.date == "2026-11-05").unwrap().commits, 2);
        let s = summarize(&acts, now);
        assert_eq!(s.total_commits, 6);
        assert_eq!(s.commits_30d, 4);
        assert_eq!((s.added, s.deleted), (24, 8));
        assert_eq!(s.last_activity.as_deref(), Some("2026-11-05"));
        assert_eq!(s.weeks.len(), 12);
        assert_eq!(s.weeks[11].value, 3); // heute und vor 3 Tagen
        assert_eq!(s.weeks[9].value, 1); // vor 20 Tagen
        assert_eq!(s.months.len(), 12);
        assert_eq!(s.months[11].label, "11/26");
        assert_eq!(s.months[11].value, 3); // 1.–5. Nov.: heute 2 + vor 3 Tagen
        assert_eq!(s.authors[0], Bar { label: "anna".into(), value: 3 });
        // Heatmap: letzte Spalte endet am heutigen Donnerstag (Zeile 3)
        let last = s.heat.last().unwrap();
        assert_eq!((last.col, last.row, last.date.as_str(), last.level), (52, 3, "2026-11-05", 4));
        assert_eq!(s.heat.len() as u32, 52 * 7 + 4);
    }

    #[test]
    fn empty_is_fine() {
        let s = summarize(&[], 0);
        assert_eq!(s.total_commits, 0);
        assert!(s.last_activity.is_none());
        assert_eq!(s.weeks.len(), 12);
    }
}

