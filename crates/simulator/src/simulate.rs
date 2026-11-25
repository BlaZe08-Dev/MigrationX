//! Concurrent migration simulation, 10ms `pg_locks`/`pg_stat_activity`
//! polling, and post-migration `PREPARE` compatibility checks.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use migrationx_contracts::{
    BrokenQuery, LockMode, LockObservation, RuntimeVerdict, VerificationStatus, SCHEMA_VERSION,
};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_postgres::Client;
use tracing::{debug, info, warn};

use crate::error::SimulatorError;
use crate::runner::PostgresRunner;

/// Default polling interval for `pg_locks` and `pg_stat_activity` (10 ms).
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Default number of concurrent `tokio-postgres` worker connections.
pub const DEFAULT_WORKER_CONCURRENCY: usize = 4;

/// Configuration for running a migration simulation against a [`PostgresRunner`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimulationConfig {
    /// Bounded number of concurrent worker connections executing `active_queries`.
    pub worker_concurrency: usize,
    /// Interval at which `pg_locks` and `pg_stat_activity` are sampled.
    pub poll_interval: Duration,
    /// Maximum duration allowed for the migration SQL execution.
    pub migration_timeout: Duration,
    /// Maximum duration allowed per `PREPARE` compatibility check.
    pub prepare_timeout: Duration,
}

impl Default for SimulationConfig {
    fn default() -> Self {
        Self {
            worker_concurrency: DEFAULT_WORKER_CONCURRENCY,
            poll_interval: DEFAULT_POLL_INTERVAL,
            migration_timeout: Duration::from_secs(30),
            prepare_timeout: Duration::from_secs(10),
        }
    }
}

/// Output of a simulation run, separating container boot latency from
/// migration simulation latency.
#[derive(Debug, Clone, PartialEq)]
pub struct SimulationOutput {
    /// Typed contract verdict for downstream reporting.
    pub verdict: RuntimeVerdict,
    /// Total number of blocked concurrent query observations sampled.
    pub blocked_query_samples: u64,
    /// Total number of `pg_locks` polling ticks completed.
    pub poll_ticks: u64,
}

/// Maps a PostgreSQL `pg_locks.mode` string to [`LockMode`].
pub fn parse_pg_lock_mode(mode: &str) -> Option<LockMode> {
    match mode {
        "AccessShareLock" => Some(LockMode::AccessShare),
        "RowShareLock" => Some(LockMode::RowShare),
        "RowExclusiveLock" => Some(LockMode::RowExclusive),
        "ShareUpdateExclusiveLock" => Some(LockMode::ShareUpdateExclusive),
        "ShareLock" => Some(LockMode::Share),
        "ShareRowExclusiveLock" => Some(LockMode::ShareRowExclusive),
        "ExclusiveLock" => Some(LockMode::Exclusive),
        "AccessExclusiveLock" => Some(LockMode::AccessExclusive),
        _ => None,
    }
}

/// Aggregates 10ms `pg_locks` + `pg_stat_activity` samples into [`LockObservation`]s.
#[derive(Debug, Default)]
pub struct LockAggregator {
    entries: HashMap<(LockMode, String), AggregatedLock>,
    /// Total number of polling ticks where at least one concurrent query was blocked.
    pub blocked_query_samples: u64,
    /// Total polling ticks recorded.
    pub poll_ticks: u64,
}

#[derive(Debug)]
struct AggregatedLock {
    sample_count: u64,
    blocked_concurrent_queries: bool,
}

impl LockAggregator {
    /// Records one polling tick's observed locks and whether any concurrent
    /// backend was blocked by the migration PID.
    pub fn record_sample(&mut self, locks: &[(LockMode, String)], blocked_count: u64) {
        self.poll_ticks += 1;
        let has_blocked = blocked_count > 0;
        if has_blocked {
            self.blocked_query_samples += 1;
        }

        for (mode, relation) in locks {
            let key = (*mode, relation.clone());
            let entry = self.entries.entry(key).or_insert(AggregatedLock {
                sample_count: 0,
                blocked_concurrent_queries: false,
            });
            entry.sample_count += 1;
            if has_blocked {
                entry.blocked_concurrent_queries = true;
            }
        }
    }

