//! Typed error definitions for the MigrationX static analyzer.

use thiserror::Error;

/// Errors that can occur during static SQL analysis.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AnalyzerError {
    /// The provided migration SQL string was empty or contained only whitespace.
    #[error("migration SQL is empty")]
    EmptyMigration,

    /// `pg_query` failed to parse the migration SQL.
    #[error("SQL parse error: {message}")]
    ParseError {
        /// Underlying parser error message from `libpg_query`.
        message: String,
    },
}
