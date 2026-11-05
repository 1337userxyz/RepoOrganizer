//! Gemeinsame Bewertungsstufe (Ampel).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    Warn,
    Error,
}
