//! # MigrationX Static Analyzer
//!
//! Statically analyzes PostgreSQL migration SQL for potential hazards
//! using `pg_query` parsing and rule-based detection.

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
