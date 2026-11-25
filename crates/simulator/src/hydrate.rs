//! Schema DDL replay and zero-row `pg_class` statistics injection.
//!
//! Hydrates an ephemeral PostgreSQL simulation container with the target schema
//! and production planner statistics (`reltuples`, `relpages`) without copying
//! any production table rows.

use std::time::{Duration, Instant};

use migrationx_contracts::Snapshot;
use tokio_postgres::Client;
use tracing::{debug, info};

use crate::error::SimulatorError;
use crate::runner::PostgresRunner;

/// PostgreSQL default block size (`BLCKSZ` = 8 KiB) used to derive `relpages`
/// from `TableStats.total_size_bytes`.
pub const POSTGRES_BLCKSZ: u64 = 8192;

/// Latency breakdown for schema replay and `pg_class` statistics injection,
/// measured separately from container boot latency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HydrationMetrics {
    /// Time spent replaying `schema_sql` into the simulation database.
    pub schema_replay_latency: Duration,
    /// Time spent updating `pg_class.reltuples` and `pg_class.relpages`.
    pub stats_injection_latency: Duration,
    /// Total simulation hydration latency (`schema_replay_latency + stats_injection_latency`).
    pub total_simulation_latency: Duration,
    /// Number of relations updated in `pg_catalog.pg_class`.
    pub tables_updated: usize,
}

/// Parses a relation identifier into `(schema, table)`.
///
/// Accepts either `"schema.table"` or an unqualified `"table"` (which defaults
/// to `"public"`). Rejects empty components, NUL bytes, or multi-dot names.
pub fn parse_qualified_name(qualified_name: &str) -> Result<(&str, &str), SimulatorError> {
    if qualified_name.is_empty() || qualified_name.contains('\0') {
        return Err(SimulatorError::InvalidIdentifier {
            qualified_name: qualified_name.to_string(),
            reason: "identifier must be non-empty and must not contain NUL bytes".to_string(),
        });
    }

    let mut parts = qualified_name.split('.');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(table), None, None) if !table.is_empty() => Ok(("public", table)),
        (Some(schema), Some(table), None) if !schema.is_empty() && !table.is_empty() => {
            Ok((schema, table))
        }
        _ => Err(SimulatorError::InvalidIdentifier {
            qualified_name: qualified_name.to_string(),
            reason: "expected 'table' or 'schema.table' with non-empty parts".to_string(),
        }),
    }
}

/// Converts `TableStats.estimated_row_count` into a PostgreSQL `pg_class.reltuples` (`float4`) value.
///
/// Negative values below `-1` are clamped to `-1.0` (PostgreSQL's sentinel for
/// unanalyzed relations).
pub fn estimate_reltuples(estimated_row_count: i64) -> f32 {
    if estimated_row_count < 0 {
        -1.0
    } else {
        estimated_row_count as f32
    }
}

/// Derives `pg_class.relpages` (`int4`) from `TableStats.total_size_bytes` using
/// PostgreSQL's 8 KiB block size, saturating at `i32::MAX`.
pub fn estimate_relpages(total_size_bytes: u64) -> i32 {
    let pages = total_size_bytes / POSTGRES_BLCKSZ;
    i32::try_from(pages).unwrap_or(i32::MAX)
}

/// Replays `schema_sql` into the simulation PostgreSQL instance with an explicit
/// timeout.
///
/// # Cancellation Safety
///
/// If cancelled or timed out, the container itself is ephemeral and will be
/// destroyed by [`PostgresRunner`] RAII cleanup.
pub async fn replay_schema(
    client: &Client,
    schema_sql: &str,
    timeout: Duration,
) -> Result<Duration, SimulatorError> {
    if schema_sql.trim().is_empty() {
        return Ok(Duration::ZERO);
    }

    let start = Instant::now();
    tokio::time::timeout(timeout, client.batch_execute(schema_sql))
        .await
        .map_err(|_| SimulatorError::OperationTimeout {
            operation: "replay_schema",
            timeout_secs: timeout.as_secs(),
        })?
        .map_err(|err| {
            let sqlstate = err.as_db_error().map(|db| db.code().code().to_string());
            SimulatorError::SchemaReplayFailed {
                message: err.to_string(),
                sqlstate,
            }
        })?;

    let elapsed = start.elapsed();
    debug!(
        schema_replay_ms = elapsed.as_millis() as u64,
        "replayed schema DDL into simulation database"
    );
    Ok(elapsed)
}

