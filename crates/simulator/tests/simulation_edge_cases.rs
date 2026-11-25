//! Edge-case integration tests for the MigrationX simulator.
//!
//! Covers:
//! 1. Migration `lock_timeout` failing fast when a worker holds a conflicting lock.
//! 2. Worker disconnection and query cancellation mid-simulation without panicking
//!    or leaking Docker containers.
//! 3. Static vs. runtime disagreement (`CREATE INDEX` without `CONCURRENTLY` on a
//!    table with `estimated_row_count = 0` vs. `50_000_000` injected into `pg_class`).

use std::collections::HashMap;
use std::process::Command;
use std::time::{Duration, Instant};

use migrationx_contracts::{Snapshot, TableStats, VerificationStatus, SCHEMA_VERSION};
use migrationx_simulator::{
    hydrate_snapshot, run_migration_simulation, PostgresRunner, RunnerConfig, SimulationConfig,
};
use migrationx_static_analyzer::analyze_migration;

fn build_orders_snapshot(estimated_row_count: i64, total_size_bytes: u64) -> Snapshot {
    let mut tables = HashMap::new();
    tables.insert(
        "public.orders".to_string(),
        TableStats {
            qualified_name: "public.orders".to_string(),
            estimated_row_count,
            total_size_bytes,
            has_triggers: false,
            has_foreign_key_refs: false,
        },
    );
    Snapshot {
        schema_version: SCHEMA_VERSION.to_string(),
        captured_at: "2026-11-25T08:00:00Z".to_string(),
        pg_version: "16.4".to_string(),
        database_name: "prod".to_string(),
        tables,
    }
}

fn container_exists_in_docker(container_id: &str) -> bool {
    let output = Command::new("docker")
        .args(["ps", "-aq", "--filter", &format!("id={container_id}")])
        .output()
        .expect("run docker ps");
    assert!(output.status.success(), "docker ps failed");
    !String::from_utf8_lossy(&output.stdout).trim().is_empty()
}

#[tokio::test]
#[ignore = "requires docker"]
async fn test_simulation_edge_cases_lock_timeout_fails_fast_when_worker_holds_lock() {
    let runner = PostgresRunner::start(RunnerConfig::default())
        .await
        .expect("start runner");

    let schema_sql = "CREATE TABLE public.orders (id BIGSERIAL PRIMARY KEY, total INT);";
    let snapshot = build_orders_snapshot(1_000, 1024 * 1024);
    hydrate_snapshot(&runner, schema_sql, &snapshot)
        .await
        .expect("hydrate");

    // Open an independent worker connection that holds an AccessShareLock on
    // public.orders inside an open transaction, blocking ACCESS EXCLUSIVE DDL.
    let (lock_client, lock_conn_handle) = runner.connect().await.expect("connect lock holder");
    lock_client
        .batch_execute("BEGIN; SELECT * FROM public.orders LIMIT 1;")
        .await
        .expect("acquire lock in worker transaction");

    let migration_sql =
        "SET lock_timeout = '50ms'; ALTER TABLE public.orders ADD COLUMN note TEXT;";
    let sim_cfg = SimulationConfig {
        worker_concurrency: 1,
        poll_interval: Duration::from_millis(10),
        migration_timeout: Duration::from_secs(10),
        prepare_timeout: Duration::from_secs(5),
    };

    let start = Instant::now();
    let output =
        run_migration_simulation(&runner, migration_sql, &["SELECT 1".to_string()], &sim_cfg)
            .await
            .expect("simulation should return verdict on lock_timeout");
    let elapsed = start.elapsed();

    assert!(
        !output.verdict.migration_succeeded,
        "migration must fail when lock_timeout = '50ms' expires"
    );
    assert_eq!(
        output.verdict.overall_status,
        VerificationStatus::ObservedInSimulation
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "lock_timeout should fail fast, took {elapsed:?}"
    );

    let _ = lock_client.batch_execute("ROLLBACK;").await;
    drop(lock_client);
    lock_conn_handle.abort();
    runner.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires docker"]
async fn test_simulation_edge_cases_worker_disconnect_and_cancel_no_panic_or_leak() {
    let runner = PostgresRunner::start(RunnerConfig::default())
        .await
        .expect("start runner");
    let container_id = runner
        .container_id()
        .expect("container id must exist")
        .to_string();

    let schema_sql = "CREATE TABLE public.orders (id BIGSERIAL PRIMARY KEY, total INT);";
    let snapshot = build_orders_snapshot(10_000, 8 * 1024 * 1024);
    hydrate_snapshot(&runner, schema_sql, &snapshot)
        .await
        .expect("hydrate");

    // Identify the migration backend PID so the terminator task only cancels/kills
    // concurrent worker backends.
    let migration_pid: i32 = runner
        .client()
        .expect("client")
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .expect("query migration pid")
        .get(0);

    let (killer_client, killer_conn_handle) = runner.connect().await.expect("connect killer");
    let killer_pid: i32 = killer_client
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .expect("query killer pid")
        .get(0);

    // Spawn a task that cancels and terminates active worker queries mid-simulation.
    let killer_task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(60)).await;
        let _ = killer_client
            .execute(
                "SELECT pg_cancel_backend(pid) FROM pg_stat_activity \
                 WHERE datname = current_database() \
                   AND pid <> $1 AND pid <> $2 \
                   AND backend_type = 'client backend' \
                   AND query LIKE '%pg_sleep%'",
                &[&migration_pid, &killer_pid],
            )
            .await;

        tokio::time::sleep(Duration::from_millis(40)).await;
        let _ = killer_client
            .execute(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
                 WHERE datname = current_database() \
                   AND pid <> $1 AND pid <> $2 \
                   AND backend_type = 'client backend' \
                   AND query LIKE '%pg_sleep%'",
                &[&migration_pid, &killer_pid],
            )
            .await;
    });

    let migration_sql =
        "BEGIN; ALTER TABLE public.orders ADD COLUMN tag TEXT; SELECT pg_sleep(0.25); COMMIT;";
    let active_queries =
        vec!["SELECT id FROM public.orders LIMIT 1; SELECT pg_sleep(0.2);".to_string()];
    let sim_cfg = SimulationConfig {
        worker_concurrency: 3,
        poll_interval: Duration::from_millis(10),
        migration_timeout: Duration::from_secs(10),
        prepare_timeout: Duration::from_secs(5),
    };

    let output = run_migration_simulation(&runner, migration_sql, &active_queries, &sim_cfg)
        .await
        .expect("simulation must not panic or error when workers disconnect");

    let _ = killer_task.await;
    killer_conn_handle.abort();

    assert!(output.verdict.migration_succeeded);
    assert_eq!(
        output.verdict.overall_status,
        VerificationStatus::ObservedInSimulation
    );

    runner.shutdown().await.expect("shutdown");
    assert!(
        !container_exists_in_docker(&container_id),
        "container {container_id} leaked after shutdown"
    );
}

