# MigrationX — System Architecture

## 1. System Overview

**MigrationX** is a PostgreSQL migration risk analysis and simulation platform built in Rust. It evaluates database schema migrations before they reach production by combining:

1. **Deterministic Static Analysis** (`crates/static-analyzer`) using `pg_query` (PostgreSQL's actual C parser) to inspect AST nodes for lock acquisition, table rewrites, and transaction boundary violations.
2. **Ephemeral Container Simulation** (`crates/simulator`) using `testcontainers` and `tokio-postgres` to execute migrations against a disposable, network-isolated PostgreSQL instance hydrated with production catalog statistics (never production rows).
3. **Strict Verification Semantics** (`crates/contracts`) that never conflate unverified assumptions (`VerificationStatus::CouldNotVerify`) with proven safety (`VerificationStatus::Passed`).

---

## 2. End-to-End Pipeline

```
+-------------------+
|     Target DB     |  (Read-only session: SET default_transaction_read_only = on)
| (pg_catalog only) |  (Zero production row exports)
+---------+---------+
          |
          v
+-------------------+
|   snapshot.json   |  contracts::Snapshot (schema_version, pg_version, TableStats)
+---------+---------+
          |
          +-------------------------------+
          |                               |
          v                               v
+-------------------+           +-------------------+
|  static-analyzer  |           |     simulator     |
|  (pg_query AST)   |           | (Docker + pg_locks|
+---------+---------+           |  + PREPARE N-1)   |
          |                     +---------+---------+
          |                               |
          v                               v
+-------------------+           +-------------------+
|  StaticFindings   |           | runtime_verdict   |
|  (HazardFinding)  |           | .json             |
+---------+---------+           | (RuntimeVerdict)  |
          |                     +---------+---------+
          |                               |
          +---------------+---------------+
                          |
                          v
                +-------------------+
                |   CLI / PR Report |  (crates/cli — thin orchestration & formatting)
                +-------------------+
```

---

## 3. Crate Boundaries & Dependency DAG

The workspace enforces a strict directed acyclic graph (DAG) with zero circular dependencies:

```
              +---------------+
              |  crates/cli   |
              +-------+-------+
                      |
         +------------+------------+
         |                         |
         v                         v
+--------------------+    +--------------------+
|  static-analyzer   |    |     simulator      |
+---------+----------+    +---------+----------+
          |                         |
          +------------+------------+
                       |
                       v
              +--------------------+
              |  crates/contracts  |
              +--------------------+
```

### Crate Responsibilities

| Crate | Dependencies | Owns | Prohibited |
|---|---|---|---|
| `contracts` (`migrationx-contracts`) | `serde`, `serde_json` | Versioned schemas (`SCHEMA_VERSION = "0.1.0"`), enums (`VerificationStatus`, `HazardSeverity`, `LockMode`), JSON round-trip invariants | Business logic, database I/O, Docker calls, CLI flags |
| `static-analyzer` (`migrationx-static-analyzer`) | `contracts`, `pg_query`, `tokio`, `thiserror`, `tracing` | SQL parsing, AST hazard rules, `CONCURRENTLY` / transaction boundary analysis, `StaticFindings` generation | Container management, target DB connections, CLI formatting |
| `simulator` (`migrationx-simulator`) | `contracts`, `tokio`, `tokio-postgres`, `testcontainers`, `thiserror`, `tracing` | Disposable container lifecycle, `pg_class` stats injection, `pg_locks` polling, `PREPARE` N-1 checks, `RuntimeVerdict` generation | SQL hazard AST rules, CLI output formatting |
| `cli` (`migrationx-cli`) | `contracts`, `static-analyzer`, `simulator`, `clap`, `anyhow`, `tokio`, `tracing` | Argument parsing, orchestrating analyzer + simulator, rendering JSON / PR Markdown reports | Domain logic, hazard detection rules, direct SQL parsing |

---

## 4. Contract Schemas (`crates/contracts/src/schema.rs`)

All cross-crate communication uses strongly-typed structs versioned by `SCHEMA_VERSION` (`"0.1.0"`):

- **`Snapshot`**: `schema_version`, `captured_at`, `pg_version`, `database_name`, `tables: HashMap<String, TableStats>`
- **`TableStats`**: `qualified_name`, `estimated_row_count`, `total_size_bytes`, `has_triggers`, `has_foreign_key_refs`
- **`StaticFindings`**: `schema_version`, `migration_sql`, `findings: Vec<HazardFinding>`, `overall_status: VerificationStatus`
- **`HazardFinding`**: `code`, `summary`, `severity: HazardSeverity` (`Critical`, `High`, `Medium`, `Low`), `offending_sql`, `status: VerificationStatus`, `remediation`
- **`RuntimeVerdict`**: `schema_version`, `migration_succeeded`, `duration_ms`, `lock_observations: Vec<LockObservation>`, `broken_queries: Vec<BrokenQuery>`, `overall_status: VerificationStatus`
- **`LockObservation`**: `mode: LockMode` (all 8 PostgreSQL lock modes), `relation`, `held_ms`, `blocked_concurrent_queries`
- **`BrokenQuery`**: `sql`, `error_message`, `sqlstate`
- **`VerificationStatus`**: `StaticallyDetected`, `ObservedInSimulation`, `CouldNotVerify`, `Passed`, `UnsupportedScenario`

---

## 5. Explicit Failure Modes

MigrationX treats partial or failed verification as an explicit, non-passing state rather than silently succeeding:

| Failure Mode | System Behavior | Resulting Status |
|---|---|---|
| **SQL Parse Failure / Unsupported Syntax** | `static-analyzer` fails to parse or encounters an unhandled AST node; returns typed `thiserror` error or flags finding. | `VerificationStatus::UnsupportedScenario` |
| **Target DB Connection / Auth Failure** | Snapshot collector aborts immediately with credential-redacted error; no simulation runs. | `VerificationStatus::CouldNotVerify` |
| **Container Startup / Docker Daemon Unavailable** | `simulator` times out via `tokio::time::timeout`, triggers RAII cleanup, and reports inability to simulate. | `VerificationStatus::CouldNotVerify` |
| **Migration Execution Error in Simulation** | `RuntimeVerdict.migration_succeeded = false`; error details captured; container destroyed. | `VerificationStatus::ObservedInSimulation` |
| **Lock Contention / Pileup Observed** | `pg_locks` poller records `LockObservation` with `blocked_concurrent_queries = true`. | `VerificationStatus::ObservedInSimulation` |
| **N-1 Query Breaks Post-Migration** | `PREPARE` fails; `BrokenQuery` populated with `sql`, `error_message`, and `sqlstate`. | `VerificationStatus::ObservedInSimulation` |
| **Async Timeout or Task Cancellation** | `tokio::select!` / `timeout` cancels simulation cleanly; RAII guard removes container; never reports `Passed`. | `VerificationStatus::CouldNotVerify` |
| **Advisory Locks in Migration** | PostgreSQL advisory locks do not surface as relation locks in `pg_locks` by default; documented limitation. | `VerificationStatus::CouldNotVerify` |
