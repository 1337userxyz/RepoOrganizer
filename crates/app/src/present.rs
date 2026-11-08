//! Darstellungslogik (UI-unabhängig, testbar).
use repoorganizer_core::model::Status;

/// Hauptstatus des Repositories (kombiniert Remote-Verhältnis und Arbeitskopie).
/// Stellt sicher, dass ein Repo NIEMALS als "✓ Synchron" bezeichnet wird, wenn uncommittete Dateien vorliegen!
pub fn repo_display_status(s: &Status, ahead: u32, behind: u32, dirty: u32, has_remote: bool) -> String {
    match s {
        Status::Unavailable => "⭘ Offline".into(),
        Status::NotARepo => "Kein Repo".into(),
        Status::Error(_) => "⚠ Fehler".into(),
        _ => {
            let eff_ahead = if ahead > 0 { ahead } else { match s { Status::Ahead(n) => *n, _ => 0 } };
            let eff_behind = if behind > 0 { behind } else { match s { Status::Behind(n) => *n, _ => 0 } };

            if !has_remote {
                if dirty > 0 {
                    format!("💻 Nur lokal (✎ {dirty} offen)")
                } else {
                    "💻 Nur lokal (sauber)".into()
                }
            } else if eff_ahead > 0 && eff_behind > 0 {
                if dirty > 0 {
                    format!("⇄ Divergiert (↑{eff_ahead} ↓{eff_behind} · ✎ {dirty})")
                } else {
                    format!("⇄ Divergiert (↑{eff_ahead} ↓{eff_behind})")
                }
            } else if eff_ahead > 0 {
                if dirty > 0 {
                    format!("↑ {eff_ahead} zu senden · ✎ {dirty} offen")
                } else {
                    format!("↑ {eff_ahead} zu senden")
                }
            } else if eff_behind > 0 {
                if dirty > 0 {
                    format!("↓ {eff_behind} zu holen · ✎ {dirty} offen")
                } else {
                    format!("↓ {eff_behind} zu holen")
                }
            } else if dirty > 0 {
                format!("✎ {dirty} uncommittet")
            } else {
                "✓ Synchron".into()
            }
        }
    }
}

pub fn repo_display_kind(s: &Status, ahead: u32, behind: u32, dirty: u32, has_remote: bool) -> i32 {
    match s {
        Status::Unavailable => 4, // grau
        Status::NotARepo | Status::Error(_) => 3, // rot
        _ => {
            if !has_remote {
                if dirty > 0 { 2 } else { 4 }
            } else {
                let eff_ahead = if ahead > 0 { ahead } else { match s { Status::Ahead(n) => *n, _ => 0 } };
                let eff_behind = if behind > 0 { behind } else { match s { Status::Behind(n) => *n, _ => 0 } };

                if eff_ahead > 0 && eff_behind > 0 {
                    3 // rot konflikt
                } else if eff_behind > 0 {
                    2 // orange pull
                } else if dirty > 0 {
                    2 // orange uncommittet
                } else if eff_ahead > 0 {
                    1 // blau push
                } else {
                    0 // grün synchron & sauber
                }
            }
        }
    }
}

/// Spezifisches Badge für das Verhältnis zu GitHub / Remote im Detailbereich.
pub fn remote_relation_badge(ahead: u32, behind: u32, has_remote: bool) -> (String, i32) {
    if !has_remote {
        ("Kein Remote".into(), 4)
    } else if ahead > 0 && behind > 0 {
        (format!("GitHub: ⇄ Divergiert (↑{ahead} ↓{behind})"), 3)
    } else if ahead > 0 {
        (format!("GitHub: ↑ {ahead} ungesendet"), 1)
    } else if behind > 0 {
        (format!("GitHub: ↓ {behind} neu"), 2)
    } else {
        ("GitHub: Aktuell".into(), 0)
    }
}