    /// Converts accumulated samples into [`LockObservation`]s using `poll_interval`.
    pub fn into_observations(self, poll_interval: Duration) -> Vec<LockObservation> {
        let interval_ms = poll_interval.as_millis().max(1) as u64;
        let mut obs: Vec<LockObservation> = self
            .entries
            .into_iter()
            .map(|((mode, relation), agg)| LockObservation {
                mode,
                relation,
                held_ms: agg.sample_count.saturating_mul(interval_ms),
                blocked_concurrent_queries: agg.blocked_concurrent_queries,
            })
            .collect();

        obs.sort_by(|a, b| {
            b.held_ms
                .cmp(&a.held_ms)
                .then_with(|| a.relation.cmp(&b.relation))
        });
        obs
    }
}

/// Computes the final [`RuntimeVerdict`] from simulation outcomes.
///
/// Never conflates unverified assumptions with proven safety:
/// - Migration failure, blocked concurrent queries, or broken N-1 queries
///   yield [`VerificationStatus::ObservedInSimulation`].
/// - If `active_queries` is empty or `prepare_verified` is `false`, returns
///   [`VerificationStatus::CouldNotVerify`] rather than `Passed`.
pub fn build_runtime_verdict(
    migration_succeeded: bool,
    duration_ms: u64,
    lock_observations: Vec<LockObservation>,
    broken_queries: Vec<BrokenQuery>,
    active_queries_count: usize,
    prepare_verified: bool,
) -> RuntimeVerdict {
    let any_blocked = lock_observations
        .iter()
        .any(|o| o.blocked_concurrent_queries);

    let overall_status = if !migration_succeeded || any_blocked || !broken_queries.is_empty() {
        VerificationStatus::ObservedInSimulation
    } else if active_queries_count == 0 || !prepare_verified {
        VerificationStatus::CouldNotVerify
    } else {
        VerificationStatus::Passed
    };

    RuntimeVerdict {
        schema_version: SCHEMA_VERSION.to_string(),
        migration_succeeded,
        duration_ms,
        lock_observations,
        broken_queries,
        overall_status,
    }
}

