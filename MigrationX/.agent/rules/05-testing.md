# 05 — Testing

## Required Test Categories

Every module must have tests from **all applicable** categories below.

### 1. Unit Tests
- Pure-logic tests for every public function.
- JSON serialization round-trips for all contract types.
- Edge cases: empty inputs, boundary values, malformed data.

### 2. Contract Tests
- Verify that types in `contracts` serialize/deserialize identically across
  schema versions.
- Breaking schema changes require a migration path and version bump.

### 3. Failure-Injection Tests
- Simulate database connection failures.
- Simulate container startup failures.
- Simulate timeout scenarios (query timeout, container timeout).
- Verify that errors propagate correctly and are human-readable.

### 4. Concurrency Tests
- Verify lock detection under concurrent DML workloads.
- Test that `CREATE INDEX CONCURRENTLY` interactions are correctly observed.
- Use `tokio::test` with multi-threaded runtime for async concurrency tests.

### 5. Docker Lifecycle Tests
- Container creation, health check, query execution, teardown.
- Verify cleanup on panic (containers must not leak).
- Test with multiple PostgreSQL versions (15, 16).

### 6. Security Tests
- Verify read-only enforcement on target database connections.
- Verify no row data appears in any output artifact.
- Verify credential redaction in log output and error messages.
- Verify container resource limits are applied.

## Test Naming Convention

```
test_<module>_<scenario>_<expected_behavior>
```

Example: `test_lock_observer_concurrent_dml_detects_access_exclusive`

## CI Requirements

- All tests must pass on every PR.
- Tests must not depend on external network access.
- Docker-dependent tests are gated behind a `docker` feature flag or
  `#[ignore]` with CI explicitly running them.
