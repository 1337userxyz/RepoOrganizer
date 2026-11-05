//! Darstellungslogik (UI-unabhängig, testbar).
use repoorganizer_core::model::Status;

pub fn status_text(s: &Status) -> String {
    match s {
        Status::Unavailable => "Offline".into(),
        Status::NotARepo => "Kein Repo".into(),
        Status::UpToDate => "Aktuell".into(),
        Status::Behind(n) => format!("{n} hinter"),
        Status::Ahead(n) => format!("{n} voraus"),
        Status::Diverged => "Divergiert".into(),
        Status::Dirty => "Geändert".into(),
        Status::Error(_) => "Fehler".into(),
    }
}

pub fn status_error(s: &Status) -> String {
    match s {
        Status::Error(msg) => msg.clone(),
        Status::NotARepo => "Ordner existiert, enthält aber kein .git-Verzeichnis.".into(),
        _ => String::new(),
    }
}

/// Einzeilige Kurzfassung für die Repo-Liste (verhindert Zeilenumbrüche/Überlagerung).
pub fn status_error_short(s: &Status) -> String {
    let full = status_error(s);
    if full.is_empty() {
        return String::new();
    }
    let first = full.lines().next().unwrap_or("").trim();
    if first.len() > 95 {
        format!("{}…", &first[..95])
    } else {
        first.to_string()
    }
}

/// 0 ok · 1 info · 2 warnung · 3 fehler · 4 neutral/offline
pub fn status_kind(s: &Status) -> i32 {
    match s {
        Status::UpToDate => 0,
        Status::Behind(_) | Status::Ahead(_) => 1,
        Status::Dirty | Status::Diverged => 2,
        Status::Error(_) | Status::NotARepo => 3,
        Status::Unavailable => 4,
    }
}

/// Schwere für die Aggregation je Repo (höher = schlimmer).
pub fn severity(s: &Status) -> u8 {
    match s {
        Status::Error(_) | Status::NotARepo => 6,
        Status::Diverged => 5,
        Status::Dirty => 4,
        Status::Behind(_) => 3,
        Status::Ahead(_) => 2,
        Status::UpToDate => 1,
        Status::Unavailable => 0,
    }
}

pub use repoorganizer_core::text::ago;
use repoorganizer_core::stats::civil_from_days;

pub fn format_commit_date(ts: Option<i64>, now: i64) -> String {
    let Some(ts) = ts else { return "–".into() };
    let (y, m, d) = civil_from_days(ts.div_euclid(86400));
    format!("{d:02}.{m:02}.{y:04} ({})", ago(Some(ts), now))
}

pub fn format_size_kb(kb: u64) -> String {
    if kb == 0 {
        "–".into()
    } else if kb < 1024 {
        format!("{kb} KB")
    } else {
        format!("{:.1} MB", kb as f64 / 1024.0)
    }
}

pub fn format_sync(ahead: u32, behind: u32, dirty: u32, stashes: u32) -> String {
    let mut parts = Vec::new();
    if ahead > 0 {
        parts.push(format!("↑{ahead}"));
    }
    if behind > 0 {
        parts.push(format!("↓{behind}"));
    }
    if dirty > 0 {
        parts.push(format!("Δ{dirty}"));
    }
    if stashes > 0 {
        parts.push(format!("📦{stashes}"));
    }
    if parts.is_empty() {
        "✓ sauber".into()
    } else {
        parts.join("  ")
    }
}

pub fn slug(name: &str) -> String {
    let s: String = name.trim().to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
    s.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ago_formats() {
        assert_eq!(ago(None, 10), "nie");
        assert_eq!(ago(Some(100), 130), "gerade eben");
        assert_eq!(ago(Some(0), 300), "vor 5 Min");
        assert_eq!(ago(Some(0), 7300), "vor 2 Std");
        assert_eq!(ago(Some(0), 3 * 86400 + 5), "vor 3 Tagen");
    }

    #[test]
    fn severity_orders() {
        assert!(severity(&Status::Diverged) > severity(&Status::Dirty));
        assert!(severity(&Status::Behind(1)) > severity(&Status::UpToDate));
        assert_eq!(slug(" My Repo!_x "), "my-repo--x");
    }
}

