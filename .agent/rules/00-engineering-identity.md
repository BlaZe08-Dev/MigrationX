# 00 — Engineering Identity & Phased Workflow

## Principal Engineering Bar

Every MigrationX contribution must meet a **principal-engineer standard**:
production-grade correctness, explicit trade-off documentation, and zero
hand-waving. If a behavior cannot be verified, it must be reported as
`CouldNotVerify` — never silently assumed safe.

## Phased Workflow

All work follows five mandatory phases. No phase may be skipped.

### Phase 1 — Inspect
- Read the existing codebase, schema contracts, and test suite.
- Identify every assumption about PostgreSQL behavior.
- List unknowns explicitly.

### Phase 2 — Plan
- Write a concise implementation plan **before** touching code.
- State what will change, what won't, and why.
- Surface any risk or ambiguity to the team for review.

### Phase 3 — Implement
- Write code that compiles and passes `cargo clippy -- -D warnings`.
- Every public type and function has doc comments.
- Every contract type has serialization round-trip tests.

### Phase 4 — Verify
- Run the full test suite: `cargo test --workspace`.
- Run `cargo fmt --check` and `cargo clippy -- -D warnings`.
- For PostgreSQL-dependent logic, verify against a real Postgres container.

### Phase 5 — Report
- Summarize what was done, what was verified, and what remains unverified.
- Never claim a hazard is safe without simulation evidence.
- Tag unverified items with `CouldNotVerify` or `UnsupportedScenario`.

## No Unverified Claims

- A finding marked `Passed` **must** have been confirmed by simulation.
- A finding marked `StaticallyDetected` **must** cite the specific AST node or SQL pattern.
- `CouldNotVerify` is always preferable to a false `Passed`.
