# ADR 0001: Rust Cargo Workspace & `tokio-postgres` Selection

- **Status:** Accepted
- **Date:** 2026-11-25
- **Authors:** MigrationX Core & Product Engineering

---

## 1. Context

MigrationX requires strict architectural separation between its shared data schemas, static SQL AST analyzer, ephemeral PostgreSQL container simulator, and CLI presentation layer. Additionally, the simulator must perform low-level PostgreSQL operations—including enforcing read-only sessions, polling `pg_locks` concurrently, injecting synthetic `pg_class` catalog statistics, and running `PREPARE` checks for N-1 query compatibility—while keeping build and runtime behavior deterministic.

---

## 2. Decisions

### 2.1 Cargo Workspace for Compile-Time Crate Boundaries
We structured the repository as a 4-crate Cargo workspace (`crates/contracts`, `crates/static-analyzer`, `crates/simulator`, `crates/cli`) rather than a single monolithic crate.

**Rationale:**
- **DAG Enforcement at Compile Time:** `contracts` has zero internal dependencies; `static-analyzer` and `simulator` depend only on `contracts`; `cli` depends on all three. Cargo forbids circular dependencies, making architectural violations a compile error.
- **Clear Team Ownership (Split C):** Core Engineering owns `static-analyzer` and `simulator`, Product Engineering owns `cli`, and `crates/contracts` serves as the cross-reviewed boundary.
- **Parallel Compilation & Scoped Testing:** Crate-level unit, contract, and Docker lifecycle tests can be compiled and executed independently.

### 2.2 `tokio-postgres` over `sqlx`
We selected `tokio-postgres` (`0.7`) as the PostgreSQL client driver instead of `sqlx`.

**Rationale:**
- **Low-Level Session & Protocol Control:** MigrationX needs direct control over raw connection lifecycles, `SET default_transaction_read_only = on`, explicit `PREPARE` / `DEALLOCATE` statements for N-1 query compatibility, and concurrent `pg_locks` polling without an ORM or compile-time query-checking macro layer.
- **No Compile-Time `DATABASE_URL` Coupling:** `sqlx`'s compile-time checked macros (`query!`) require either a live database or offline `.sqlx` caches during builds, which adds friction when targeting dynamic ephemeral containers and multiple PostgreSQL versions (15, 16).
- **Minimal Dependency Footprint:** `tokio-postgres` gives us direct async wire-protocol execution with `tokio`, matching our explicit timeout and cancellation-safety requirements in `.agent/rules/02-rust-standards.md`.

### 2.3 Linux `C_INCLUDE_PATH` Requirement for `pg_query` v18
`crates/static-analyzer` uses `pg_query` v18 (`libpg_query` bindings generated via `bindgen` / `clang`).

**Operational Note:**
- On Ubuntu/Debian Linux environments without full `libc6-dev` standard header symlinked into Clang's default search path, `bindgen` may fail during build with:
  `fatal error: 'stdbool.h' file not found`
- **Resolution:** Ensure `C_INCLUDE_PATH` includes GCC's internal include directory and `/usr/include` (e.g., `export C_INCLUDE_PATH="/usr/lib/gcc/x86_64-linux-gnu/13/include:/usr/include"`), or install `libc6-dev` / `build-essential` in CI images and developer workstations.
- We intentionally upgraded from `pg_query` `0.8` (which required `prost-build` + `cmake`) to `pg_query` `18` to eliminate the `cmake` build dependency and align with modern PostgreSQL grammar.

---

## 3. Consequences

- **Positive:** Compile-time enforcement of crate DAG; clean separation of `StaticFindings` and `RuntimeVerdict`; direct control over PostgreSQL sessions, locks, and `PREPARE` statements; no `cmake` required for builds.
- **Negative / Trade-offs:** Raw SQL queries in `simulator` are not compile-time checked by `sqlx` and must be thoroughly covered by Docker lifecycle and contract tests; Linux build environments must have `stdbool.h` discoverable by `clang`/`bindgen` via `C_INCLUDE_PATH` or `libc6-dev`.