/// Status des Remote-Syncs (Verhältnis der Commits zwischen lokal und Remote).
pub fn remote_sync_text(s: &Status, ahead: u32, behind: u32, has_remote: bool) -> String {
    repo_display_status(s, ahead, behind, 0, has_remote)
}

pub fn remote_sync_kind(s: &Status, ahead: u32, behind: u32, has_remote: bool) -> i32 {
    repo_display_kind(s, ahead, behind, 0, has_remote)
}

/// Status der lokalen Arbeitskopie (uncommittete / geänderte / neue Dateien auf der Festplatte).
pub fn working_tree_text(dirty: u32, stashes: u32) -> String {
    if dirty == 0 && stashes == 0 {
        "✓ Sauber".into()
    } else if dirty > 0 && stashes > 0 {
        format!("✎ {dirty} uncommittet · 📦{stashes}")
    } else if dirty > 0 {
        format!("✎ {dirty} uncommittet")
    } else {
        format!("✓ Sauber · 📦{stashes}")
    }
}

pub fn working_tree_kind(dirty: u32) -> i32 {
    if dirty > 0 {
        2 // warnung / uncommittet
    } else {
        0 // grün / sauber
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

#[allow(dead_code)]
pub fn status_text(s: &Status) -> String {
    remote_sync_text(s, 0, 0, true)
}

#[allow(dead_code)]
pub fn detailed_status_text(s: &Status, ahead: u32, behind: u32, _dirty: u32) -> String {
    remote_sync_text(s, ahead, behind, true)
}

#[allow(dead_code)]
pub fn detailed_status_kind(s: &Status, ahead: u32, behind: u32, _dirty: u32) -> i32 {
    remote_sync_kind(s, ahead, behind, true)
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

/// Formatiert eine Datei-Änderungszeit mit Uhrzeit (und Datum bei älteren Änderungen) plus relativer Zeit.
pub fn format_mtime(ts: Option<i64>, now: i64) -> String {
    let Some(ts) = ts else { return "–".into() };
    let rel = ago(Some(ts), now);
    let day_secs = ts.rem_euclid(86400);
    let hour = day_secs / 3600;
    let min = (day_secs % 3600) / 60;
    let sec = day_secs % 60;
    let now_days = now.div_euclid(86400);
    let ts_days = ts.div_euclid(86400);
    if now_days == ts_days {
        format!("{hour:02}:{min:02}:{sec:02} ({rel})")
    } else {
        let (y, m, d) = civil_from_days(ts_days);
        format!("{d:02}.{m:02}.{y:04} {hour:02}:{min:02} ({rel})")
    }
}

pub fn format_file_size(bytes: Option<u64>) -> String {
    let Some(b) = bytes else { return "–".into() };
    if b < 1024 {
        format!("{b} B")
    } else if b < 1024 * 1024 {
        format!("{:.1} KB", b as f64 / 1024.0)
    } else {
        format!("{:.1} MB", b as f64 / (1024.0 * 1024.0))
    }
}

#[allow(dead_code)]
pub fn dirty_summary(files: &[repoorganizer_core::model::DirtyFile], now: i64) -> String {
    if files.is_empty() {
        return "Keine uncommitteten Änderungen".into();
    }
    let newest = files.iter().filter_map(|f| f.mtime).max();
    if let Some(t) = newest {
        format!("Letzte Änderung: {}", format_mtime(Some(t), now))
    } else {
        format!("{} uncommittete Datei(en)", files.len())
    }
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

#[allow(dead_code)]
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

/// Farbkategorie für die Größe des .git-Verzeichnisses:
/// 0 = Grün (< 50 MB), 1 = Orange (50 - 500 MB), 2 = Rot (> 500 MB)
pub fn size_kind(size_kb: u64) -> i32 {
    let mb = size_kb / 1024;
    if mb >= 500 {
        2 // rot (> 500 MB)
    } else if mb >= 50 {
        1 // orange (50 - 500 MB)
    } else {
        0 // grün (< 50 MB)
    }
}

/// Farbkategorie für Build-Artefakte:
/// 0 = Neutral / Sauber (0 Bytes), 1 = Orange (< 500 MB), 2 = Rot (>= 500 MB, Bereinigung empfohlen)
pub fn build_size_kind(bytes: u64) -> i32 {
    let mb = bytes / 1_048_576;
    if mb >= 500 {
        2 // rot (große Build-Reste)
    } else if bytes > 0 {
        1 // orange (moderate Build-Artefakte)
    } else {
        0 // neutral / sauber
    }
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

    #[test]
    fn repo_display_status_formatting() {
        // Uncommittete Dateien: Darf NIEMALS als "✓ Synchron" angezeigt werden!
        assert_eq!(repo_display_status(&Status::Dirty, 0, 0, 15, true), "✎ 15 uncommittet");
        assert_eq!(repo_display_kind(&Status::Dirty, 0, 0, 15, true), 2); // Orange

        // Sowohl ungesendete Commits als auch uncommittete Dateien
        assert_eq!(repo_display_status(&Status::Dirty, 3, 0, 5, true), "↑ 3 zu senden · ✎ 5 offen");
        assert_eq!(repo_display_kind(&Status::Dirty, 3, 0, 5, true), 2); // Orange

        // Nur ungesendete Commits (saubere Arbeitskopie)
        assert_eq!(repo_display_status(&Status::Ahead(3), 3, 0, 0, true), "↑ 3 zu senden");
        assert_eq!(repo_display_kind(&Status::Ahead(3), 3, 0, 0, true), 1); // Blau

        // Nur Pull nötig
        assert_eq!(repo_display_status(&Status::Behind(2), 0, 2, 0, true), "↓ 2 zu holen");
        assert_eq!(repo_display_kind(&Status::Behind(2), 0, 2, 0, true), 2); // Orange

        // Vollständig synchron und sauber
        assert_eq!(repo_display_status(&Status::UpToDate, 0, 0, 0, true), "✓ Synchron");
        assert_eq!(repo_display_kind(&Status::UpToDate, 0, 0, 0, true), 0); // Grün

        // Nur lokal
        assert_eq!(repo_display_status(&Status::UpToDate, 0, 0, 0, false), "💻 Nur lokal (sauber)");
        assert_eq!(repo_display_status(&Status::Dirty, 0, 0, 2, false), "💻 Nur lokal (✎ 2 offen)");

        // Offline
        assert_eq!(repo_display_status(&Status::Unavailable, 0, 0, 0, true), "⭘ Offline");
        assert_eq!(repo_display_kind(&Status::Unavailable, 0, 0, 0, true), 4);
    }

    #[test]
    fn size_kinds_classification() {
        assert_eq!(size_kind(10 * 1024), 0); // 10 MB -> grün
        assert_eq!(size_kind(150 * 1024), 1); // 150 MB -> orange
        assert_eq!(size_kind(800 * 1024), 2); // 800 MB -> rot

        assert_eq!(build_size_kind(0), 0); // 0 B -> sauber
        assert_eq!(build_size_kind(50 * 1024 * 1024), 1); // 50 MB -> orange
        assert_eq!(build_size_kind(1200 * 1024 * 1024), 2); // 1.2 GB -> rot
    }

    #[test]
    fn format_mtime_and_sizes() {
        assert_eq!(format_mtime(None, 100), "–");
        // Gleicher Tag
        let now = 100_000;
        let ts = 99_900;
        let s = format_mtime(Some(ts), now);
        assert!(s.contains("vor"));

        assert_eq!(format_file_size(None), "–");
        assert_eq!(format_file_size(Some(500)), "500 B");
        assert_eq!(format_file_size(Some(2048)), "2.0 KB");
        assert_eq!(format_file_size(Some(5 * 1024 * 1024)), "5.0 MB");
    }
}