/// Injects `Snapshot.tables` statistics (`estimated_row_count` -> `reltuples`,
/// derived `pages` -> `relpages`) into `pg_catalog.pg_class` without copying
/// any production rows.
///
/// # Cancellation Safety
///
/// Uses parameterized queries against an ephemeral container; on timeout or
/// cancellation, [`PostgresRunner`] destroys the container.
pub async fn inject_table_stats(
    client: &Client,
    snapshot: &Snapshot,
    timeout: Duration,
) -> Result<(usize, Duration), SimulatorError> {
    const UPDATE_SQL: &str = "\
        UPDATE pg_catalog.pg_class AS c \
        SET reltuples = $1, \
            relpages  = $2 \
        FROM pg_catalog.pg_namespace AS n \
        WHERE c.relnamespace = n.oid \
          AND n.nspname = $3 \
          AND c.relname = $4 \
          AND c.relkind IN ('r', 'p', 'm')";

    let start = Instant::now();
    let mut updated_count: usize = 0;

    for stats in snapshot.tables.values() {
        let (schema, table) = parse_qualified_name(&stats.qualified_name)?;
        let reltuples = estimate_reltuples(stats.estimated_row_count);
        let relpages = estimate_relpages(stats.total_size_bytes);

        let affected = tokio::time::timeout(
            timeout,
            client.execute(UPDATE_SQL, &[&reltuples, &relpages, &schema, &table]),
        )
        .await
        .map_err(|_| SimulatorError::OperationTimeout {
            operation: "inject_table_stats",
            timeout_secs: timeout.as_secs(),
        })?
        .map_err(|err| SimulatorError::StatsInjectionFailed {
            qualified_name: stats.qualified_name.clone(),
            message: err.to_string(),
        })?;

        if affected == 0 {
            return Err(SimulatorError::StatsInjectionFailed {
                qualified_name: stats.qualified_name.clone(),
                message: "target relation not found in pg_class after schema replay".to_string(),
            });
        }

        updated_count += 1;
    }

    let elapsed = start.elapsed();
    info!(
        tables_updated = updated_count,
        stats_injection_ms = elapsed.as_millis() as u64,
        "injected snapshot statistics into pg_class"
    );

    Ok((updated_count, elapsed))
}

/// Replays `schema_sql` and injects `snapshot.tables` into `pg_class` inside
/// the given [`PostgresRunner`], returning separate simulation latency metrics.
pub async fn hydrate_snapshot(
    runner: &PostgresRunner,
    schema_sql: &str,
    snapshot: &Snapshot,
) -> Result<HydrationMetrics, SimulatorError> {
    let client = runner.client()?;
    let timeout = runner.config().query_timeout;

    let sim_start = Instant::now();
    let schema_replay_latency = replay_schema(client, schema_sql, timeout).await?;
    let (tables_updated, stats_injection_latency) =
        inject_table_stats(client, snapshot, timeout).await?;
    let total_simulation_latency = sim_start.elapsed();

    Ok(HydrationMetrics {
        schema_replay_latency,
        stats_injection_latency,
        total_simulation_latency,
        tables_updated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use migrationx_contracts::{TableStats, SCHEMA_VERSION};
    use std::collections::HashMap;

    #[test]
    fn test_hydrate_parse_qualified_name_valid_and_invalid() {
        assert_eq!(
            parse_qualified_name("public.users").expect("qualified"),
            ("public", "users")
        );
        assert_eq!(
            parse_qualified_name("orders").expect("unqualified"),
            ("public", "orders")
        );
        assert!(parse_qualified_name("").is_err());
        assert!(parse_qualified_name(".users").is_err());
        assert!(parse_qualified_name("public.").is_err());
        assert!(parse_qualified_name("a.b.c").is_err());
        assert!(parse_qualified_name("public.us\0ers").is_err());
    }

    #[test]
    fn test_hydrate_estimate_reltuples_and_relpages() {
        assert_eq!(estimate_reltuples(0), 0.0);
        assert_eq!(estimate_reltuples(1_500_000), 1_500_000.0);
        assert_eq!(estimate_reltuples(-5), -1.0);

        assert_eq!(estimate_relpages(0), 0);
        assert_eq!(estimate_relpages(8192), 1);
        assert_eq!(estimate_relpages(256 * 1024 * 1024), 32768);
        assert_eq!(estimate_relpages(u64::MAX), i32::MAX);
    }

    #[tokio::test]
    #[ignore = "requires docker"]
    async fn test_hydrate_replays_schema_and_injects_pg_class_stats() {
        let mut tables = HashMap::new();
        tables.insert(
            "public.orders".to_string(),
            TableStats {
                qualified_name: "public.orders".to_string(),
                estimated_row_count: 5_000_000,
                total_size_bytes: 512 * 1024 * 1024,
                has_triggers: false,
                has_foreign_key_refs: false,
            },
        );
        let snapshot = Snapshot {
            schema_version: SCHEMA_VERSION.to_string(),
            captured_at: "2026-11-25T08:00:00Z".to_string(),
            pg_version: "16.4".to_string(),
            database_name: "prod".to_string(),
            tables,
        };
        let schema_sql = "CREATE TABLE public.orders (id BIGSERIAL PRIMARY KEY, total INT);";

        let runner = PostgresRunner::start(crate::RunnerConfig::default())
            .await
            .expect("start runner");
        let metrics = hydrate_snapshot(&runner, schema_sql, &snapshot)
            .await
            .expect("hydrate");

        assert_eq!(metrics.tables_updated, 1);

        let client = runner.client().expect("client");
        let row = client
            .query_one(
                "SELECT c.reltuples, c.relpages FROM pg_catalog.pg_class c \
                 JOIN pg_catalog.pg_namespace n ON c.relnamespace = n.oid \
                 WHERE n.nspname = 'public' AND c.relname = 'orders'",
                &[],
            )
            .await
            .expect("query pg_class");
        let reltuples: f32 = row.get(0);
        let relpages: i32 = row.get(1);
        assert_eq!(reltuples, 5_000_000.0);
        assert_eq!(relpages, 65536);

        runner.shutdown().await.expect("shutdown");
    }
}
