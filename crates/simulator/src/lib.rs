//! # MigrationX Simulator
//!
//! Runs migration SQL against a real PostgreSQL instance inside a disposable
//! `postgres:16.4-bookworm` container, observing locks, broken queries, and
//! runtime behavior without copying production row data.
//!
//! Zero `unsafe` blocks are used in this crate.

#![forbid(unsafe_code)]

pub mod error;
pub mod hydrate;
pub mod runner;
pub mod simulate;

pub use error::SimulatorError;
pub use hydrate::{
    estimate_relpages, estimate_reltuples, hydrate_snapshot, inject_table_stats,
    parse_qualified_name, replay_schema, HydrationMetrics, POSTGRES_BLCKSZ,
};
pub use runner::{
    build_postgres_container_request, PostgresRunner, RunnerConfig, DEFAULT_CPU_NANO_LIMIT,
    DEFAULT_MEMORY_BYTES_LIMIT, DEFAULT_TMPFS_BYTES_LIMIT, PGDATA_MOUNT_PATH,
    PG_SOCKET_CONTAINER_DIR, POSTGRES_IMAGE_NAME, POSTGRES_IMAGE_TAG,
};
pub use simulate::{
    build_runtime_verdict, check_prepare_compatibility, parse_pg_lock_mode,
    run_migration_simulation, LockAggregator, SimulationConfig, SimulationOutput,
    DEFAULT_POLL_INTERVAL, DEFAULT_WORKER_CONCURRENCY,
};

/// Returns the crate version.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
