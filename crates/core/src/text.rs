//! Kleine Textformatierer.

/// "vor 5 Min" usw.; `None` → "nie".
pub fn ago(ts: Option<i64>, now: i64) -> String {
    let Some(ts) = ts else { return "nie".into() };
    let d = (now - ts).max(0);
    match d {
        0..=59 => "gerade eben".into(),
        60..=3599 => format!("vor {} Min", d / 60),
        3600..=86399 => format!("vor {} Std", d / 3600),
        _ => format!("vor {} Tagen", d / 86400),
    }
}
