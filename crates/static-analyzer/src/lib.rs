//! # MigrationX Static Analyzer
//!
//! Statically analyzes PostgreSQL migration SQL for potential hazards
//! using `pg_query` v18 AST inspection.
//!
//! ## Safety & Dependencies
//!
//! This crate contains zero `unsafe` blocks. It wraps `pg_query` (which uses
//! `libpg_query` C bindings internally) solely for deterministic SQL parsing.

mod analyzer;
mod error;
mod rules;

pub use analyzer::analyze_migration;
pub use error::AnalyzerError;

/// Returns the crate version.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
