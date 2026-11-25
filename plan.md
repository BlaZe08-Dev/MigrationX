# MigrationX — 8-Week Parallel Execution Roadmap (Split C)

This roadmap defines the 8-week parallel execution plan for **MigrationX** under the **Split C** two-person team model (see [07-team-ownership.md](.agent/rules/07-team-ownership.md) and [06-git-and-worktrees.md](.agent/rules/06-git-and-worktrees.md)).

## Team Split & Worktree Boundaries

| Track | Worktree / Branch | Owner | Scope |
|---|---|---|---|
| **Core Engineering** | `feat/core-*` | Core Engineer | `crates/static-analyzer`, `crates/simulator`, security architecture, PostgreSQL correctness |
| **Product Engineering & Validation** | `feat/product-*` | Product Engineer | `crates/cli`, snapshot collector, PR/CLI reports, Expand-Contract recipes, deterministic fixtures, CI/CD |
| **Shared Boundary** | Cross-reviewed PRs | Both (mutual approval required) | `crates/contracts` (`SCHEMA_VERSION = "0.1.0"`) |

> **Governance Gate:** Per `.agent/rules/06-git-and-worktrees.md`, all feature branches merge to `main` every **48–72 hours**, and the **Deterministic Fixture Gate** (top 20 PostgreSQL migration hazards passing deterministically) must be green before any non-deterministic or AI/LLM extras are considered.

---

## Timeline Overview

```
Week:   1       2       3       4       5       6       7       8
      +---------------+---------------+---------------+---------------+
Core: | Contracts +   | Scale stats   | PREPARE N-1 + | tmpfs <12s +  |
      | AST + pg_locks| + 50M lock    | CONCURRENTLY  | benchmarks +  |
      | poller        | pileup proof  | splitting     | v1.0 release  |
      +---------------+---------------+---------------+---------------+
Prod: | Contracts +   | Snapshotter + | N-1 query     | Recipes +     |
      | Mock CLI +    | Top-20 hazard | fixtures +    | PR formatter +|
      | CI gates      | fixtures      | report UX     | v1.0 release  |
      +---------------+---------------+---------------+---------------+
```

---

## Weeks 1–2: Foundation — Contracts, AST Hazard Detector, Basic Docker `pg_locks` Poller & Mock CLI

### Core Engineering (`feat/core-*`)
- **Contracts (`crates/contracts`):** Finalize and freeze `v0.1.0` schemas (`Snapshot`, `TableStats`, `StaticFindings`, `HazardFinding`, `RuntimeVerdict`, `LockObservation`, `BrokenQuery`, `VerificationStatus`).
- **AST Hazard Detector (`crates/static-analyzer`):**
  - Integrate `pg_query` (v18) parser for SQL AST traversal.
  - Detect `ACCESS EXCLUSIVE` DDL hazards, table rewrites, and `CONCURRENTLY` inside transaction blocks.
  - Emit `StaticFindings` with `VerificationStatus::StaticallyDetected` or `VerificationStatus::UnsupportedScenario`.
- **Basic Docker `pg_locks` Poller (`crates/simulator`):**
  - Ephemeral container lifecycle via `testcontainers` using pinned `postgres:16.4-bookworm` tags.
  - Background poller querying `pg_locks` via `tokio-postgres` to record `LockObservation` (`LockMode`, `relation`, `held_ms`, `blocked_concurrent_queries`).
  - RAII teardown guard guaranteeing container cleanup on panic or timeout.

### Product Engineering & Validation (`feat/product-*`)
- **Mock CLI (`crates/cli`):**
  - Implement `clap` subcommands (`analyze`, `simulate`, `report`) wired to `contracts` JSON schemas.
  - Render deterministic mock outputs for `StaticFindings` and `RuntimeVerdict` to unblock report UX iteration.
