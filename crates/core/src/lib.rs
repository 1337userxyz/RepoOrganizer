//! RepoOrganizer Core: UI-freie, testbare Logik.
pub mod cache;
pub mod config;
pub mod drift;
pub mod health;
pub mod level;
pub mod model;
pub mod paths;
pub mod reach;
pub mod scan;
pub mod stats;
pub mod text;
pub mod views;

#[cfg(test)]
mod tests;



