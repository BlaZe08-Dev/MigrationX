# MigrationX — Security Architecture & Threat Model

This document defines the security boundaries, threat model, and mandatory controls enforced across MigrationX (see [.agent/rules/04-security.md](../../.agent/rules/04-security.md)).

---

## 1. Threat Model

MigrationX inspects production database metadata and executes untrusted or unverified migration SQL inside local/CI environments. The primary threats are:

1. **Production Data Exfiltration:** Accidentally querying, logging, or persisting customer/production row data during snapshot collection.
2. **Production Schema/Data Mutation:** Executing write statements or DDL against the target production database.
3. **Credential Leakage:** Exposing database passwords or connection strings in `tracing` logs, `thiserror`/`anyhow` error messages, serialized `Snapshot`/`RuntimeVerdict` JSON files, or Git history.
4. **Container Escape / Resource Exhaustion / Supply-Chain Drift:** Untrusted migration SQL exhausting host CPU/RAM/disk, making outbound network calls from the simulation container, or running against unpinned/mutable PostgreSQL images.
5. **Orphaned Containers:** Simulation containers leaking after panics, timeouts, or `SIGINT`/`SIGTERM` cancellation.

---

## 2. Enforced Controls

### 2.1 Zero Production Row Exports
- **Strict Catalog-Only Queries:** Snapshot collection queries **only** `information_schema`, `pg_catalog`, `pg_class`, and `pg_stat_*` views.
- **Schema & Metadata Only:** `contracts::Snapshot` and `contracts::TableStats` store only `pg_version`, `database_name`, `qualified_name`, `estimated_row_count`, `total_size_bytes`, `has_triggers`, and `has_foreign_key_refs`. No row values, sample rows, or `pg_statistic` histogram values containing PII are exported.

### 2.2 Read-Only Target Database Sessions
- Every connection to a target database **must** execute:
  ```sql
  SET default_transaction_read_only = on;
  ```
- Target database credentials must use a least-privilege read-only role (e.g., `pg_read_all_stats` or equivalent).

### 2.3 Credential Redaction
- Credentials are injected via environment variables or a secrets manager—never hardcoded or committed.
- All `tracing` spans, error types (`thiserror` / `anyhow`), and serialized reports mask password components in connection strings with `<redacted>`.
- Automated security tests assert that raw credentials never appear in stdout, stderr, logs, or output JSON artifacts.

### 2.4 Disposable Containers & Pinned PostgreSQL Images
- Simulation containers are **strictly ephemeral**: created per simulation run and destroyed immediately upon completion. Persistent volumes and cross-run container reuse are prohibited.
- Mutable tags (`postgres:latest`, `postgres:16`) are forbidden. Test fixtures and simulation runners must pin full version tags (e.g., `postgres:16.4-bookworm`) and document image digests.

### 2.5 Container Resource & Network Isolation
Every simulation container must enforce explicit resource and network caps:

| Control | Enforced Limit | Rationale |
|---|---|---|
| **Network** | Host-only / disabled outbound (`network_mode = none` or isolated host-only) | Prevents SSRF or data exfiltration from malicious/buggy SQL extensions |
| **CPU** | `2` cores | Prevents host CPU starvation during `CONCURRENTLY` / 50M-row lock simulations |
| **Memory** | `512 MiB` | Prevents OOM-killing the host or CI runner |
| **Storage** | `1 GiB` `tmpfs` | Bounds disk usage and enables `<12s` ephemeral execution without host disk persistence |

### 2.6 Guaranteed RAII Container Teardown
- Container handles in `crates/simulator` wrap `testcontainers` lifecycle management in an RAII guard (`Drop` + explicit async cleanup with timeout).
- Even if a simulation panics, hits a `tokio::time::timeout`, or is cancelled via `tokio::select!`, the container is terminated and removed so no containers or `tmpfs` mounts leak.

### 2.7 Dependency & Supply-Chain Hygiene
- Zero `unsafe` blocks in workspace crates.
- `cargo audit` runs in CI on every PR; no `yanked` crates are permitted in `Cargo.lock`.