- **CI & Tooling:**
  - Enforce `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and `cargo audit` on every PR.
- **Fixture Scaffolding:** Begin collecting baseline SQL migration fixtures and expected `HazardFinding` outputs.

### Milestone Exit Criteria (End of Week 2)
- End-to-end JSON flow works across `Snapshot` → `StaticFindings` → `RuntimeVerdict` using `contracts` `0.1.0`.
- Basic AST detector flags `ACCESS EXCLUSIVE` DDL; Docker poller captures real `pg_locks` rows in CI.

---

## Weeks 3–4: Scale — `pg_class` Statistics Injection & 50M-Row Lock Pileup Proof

### Core Engineering (`feat/core-*`)
- **Scale Statistics Injection (`crates/simulator`):**
  - Read `TableStats` (`estimated_row_count`, `total_size_bytes`, `has_triggers`, `has_foreign_key_refs`) from `Snapshot`.
  - Inject synthetic catalog statistics (`pg_class.reltuples`, `relpages`) and representative table/index structures into the ephemeral simulation container without exporting production rows.
- **50M-Row Lock Pileup Proof (`crates/simulator`):**
  - Implement concurrent DML workload generator (`tokio::select!` + explicit timeouts) running `SELECT`/`UPDATE` queries alongside the candidate migration.
  - Prove and observe lock queue pileups (`LockObservation.blocked_concurrent_queries = true`) when an `AccessExclusive` or `Share` lock stalls behind active queries, promoting findings to `VerificationStatus::ObservedInSimulation`.

### Product Engineering & Validation (`feat/product-*`)
- **Read-Only Snapshot Collector:**
  - Enforce `SET default_transaction_read_only = on;` on target DB sessions.
  - Query only `pg_catalog`, `information_schema`, `pg_class`, and `pg_stat_*` views; strip/redact credentials.
- **Deterministic Fixture Gate (Top 20 Hazards):**
  - Build and lock deterministic test fixtures covering the top 20 PostgreSQL migration hazards (table rewrites, missing `CONCURRENTLY`, FK lock escalation, volatile defaults, `SET NOT NULL` pre-PG12, etc.).
  - Gate CI on 100% pass rate for all 20 fixtures.

### Milestone Exit Criteria (End of Week 4)
- Simulation proves lock pileup behavior at 50M-row scale metadata without copying a single production row.
- Top-20 deterministic hazard fixture suite is green in CI.

---

## Weeks 5–6: Compatibility & Splitting — `PREPARE` N-1 Query Compatibility & Multi-Statement / `CONCURRENTLY` Splitting

### Core Engineering (`feat/core-*`)
- **`PREPARE` N-1 Query Compatibility (`crates/simulator`):**
  - Validate N-1 application queries against the post-migration schema inside the ephemeral container using PostgreSQL `PREPARE` statements.
  - Capture failing queries as `BrokenQuery` (`sql`, `error_message`, `sqlstate`) in `RuntimeVerdict`.
  - Explicitly distinguish unverified scenarios (`VerificationStatus::CouldNotVerify`) when query workloads are incomplete or unparseable.
- **Multi-Statement & `CONCURRENTLY` Splitting (`crates/static-analyzer` & `crates/simulator`):**
  - Split multi-statement migration scripts at transaction boundaries.
  - Detect `CREATE INDEX CONCURRENTLY`, `DROP INDEX CONCURRENTLY`, and `REINDEX CONCURRENTLY` statements and ensure they execute outside `BEGIN`/`COMMIT` transaction blocks.
  - Detect and flag failed concurrent index builds leaving `INVALID` indexes.

### Product Engineering & Validation (`feat/product-*`)
- **N-1 Query Corpus & Fixtures:**
  - Author realistic N-1 query workload fixtures (renamed columns, dropped tables, changed types, constraint violations) asserting exact `BrokenQuery.sqlstate` codes (e.g., `42703`, `42P01`).
- **Report UX Enhancements:**
  - Surface `VerificationStatus` badges (`Passed`, `ObservedInSimulation`, `StaticallyDetected`, `CouldNotVerify`, `UnsupportedScenario`) prominently in terminal and Markdown PR reports, ensuring `CouldNotVerify` is never rendered as safe.

### Milestone Exit Criteria (End of Week 6)
- `RuntimeVerdict` accurately reports broken N-1 queries with `sqlstate` and `error_message`.
- Multi-statement migrations with `CONCURRENTLY` execute cleanly with accurate transaction-boundary diagnostics.

---

## Weeks 7–8: Performance & Release — `tmpfs` `<12s` Optimization, Expand-Contract Recipes, Benchmarks & v1.0 Release

### Core Engineering (`feat/core-*`)
- **`tmpfs` `<12s` Simulation Optimization (`crates/simulator`):**
  - Tune ephemeral PostgreSQL container startup (`fsync=off`, `synchronous_commit=off`, `full_page_writes=off` inside disposable `1 GiB` `tmpfs` mount only) and connection pooling so full container boot + schema hydration + migration + teardown completes in **<12 seconds**.
- **Security & Failure-Injection Hardening:**
  - Complete failure-injection tests (DB connection drop, container OOM/timeout, panic cleanup) and security tests (network isolation, credential redaction, zero row export).

### Product Engineering & Validation (`feat/product-*`)
- **Expand-Contract Recipes:**
  - Build actionable remediation recipes mapped to `HazardFinding.remediation` (e.g., dual-write + backfill + `NOT VALID` constraint + `VALIDATE CONSTRAINT` + swap/drop).
- **Benchmarks & v1.0 Release:**
  - Publish end-to-end latency benchmarks (<12s simulation target) and accuracy benchmarks against the Top-20 fixture suite.
  - Finalize CLI UX, JSON/Markdown PR report templates, release artifacts, and tag `v1.0.0`.

### Milestone Exit Criteria (End of Week 8)
- End-to-end simulation latency `<12s` on `tmpfs` verified by benchmark suite.
- All 6 test categories (unit, contract, failure-injection, concurrency, Docker lifecycle, security) passing in CI.
- `v1.0.0` released.
