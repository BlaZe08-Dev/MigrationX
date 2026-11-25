# ADR: Zero-Row `pg_class` Metadata Injection

- **Status:** Accepted
- **Date:** 2026-11-25
- **Authors:** MigrationX Core Engineering

---

## 1. Context

PostgreSQL query planner decisions and migration risk assessments depend heavily on table scale (`reltuples` and `relpages`), yet copying production table rows into an ephemeral simulation container is unacceptable for security, privacy, compliance, and speed reasons. MigrationX needs a way to hydrate a disposable `postgres:16.4-bookworm` container with realistic catalog scale without transferring any production row data.

---

## 2. Decisions

### 2.1 DDL-Only Schema Replay + `pg_catalog.pg_class` Statistics Injection
In `crates/simulator/src/hydrate.rs`, `hydrate_snapshot` performs two distinct phases without copying any production rows:

1. **Schema Replay (`replay_schema`):** Executes the target `schema_sql` via `client.batch_execute(schema_sql)` inside the ephemeral container.
2. **Catalog Statistics Injection (`inject_table_stats`):** Iterates over `Snapshot.tables` (`HashMap<String, TableStats>`) and updates `pg_catalog.pg_class` (joined with `pg_catalog.pg_namespace` for `c.relkind IN ('r', 'p', 'm')`) using parameterized queries:
   - `TableStats.estimated_row_count` -> `pg_class.reltuples` (`f32` / `float4`) via `estimate_reltuples`, clamping values below `-1` to `-1.0` (PostgreSQL's unanalyzed sentinel).
   - `TableStats.total_size_bytes` -> `pg_class.relpages` (`i32` / `int4`) via `estimate_relpages`, dividing by `POSTGRES_BLCKSZ = 8192` bytes and saturating at `i32::MAX`.

### 2.2 Separate Latency Accounting
`HydrationMetrics` records `schema_replay_latency`, `stats_injection_latency`, `total_simulation_latency`, and `tables_updated` separately from `PostgresRunner::boot_latency()`, keeping container startup overhead distinct from schema/statistics hydration cost.

---

## 3. Consequences

- **Positive:** Zero production row data ever leaves the target database or enters the simulation container; hydration completes in milliseconds regardless of whether production tables hold millions or billions of rows; planner estimates (`EXPLAIN`) inside the simulator reflect production table sizes.
- **Negative / Trade-offs:** Because simulated tables remain physically empty, operations whose runtime scales with physical tuple count (such as full table rewrites or non-concurrent index builds) execute quickly in simulation rather than reproducing production wall-clock duration. MigrationX mitigates this by pairing runtime lock/compatibility observation with deterministic static analysis (`crates/static-analyzer`) and `TableStats` metadata (`estimated_row_count`, `has_triggers`, `has_foreign_key_refs`).