/// Executes `migration_sql` against `runner` while running a bounded pool of
/// concurrent `tokio-postgres` workers executing `active_queries` and polling
/// `pg_locks` + `pg_stat_activity` every 10ms, then runs `PREPARE` checks
/// post-migration on all `active_queries`.
///
/// # Cancellation Safety
///
/// All spawned worker and poller tasks are stopped via a `watch` channel and
/// aborted/awaited on exit or error; the underlying [`PostgresRunner`] guarantees
/// container teardown on `Drop`.
pub async fn run_migration_simulation(
    runner: &PostgresRunner,
    migration_sql: &str,
    active_queries: &[String],
    config: &SimulationConfig,
) -> Result<SimulationOutput, SimulatorError> {
    let migration_client = runner.client()?;
    let pid_row = tokio::time::timeout(
        config.migration_timeout,
        migration_client.query_one("SELECT pg_backend_pid()", &[]),
    )
    .await
    .map_err(|_| SimulatorError::OperationTimeout {
        operation: "query_migration_pid",
        timeout_secs: config.migration_timeout.as_secs(),
    })?
    .map_err(|err| SimulatorError::ConnectionFailed {
        message: format!("failed to query migration backend pid: {err}"),
    })?;
    let migration_pid: i32 = pid_row.get(0);

    let (stop_tx, stop_rx) = watch::channel(false);

    // Spawn bounded concurrent worker pool if active_queries is non-empty.
    let worker_count = if active_queries.is_empty() {
        0
    } else {
        config.worker_concurrency.clamp(1, 32)
    };

    let mut worker_handles: Vec<(JoinHandle<()>, JoinHandle<()>)> =
        Vec::with_capacity(worker_count);

    for worker_idx in 0..worker_count {
        let (client, conn_handle) = runner.connect().await?;
        let queries = active_queries.to_vec();
        let mut rx = stop_rx.clone();
        let query_timeout = runner.config().query_timeout;

        let handle = tokio::spawn(async move {
            let mut idx = worker_idx % queries.len();
            loop {
                if *rx.borrow() {
                    break;
                }
                let sql = &queries[idx];
                idx = (idx + 1) % queries.len();

                tokio::select! {
                    _ = rx.changed() => {
                        break;
                    }
                    res = tokio::time::timeout(query_timeout, client.batch_execute(sql)) => {
                        if let Ok(Err(err)) = res {
                            debug!(worker = worker_idx, error = %err, "worker query returned error during simulation");
                            if err.is_closed() || client.is_closed() {
                                break;
                            }
                        }
                    }
                }
            }
        });
        worker_handles.push((handle, conn_handle));
    }

    // Spawn 10ms pg_locks + pg_stat_activity poller on a dedicated connection.
    let (poller_client, poller_conn_handle) = runner.connect().await?;
    let poll_interval = if config.poll_interval.is_zero() {
        DEFAULT_POLL_INTERVAL
    } else {
        config.poll_interval
    };
    let mut poller_rx = stop_rx.clone();
    let poller_handle: JoinHandle<LockAggregator> = tokio::spawn(async move {
        let mut aggregator = LockAggregator::default();
        let mut ticker = tokio::time::interval(poll_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = poller_rx.changed() => {
                    break;
                }
                _ = ticker.tick() => {
                    match sample_locks_and_activity(&poller_client, migration_pid).await {
                        Ok((locks, blocked_count)) => {
                            aggregator.record_sample(&locks, blocked_count);
                        }
                        Err(err) => {
                            debug!(error = %err, "pg_locks/pg_stat_activity poll failed");
                        }
                    }
                }
            }
        }
        aggregator
    });

    // Execute the migration SQL and measure its duration.
    let migration_start = Instant::now();
    let migration_result = tokio::time::timeout(
        config.migration_timeout,
        migration_client.batch_execute(migration_sql),
    )
    .await;
    let duration_ms = migration_start.elapsed().as_millis() as u64;

    // Signal workers and poller to stop immediately after migration finishes.
    let _ = stop_tx.send(true);

    for (worker_h, conn_h) in worker_handles {
        worker_h.abort();
        let _ = worker_h.await;
        conn_h.abort();
    }

    let aggregator = match poller_handle.await {
        Ok(agg) => agg,
        Err(err) => {
            warn!(error = %err, "poller task failed to join");
            LockAggregator::default()
        }
    };
    poller_conn_handle.abort();

    let blocked_query_samples = aggregator.blocked_query_samples;
    let poll_ticks = aggregator.poll_ticks;
    let lock_observations = aggregator.into_observations(poll_interval);

    let migration_succeeded = match migration_result {
        Ok(Ok(())) => true,
        Ok(Err(err)) => {
            warn!(error = %err, "migration SQL failed in simulation");
            false
        }
        Err(_) => {
            return Err(SimulatorError::OperationTimeout {
                operation: "execute_migration",
                timeout_secs: config.migration_timeout.as_secs(),
            });
        }
    };

    // Run PREPARE compatibility checks post-migration on all active_queries.
    let (broken_queries, prepare_verified) = if migration_succeeded && !active_queries.is_empty() {
        check_prepare_compatibility(migration_client, active_queries, config.prepare_timeout).await
    } else {
        (Vec::new(), false)
    };

    let verdict = build_runtime_verdict(
        migration_succeeded,
        duration_ms,
        lock_observations,
        broken_queries,
        active_queries.len(),
        prepare_verified,
    );

    info!(
        migration_succeeded = verdict.migration_succeeded,
        duration_ms = verdict.duration_ms,
        lock_observations = verdict.lock_observations.len(),
        broken_queries = verdict.broken_queries.len(),
        overall_status = %verdict.overall_status,
        "completed migration simulation"
    );

    Ok(SimulationOutput {
        verdict,
        blocked_query_samples,
        poll_ticks,
    })
}

