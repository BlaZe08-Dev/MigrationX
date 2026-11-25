//! Typed error definitions for the MigrationX simulator.

use thiserror::Error;

/// Errors returned by the ephemeral PostgreSQL container runner and schema
/// hydration pipeline.
#[derive(Debug, Error)]
pub enum SimulatorError {
    /// Failed to start the PostgreSQL container.
    #[error("failed to start PostgreSQL container: {message}")]
    ContainerStartFailed {
        /// Diagnostic message (must not contain raw credentials).
        message: String,
    },

    /// Timed out waiting for the PostgreSQL container to become ready.
    #[error("PostgreSQL container boot timed out after {timeout_secs}s")]
    ContainerBootTimeout {
        /// Configured boot timeout in seconds.
        timeout_secs: u64,
    },

    /// Failed to remove the disposable PostgreSQL container during explicit shutdown.
    #[error("failed to clean up PostgreSQL container {container_id}: {message}")]
    ContainerCleanupFailed {
        /// Docker container ID.
        container_id: String,
        /// Diagnostic message.
        message: String,
    },

    /// Failed to establish a PostgreSQL client connection to the container.
    #[error("failed to connect to simulation PostgreSQL instance: {message}")]
    ConnectionFailed {
        /// Diagnostic message (credentials redacted).
        message: String,
    },

    /// An async database or container operation exceeded its configured timeout.
    #[error("simulation operation '{operation}' timed out after {timeout_secs}s")]
    OperationTimeout {
        /// Name of the operation that timed out.
        operation: &'static str,
        /// Configured timeout in seconds.
        timeout_secs: u64,
    },

    /// Replaying the target schema DDL failed inside the simulation container.
    #[error("schema replay failed{}: {message}", sqlstate.as_deref().map(|s| format!(" (SQLSTATE {s})")).unwrap_or_default())]
    SchemaReplayFailed {
        /// PostgreSQL error message.
        message: String,
        /// Optional SQLSTATE code.
        sqlstate: Option<String>,
    },

    /// Injecting `pg_class` statistics for a table failed.
    #[error("failed to inject pg_class stats for '{qualified_name}': {message}")]
    StatsInjectionFailed {
        /// Fully-qualified table name (`schema.table`).
        qualified_name: String,
        /// Diagnostic message.
        message: String,
    },

    /// A table identifier in `Snapshot.tables` is invalid.
    #[error("invalid table identifier '{qualified_name}': {reason}")]
    InvalidIdentifier {
        /// Offending identifier.
        qualified_name: String,
        /// Reason the identifier was rejected.
        reason: String,
    },

    /// A filesystem or OS I/O operation failed.
    #[error("I/O error during '{operation}': {message}")]
    Io {
        /// Operation description.
        operation: &'static str,
        /// Error message.
        message: String,
    },
}
