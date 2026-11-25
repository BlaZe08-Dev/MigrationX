# ADR: Five-State Verification Epistemics (`VerificationStatus`)

- **Status:** Accepted
- **Date:** 2026-11-25
- **Authors:** MigrationX Core Engineering

---

## 1. Context

Migration safety tools often collapse outcomes into a binary `pass`/`fail` or a three-state `pass`/`warn`/`fail`. That conflation is dangerous: if a SQL parser encounters unsupported dynamic SQL, if Docker is unavailable, or if no representative `active_queries` were provided for `PREPARE` checks, reporting `Passed` gives engineers false confidence that a migration is safe when it was merely not verified.

---

## 2. Decisions

### 2.1 Five-State `VerificationStatus` Enum
`crates/contracts/src/schema.rs` defines `VerificationStatus` (serialized as `SCREAMING_SNAKE_CASE`) with exactly five mutually distinct states:

1. **`StaticallyDetected`** — A hazard was identified deterministically by static AST analysis (`crates/static-analyzer` using `pg_query`), prior to or independent of live execution.
2. **`ObservedInSimulation`** — A hazard or failure was confirmed in a live ephemeral PostgreSQL container (`crates/simulator`), such as `migration_succeeded == false`, `pg_locks` polling observing `blocked_concurrent_queries == true`, or post-migration `PREPARE` failing with one or more `BrokenQuery` entries.
3. **`CouldNotVerify`** — The system could not verify whether the migration is safe or hazardous (e.g., empty `active_queries`, `PREPARE` transport/timeout error, Docker daemon unavailable, container boot timeout, target DB connection failure, or async cancellation). **Never** treated as equivalent to `Passed`.
4. **`Passed`** — Simulation completed with `migration_succeeded == true`, zero blocked concurrent queries, non-empty `active_queries`, and `prepare_verified == true` with zero `BrokenQuery` failures.
5. **`UnsupportedScenario`** — The migration or query uses syntax or constructs outside the system's current analysis capabilities (e.g., SQL parse failure, `DO` blocks, or dynamic `EXECUTE`).

### 2.2 Enforcement in Contract & Simulation Builders
`build_runtime_verdict` in `crates/simulator/src/simulate.rs` enforces that `Passed` is returned only when `migration_succeeded` is `true`, no lock observations blocked concurrent queries, `broken_queries` is empty, `active_queries_count > 0`, and `prepare_verified` is `true`; otherwise it returns `ObservedInSimulation` (for observed failures/contention/breakage) or `CouldNotVerify` (for incomplete verification).

---

## 3. Consequences

- **Positive:** Eliminates silent false negatives ("unknown != safe"); clearly separates static heuristics (`StaticallyDetected`) from empirical runtime proof (`ObservedInSimulation` / `Passed`); makes analysis boundaries explicit (`UnsupportedScenario`).
- **Negative / Trade-offs:** Downstream CLI/CI integrations and PR reporters must handle all five states explicitly rather than checking a single boolean, and contract tests must verify that `CouldNotVerify` is never mapped to a passing exit status.