async fn sample_locks_and_activity(
    client: &Client,
    migration_pid: i32,
) -> Result<(Vec<(LockMode, String)>, u64), tokio_postgres::Error> {
    const LOCKS_SQL: &str = "\
        SELECT l.mode, COALESCE(n.nspname || '.' || c.relname, l.locktype) AS relation \
        FROM pg_catalog.pg_locks l \
        LEFT JOIN pg_catalog.pg_class c ON l.relation = c.oid \
        LEFT JOIN pg_catalog.pg_namespace n ON c.relnamespace = n.oid \
        WHERE l.pid = $1 \
          AND l.granted = true \
          AND l.relation IS NOT NULL";

    const BLOCKED_SQL: &str = "\
        SELECT COUNT(*)::bigint \
        FROM pg_catalog.pg_stat_activity a \
        WHERE a.pid <> $1 \
          AND a.wait_event_type = 'Lock' \
          AND $1 = ANY(pg_catalog.pg_blocking_pids(a.pid))";

    let lock_rows = client.query(LOCKS_SQL, &[&migration_pid]).await?;
    let mut locks = Vec::with_capacity(lock_rows.len());
    for row in lock_rows {
        let mode_str: &str = row.get(0);
        let relation: String = row.get(1);
        if let Some(mode) = parse_pg_lock_mode(mode_str) {
            locks.push((mode, relation));
        }
    }

    let blocked_row = client.query_one(BLOCKED_SQL, &[&migration_pid]).await?;
    let blocked_count: i64 = blocked_row.get(0);

    Ok((locks, blocked_count.max(0) as u64))
}

