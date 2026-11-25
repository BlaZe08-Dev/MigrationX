# 04 — Security

## Zero Production Row Exports

- **Never** export, copy, log, or snapshot production row data.
- Snapshots capture only schema metadata, table statistics, and configuration — never row contents.
- All queries against the target database must be in `information_schema`, `pg_catalog`,
  or use `pg_class`/`pg_stat_*` views only.

## Read-Only Target Database Sessions

- Every connection to the target (production) database must enforce:
  ```sql
  SET default_transaction_read_only = on;
  ```
- The connection string must use a role with minimal privileges (`pg_read_all_stats`
  or equivalent read-only role).

## Disposable Containers Only

- Simulation containers are **ephemeral** — created for one test run, destroyed after.
- No persistent volumes for simulation containers.
- No shared containers across test runs.

## Pinned PostgreSQL Image Tags

- Never use `postgres:latest` or `postgres:16`.
- Always pin to a full version tag: `postgres:16.4-bookworm`.
- Document the exact image digest in test fixtures.

## Container Resource Limits

Every simulation container must have explicit limits:

| Resource | Limit |
|---|---|
| CPU | 2 cores |
| Memory | 512 MiB |
| Storage | 1 GiB tmpfs |
| Network | Host-only (no external access) |

## Credential Redaction

- Database credentials must **never** appear in:
  - Log output (redact in `tracing` spans).
  - Error messages (replace with `<redacted>`).
  - Serialized report files.
  - Git history.
- Use environment variables or a secrets manager for credential injection.
- Connection strings in error messages must mask the password component.

## Dependency Auditing

- Run `cargo audit` in CI on every PR.
- No `yanked` crates allowed in `Cargo.lock`.
- Review all new transitive dependencies before merging.