#[tokio::test]
#[ignore = "requires docker"]
async fn test_simulation_edge_cases_static_vs_runtime_disagreement_create_index() {
    let schema_sql = "CREATE TABLE public.orders (id BIGSERIAL PRIMARY KEY, total INT);";
    let migration_sql = "CREATE INDEX idx_orders_total ON public.orders (total);";
    let active_queries = vec!["SELECT id, total FROM public.orders LIMIT 1".to_string()];

    // Static analysis has no row-count context and flags CREATE INDEX without
    // CONCURRENTLY regardless of table size.
    let static_findings = analyze_migration(migration_sql).expect("static analysis");
    assert_eq!(
        static_findings.overall_status,
        VerificationStatus::StaticallyDetected
    );
    assert!(
        !static_findings.findings.is_empty(),
        "static analyzer must flag CREATE INDEX without CONCURRENTLY"
    );

    // Case 1: Empty table (estimated_row_count = 0).
    let runner_empty = PostgresRunner::start(RunnerConfig::default())
        .await
        .expect("start empty runner");
    let snapshot_empty = build_orders_snapshot(0, 0);
    hydrate_snapshot(&runner_empty, schema_sql, &snapshot_empty)
        .await
        .expect("hydrate empty");

    let row_empty = runner_empty
        .client()
        .expect("client")
        .query_one(
            "SELECT c.reltuples, c.relpages FROM pg_catalog.pg_class c \
             JOIN pg_catalog.pg_namespace n ON c.relnamespace = n.oid \
             WHERE n.nspname = 'public' AND c.relname = 'orders'",
            &[],
        )
        .await
        .expect("query pg_class empty");
    assert_eq!(row_empty.get::<_, f32>(0), 0.0);
    assert_eq!(row_empty.get::<_, i32>(1), 0);

    let output_empty = run_migration_simulation(
        &runner_empty,
        migration_sql,
        &active_queries,
        &SimulationConfig::default(),
    )
    .await
    .expect("simulate empty");
    assert!(output_empty.verdict.migration_succeeded);
    runner_empty.shutdown().await.expect("shutdown empty");

    // Case 2: Large table statistics (50_000_000 rows, 4 GiB) injected into pg_class.
    let runner_large = PostgresRunner::start(RunnerConfig::default())
        .await
        .expect("start large runner");
    let snapshot_large = build_orders_snapshot(50_000_000, 4 * 1024 * 1024 * 1024);
    hydrate_snapshot(&runner_large, schema_sql, &snapshot_large)
        .await
        .expect("hydrate large");

    let row_large = runner_large
        .client()
        .expect("client")
        .query_one(
            "SELECT c.reltuples, c.relpages FROM pg_catalog.pg_class c \
             JOIN pg_catalog.pg_namespace n ON c.relnamespace = n.oid \
             WHERE n.nspname = 'public' AND c.relname = 'orders'",
            &[],
        )
        .await
        .expect("query pg_class large");
    assert_eq!(row_large.get::<_, f32>(0), 50_000_000.0);
    assert_eq!(row_large.get::<_, i32>(1), 524_288);

    // Zero-row runtime simulation still builds the index against 0 physical heap
    // pages, demonstrating why static analysis (StaticallyDetected) disagrees
    // with zero-row runtime duration even when pg_class reflects 50M rows.
    let output_large = run_migration_simulation(
        &runner_large,
        migration_sql,
        &active_queries,
        &SimulationConfig::default(),
    )
    .await
    .expect("simulate large");
    assert!(output_large.verdict.migration_succeeded);
    assert!(output_large.verdict.broken_queries.is_empty());
    runner_large.shutdown().await.expect("shutdown large");
}
