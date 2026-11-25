//! Latency benchmark integration test for the MigrationX simulator.
//!
//! Measures and prints separate timings for:
//! 1. Container boot latency (`runner.boot_latency()`)
//! 2. Schema replay + `pg_class` stats injection latency
//! 3. Migration execution + 10ms `pg_locks` polling + `PREPARE` check latency
//!
//! Run with:
//! `cargo test -p migrationx-simulator --test latency_bench -- --ignored --nocapture`

use std::collections::HashMap;
use std::time::{Duration, Instant};

use migrationx_contracts::{Snapshot, TableStats, VerificationStatus, SCHEMA_VERSION};
use migrationx_simulator::{
    hydrate_snapshot, run_migration_simulation, PostgresRunner, RunnerConfig, SimulationConfig,
    SimulatorError,
};

fn build_benchmark_snapshot() -> Snapshot {
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
    tables.insert(
        "public.users".to_string(),
        TableStats {
            qualified_name: "public.users".to_string(),
            estimated_row_count: 1_000_000,
            total_size_bytes: 128 * 1024 * 1024,
            has_triggers: false,
            has_foreign_key_refs: true,
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

#[tokio::test]
#[ignore = "requires docker"]
async fn bench_simulator_latency_breakdown() {
    let runner = PostgresRunner::start(RunnerConfig::default())
        .await
        .expect("start ephemeral postgres container");
    let boot_latency = runner.boot_latency();

    let schema_sql = "\
        CREATE TABLE public.users (id BIGSERIAL PRIMARY KEY, email TEXT NOT NULL);\n\
        CREATE TABLE public.orders (id BIGSERIAL PRIMARY KEY, user_id BIGINT REFERENCES public.users(id), total INT NOT NULL);";
    let snapshot = build_benchmark_snapshot();

    let migration_sql = "ALTER TABLE public.orders ADD COLUMN notes TEXT;";
    let active_queries = vec![
        "SELECT id, email FROM public.users LIMIT 1".to_string(),
        "SELECT id, user_id, total FROM public.orders LIMIT 1".to_string(),
    ];

    // Execute hydration and simulation, capturing any error before explicit
    // shutdown so container cleanup is guaranteed even on failure or panic.
    let bench_result: Result<_, SimulatorError> = async {
        let hydration = hydrate_snapshot(&runner, schema_sql, &snapshot).await?;

        let sim_start = Instant::now();
        let sim_output = run_migration_simulation(
            &runner,
            migration_sql,
            &active_queries,
            &SimulationConfig::default(),
        )
        .await?;
        let sim_total_latency = sim_start.elapsed();

        Ok((hydration, sim_output, sim_total_latency))
    }
    .await;

    // Guarantee explicit container removal before asserting on results.
    // If an earlier panic occurs, `PostgresRunner::drop` also performs RAII cleanup.
    runner
        .shutdown()
        .await
        .expect("shutdown postgres container");

    let (hydration, sim_output, sim_total_latency) =
        bench_result.expect("simulation benchmark should succeed");

    println!("=== MigrationX Simulator Latency Breakdown ===");
    println!(
        "1. Container boot latency:                      {:.2} ms",
        boot_latency.as_secs_f64() * 1000.0
    );
    println!(
        "2. Schema replay + pg_class stats injection:    {:.2} ms (replay: {:.2} ms, stats: {:.2} ms, tables: {})",
        hydration.total_simulation_latency.as_secs_f64() * 1000.0,
        hydration.schema_replay_latency.as_secs_f64() * 1000.0,
        hydration.stats_injection_latency.as_secs_f64() * 1000.0,
        hydration.tables_updated
    );
    println!(
        "3. Migration + 10ms pg_locks poll + PREPARE:    {:.2} ms (migration_exec: {} ms, poll_ticks: {}, blocked_samples: {})",
        sim_total_latency.as_secs_f64() * 1000.0,
        sim_output.verdict.duration_ms,
        sim_output.poll_ticks,
        sim_output.blocked_query_samples
    );

    assert!(boot_latency > Duration::ZERO);
    assert_eq!(hydration.tables_updated, 2);
    assert!(hydration.total_simulation_latency > Duration::ZERO);
    assert!(sim_total_latency > Duration::ZERO);
    assert!(sim_output.verdict.migration_succeeded);
    assert_eq!(
        sim_output.verdict.overall_status,
        VerificationStatus::Passed
    );
}
