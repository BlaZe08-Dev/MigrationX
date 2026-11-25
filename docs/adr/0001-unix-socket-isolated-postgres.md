# ADR: Unix-Socket-Isolated Ephemeral PostgreSQL Containers

- **Status:** Accepted
- **Date:** 2026-11-25
- **Authors:** MigrationX Core Engineering

---

## 1. Context

`crates/simulator` (`migrationx-simulator`) must run untrusted or potentially hazardous migration SQL against a real PostgreSQL `16.4-bookworm` instance to observe `pg_locks` contention, execution failures, and post-migration `PREPARE` compatibility. Doing so safely and deterministically in local development and CI requires:

1. Complete network isolation so migration scripts cannot make outbound network calls or collide on host TCP ports during parallel test runs.
2. Fast container startup and `initdb`/DDL execution without leaving persistent disk state behind.
3. Bounded CPU and memory usage so runaway migrations cannot starve the host.
4. Guaranteed cleanup even when a simulation times out, errors, or is cancelled mid-`.await`.

---

## 2. Decisions

### 2.1 `network_mode = "none"` + Bind-Mounted Unix Domain Socket (vs. Bridge Port Mapping)
We disable all container networking by default (`RunnerConfig::disable_outbound_network = true`, setting `req.with_network("none")` and `host_config.network_mode = Some("none")` in `build_postgres_container_request`) and communicate exclusively over a bind-mounted Unix domain socket:

- On the host, `SocketDirGuard::create()` creates a unique temporary directory under `std::env::temp_dir()` (with `0o777` permissions on Unix) and bind-mounts it to `/var/run/postgresql` (`PG_SOCKET_CONTAINER_DIR`) inside the container.
- `PostgresRunner::start` and `PostgresRunner::connect` connect to `.s.PGSQL.5432` via `tokio::net::UnixStream` and `tokio_postgres::Config::connect_raw(..., NoTls)`.

**Rationale:**
- Eliminates host port allocation races and ephemeral port exhaustion when multiple simulations run concurrently.
- Prevents any SQL feature (e.g., extensions or external-connection primitives) from reaching external networks or host services.
- Avoids Docker bridge NAT overhead.

### 2.2 `tmpfs` Mount for `PGDATA`
`PGDATA` (`/var/lib/postgresql/data`) is mounted as an in-memory `tmpfs` volume (`DEFAULT_TMPFS_BYTES_LIMIT = 1 GiB`, mode `0o700`).

**Rationale:**
- Keeps `initdb`, schema replay, and migration execution in RAM, avoiding host disk I/O jitter.
- Guarantees zero leftover database files on disk when the container is removed.

### 2.3 Strict CPU and Memory Resource Caps
`RunnerConfig::default()` enforces:
- **CPU limit:** `2_000_000_000` nanocpus (2 cores).
- **Memory limit:** `512 MiB` (`512 * 1024 * 1024` bytes), with `memory_swap` set equal to `memory` to disable swap.
- **Pinned image:** `postgres:16.4-bookworm` (mutable tags like `latest` or `16` are forbidden).

### 2.4 RAII Teardown and Explicit Shutdown
`PostgresRunner` implements both explicit async `shutdown(self)` (with `cleanup_timeout = 10s`) and `Drop` (aborting the connection task, dropping `testcontainers::ContainerAsync`, and removing the temporary socket directory). `SocketDirGuard` also removes the host socket directory if `PostgresRunner::start` is cancelled or times out before completion.

---

## 3. Consequences

- **Positive:** Zero host port conflicts, zero outbound/inbound network exposure, fast `tmpfs`-backed container lifecycles, bounded resource consumption, and deterministic cleanup on panics, timeouts, or `tokio::select!` cancellation.
- **Negative / Trade-offs:** Requires a Unix-like host supporting bind-mounted Unix domain sockets (standard on Linux CI and developer environments) and a running Docker daemon for live simulation tests.
