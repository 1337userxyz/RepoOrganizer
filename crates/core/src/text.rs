//! Kleine Textformatierer.

/// "vor 5 Min" usw.; `None` → "nie".
pub fn ago(ts: Option<i64>, now: i64) -> String {
    kit_fs::format_relative_time(ts, now)
}