/// Runs `PREPARE` compatibility checks against all `active_queries` post-migration.
///
/// Returns `(broken_queries, prepare_verified)`. If any check fails due to a
/// transport/timeout error rather than a PostgreSQL SQL error, `prepare_verified`
/// is returned as `false` so the caller reports `CouldNotVerify`.
pub async fn check_prepare_compatibility(
    client: &Client,
    active_queries: &[String],
    prepare_timeout: Duration,
) -> (Vec<BrokenQuery>, bool) {
    let mut broken = Vec::new();
    let mut all_verified = true;

    for sql in active_queries {
        match tokio::time::timeout(prepare_timeout, client.prepare(sql)).await {
            Ok(Ok(_stmt)) => {}
            Ok(Err(err)) => {
                if let Some(db_err) = err.as_db_error() {
                    broken.push(BrokenQuery {
                        sql: sql.clone(),
                        error_message: db_err.message().to_string(),
                        sqlstate: Some(db_err.code().code().to_string()),
                    });
                } else {
                    warn!(error = %err, "PREPARE failed with non-DB transport error");
                    all_verified = false;
                }
            }
            Err(_) => {
                warn!("PREPARE compatibility check timed out");
                all_verified = false;
            }
        }
    }

    (broken, all_verified)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hydrate::hydrate_snapshot;
    use crate::runner::RunnerConfig;
    use migrationx_contracts::{Snapshot, TableStats};

    #[test]
    fn test_simulate_parse_pg_lock_mode_covers_all_eight_modes() {
        let cases = [
            ("AccessShareLock", Some(LockMode::AccessShare)),
            ("RowShareLock", Some(LockMode::RowShare)),
            ("RowExclusiveLock", Some(LockMode::RowExclusive)),
            (
                "ShareUpdateExclusiveLock",
                Some(LockMode::ShareUpdateExclusive),
            ),
            ("ShareLock", Some(LockMode::Share)),
            ("ShareRowExclusiveLock", Some(LockMode::ShareRowExclusive)),
            ("ExclusiveLock", Some(LockMode::Exclusive)),
            ("AccessExclusiveLock", Some(LockMode::AccessExclusive)),
            ("UnknownLock", None),
        ];
        for (input, expected) in cases {
            assert_eq!(parse_pg_lock_mode(input), expected, "mode: {input}");
        }
    }

    #[test]
    fn test_simulate_lock_aggregator_computes_held_ms_and_blocked_flag() {
        let mut agg = LockAggregator::default();
        let locks = vec![(LockMode::AccessExclusive, "public.users".to_string())];
        agg.record_sample(&locks, 0);
        agg.record_sample(&locks, 2);
        agg.record_sample(&locks, 1);

        assert_eq!(agg.poll_ticks, 3);
        assert_eq!(agg.blocked_query_samples, 2);

        let obs = agg.into_observations(Duration::from_millis(10));
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].mode, LockMode::AccessExclusive);
        assert_eq!(obs[0].relation, "public.users");
        assert_eq!(obs[0].held_ms, 30);
        assert!(obs[0].blocked_concurrent_queries);
    }

    #[test]
    fn test_simulate_build_runtime_verdict_semantics() {
        let passed = build_runtime_verdict(true, 50, vec![], vec![], 2, true);
        assert_eq!(passed.overall_status, VerificationStatus::Passed);

        let unverified_empty = build_runtime_verdict(true, 50, vec![], vec![], 0, false);
        assert_eq!(
            unverified_empty.overall_status,
            VerificationStatus::CouldNotVerify
        );

        let unverified_prepare = build_runtime_verdict(true, 50, vec![], vec![], 2, false);
        assert_eq!(
            unverified_prepare.overall_status,
            VerificationStatus::CouldNotVerify
        );

        let blocked = build_runtime_verdict(
            true,
            50,
            vec![LockObservation {
                mode: LockMode::AccessExclusive,
                relation: "public.users".to_string(),
                held_ms: 40,
                blocked_concurrent_queries: true,
            }],
            vec![],
            2,
            true,
        );
        assert_eq!(
            blocked.overall_status,
            VerificationStatus::ObservedInSimulation
        );

        let broken = build_runtime_verdict(
            true,
            50,
            vec![],
            vec![BrokenQuery {
                sql: "SELECT email FROM public.users".to_string(),
                error_message: "column \"email\" does not exist".to_string(),
                sqlstate: Some("42703".to_string()),
            }],
            1,
            true,
        );
        assert_eq!(
            broken.overall_status,
            VerificationStatus::ObservedInSimulation
        );

        let failed_mig = build_runtime_verdict(false, 10, vec![], vec![], 1, false);
        assert_eq!(
            failed_mig.overall_status,
            VerificationStatus::ObservedInSimulation
        );
    }

    #[tokio::test]
    #[ignore = "requires docker"]
    async fn test_simulate_concurrent_dml_detects_access_exclusive_and_broken_query() {
        let mut tables = HashMap::new();
        tables.insert(
            "public.users".to_string(),
            TableStats {
                qualified_name: "public.users".to_string(),
                estimated_row_count: 100_000,
                total_size_bytes: 16 * 1024 * 1024,
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
        let schema_sql = "CREATE TABLE public.users (id BIGSERIAL PRIMARY KEY, email TEXT);";
        let migration_sql =
            "BEGIN; ALTER TABLE public.users DROP COLUMN email; SELECT pg_sleep(0.15); COMMIT;";
        let active_queries = vec!["SELECT id, email FROM public.users LIMIT 1".to_string()];

        let runner = PostgresRunner::start(RunnerConfig::default())
            .await
            .expect("start runner");
        hydrate_snapshot(&runner, schema_sql, &snapshot)
            .await
            .expect("hydrate");

        let output = run_migration_simulation(
            &runner,
            migration_sql,
            &active_queries,
            &SimulationConfig::default(),
        )
        .await
        .expect("run simulation");

        assert!(output.verdict.migration_succeeded);
        assert_eq!(
            output.verdict.overall_status,
            VerificationStatus::ObservedInSimulation
        );
        assert!(output
            .verdict
            .lock_observations
            .iter()
            .any(|o| o.mode == LockMode::AccessExclusive && o.blocked_concurrent_queries));
        assert_eq!(output.verdict.broken_queries.len(), 1);
        assert_eq!(
            output.verdict.broken_queries[0].sqlstate.as_deref(),
            Some("42703")
        );

        runner.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    #[ignore = "requires docker"]
    async fn test_simulate_failure_injection_bad_migration_sql_returns_observed_in_simulation() {
        let runner = PostgresRunner::start(RunnerConfig::default())
            .await
            .expect("start runner");

        let output = run_migration_simulation(
            &runner,
            "ALTER TABLE nonexistent_table ADD COLUMN x INT;",
            &["SELECT 1".to_string()],
            &SimulationConfig::default(),
        )
        .await
        .expect("simulation returns verdict");

        assert!(!output.verdict.migration_succeeded);
        assert_eq!(
            output.verdict.overall_status,
            VerificationStatus::ObservedInSimulation
        );

        runner.shutdown().await.expect("shutdown");
    }
}
