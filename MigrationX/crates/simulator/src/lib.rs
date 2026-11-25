//! # MigrationX Simulator
//!
//! Runs migration SQL against a real PostgreSQL instance inside a disposable
//! container, observing locks, broken queries, and runtime behavior.

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
