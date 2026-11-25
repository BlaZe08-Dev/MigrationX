//! Disposable PostgreSQL container runner with resource/network isolation and
//! guaranteed RAII cleanup.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use testcontainers::core::{ContainerAsync, ContainerRequest, Mount, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{GenericImage, ImageExt};
use tokio::net::UnixStream;
use tokio::task::JoinHandle;
use tokio_postgres::{Client, NoTls};
use tracing::{debug, info, warn};

use crate::error::SimulatorError;

/// Pinned PostgreSQL Docker image repository.
pub const POSTGRES_IMAGE_NAME: &str = "postgres";

/// Pinned PostgreSQL Docker image tag (mutable tags like `latest` or `16` are forbidden).
pub const POSTGRES_IMAGE_TAG: &str = "16.4-bookworm";

/// Default CPU limit: 2 cores expressed in nanocpus.
pub const DEFAULT_CPU_NANO_LIMIT: i64 = 2_000_000_000;

/// Default memory limit: 512 MiB.
pub const DEFAULT_MEMORY_BYTES_LIMIT: i64 = 512 * 1024 * 1024;

/// Default tmpfs mount limit for `PGDATA`: 1 GiB.
pub const DEFAULT_TMPFS_BYTES_LIMIT: i64 = 1024 * 1024 * 1024;

/// Container path for PostgreSQL data directory (`tmpfs` mount target).
pub const PGDATA_MOUNT_PATH: &str = "/var/lib/postgresql/data";

/// Container path for PostgreSQL Unix domain socket directory.
pub const PG_SOCKET_CONTAINER_DIR: &str = "/var/run/postgresql";

const SIM_DB_USER: &str = "postgres";
const SIM_DB_NAME: &str = "migrationx_sim";
const SIM_DB_PASSWORD: &str = "migrationx_sim";

/// Configuration for an ephemeral simulation PostgreSQL container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerConfig {
    /// CPU limit in nanocpus (default: 2 cores = `2_000_000_000`).
    pub cpu_nano_limit: i64,
    /// Memory limit in bytes (default: 512 MiB).
    pub memory_bytes_limit: i64,
    /// `tmpfs` size limit for `PGDATA` in bytes (default: 1 GiB).
    pub tmpfs_bytes_limit: i64,
    /// Disable all outbound/inbound container networking (`network_mode = "none"`).
    /// Communication uses a bind-mounted Unix domain socket in `/var/run/postgresql`.
    pub disable_outbound_network: bool,
    /// Maximum duration allowed for container startup and Postgres readiness.
    pub boot_timeout: Duration,
    /// Maximum duration allowed for individual SQL queries or batches.
    pub query_timeout: Duration,
    /// Maximum duration allowed for explicit async container removal.
    pub cleanup_timeout: Duration,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        Self {
            cpu_nano_limit: DEFAULT_CPU_NANO_LIMIT,
            memory_bytes_limit: DEFAULT_MEMORY_BYTES_LIMIT,
            tmpfs_bytes_limit: DEFAULT_TMPFS_BYTES_LIMIT,
            disable_outbound_network: true,
            boot_timeout: Duration::from_secs(60),
            query_timeout: Duration::from_secs(30),
            cleanup_timeout: Duration::from_secs(10),
        }
    }
}

/// RAII guard for the host-side temporary Unix socket directory.
///
/// Ensures the temporary directory is removed even if `PostgresRunner::start`
/// is cancelled mid-`.await`, times out, or returns an error.
#[derive(Debug)]
struct SocketDirGuard {
    path: Option<PathBuf>,
}

impl SocketDirGuard {
    fn create() -> Result<Self, SimulatorError> {
        let unique = format!(
            "migrationx-pgsock-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let dir = std::env::temp_dir().join(unique);
        fs::create_dir_all(&dir).map_err(|e| SimulatorError::Io {
            operation: "create_socket_dir",
            message: e.to_string(),
        })?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o777);
            fs::set_permissions(&dir, perms).map_err(|e| SimulatorError::Io {
                operation: "chmod_socket_dir",
                message: e.to_string(),
            })?;
        }

        Ok(Self { path: Some(dir) })
    }

    fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    fn take(&mut self) -> Option<PathBuf> {
        self.path.take()
    }
}

impl Drop for SocketDirGuard {
    fn drop(&mut self) {
        if let Some(dir) = self.path.take() {
            if let Err(err) = fs::remove_dir_all(&dir) {
                warn!(path = %dir.display(), error = %err, "failed to remove temporary socket directory");
            }
        }
    }
}

/// Builds a `testcontainers` `ContainerRequest` for `postgres:16.4-bookworm`
/// with `tmpfs` PGDATA, disabled outbound networking, and CPU/memory limits.
pub fn build_postgres_container_request(
    config: &RunnerConfig,
    host_socket_dir: &Path,
) -> Result<ContainerRequest<GenericImage>, SimulatorError> {
    let socket_dir_str = host_socket_dir
        .to_str()
        .ok_or_else(|| SimulatorError::Io {
            operation: "encode_socket_dir_path",
            message: "socket directory path is not valid UTF-8".to_string(),
        })?
        .to_string();

    let cpu_nano = config.cpu_nano_limit;
    let mem_bytes = config.memory_bytes_limit;
    let disable_net = config.disable_outbound_network;

    let mut req = GenericImage::new(POSTGRES_IMAGE_NAME, POSTGRES_IMAGE_TAG)
        .with_wait_for(WaitFor::Nothing)
        .with_env_var("POSTGRES_USER", SIM_DB_USER)
        .with_env_var("POSTGRES_PASSWORD", SIM_DB_PASSWORD)
        .with_env_var("POSTGRES_DB", SIM_DB_NAME)
        .with_env_var("PGDATA", PGDATA_MOUNT_PATH)
        .with_mount(
            Mount::tmpfs_mount(PGDATA_MOUNT_PATH)
                .with_size_bytes(config.tmpfs_bytes_limit)
                .with_mode(0o700),
        )
        .with_mount(Mount::bind_mount(socket_dir_str, PG_SOCKET_CONTAINER_DIR))
        .with_startup_timeout(config.boot_timeout)
        .with_host_config_modifier(move |host_config| {
            host_config.nano_cpus = Some(cpu_nano);
            host_config.memory = Some(mem_bytes);
            host_config.memory_swap = Some(mem_bytes);
            if disable_net {
                host_config.network_mode = Some("none".to_string());
            }
        });

    if disable_net {
        req = req.with_network("none");
    }

    Ok(req)
}

/// Ephemeral PostgreSQL simulation container with RAII lifecycle management.
///
/// Container teardown is guaranteed via [`Drop`] on normal completion, error,
/// timeout, or async task cancellation (`tokio::select!`). Callers may also
/// invoke [`PostgresRunner::shutdown`] for explicit async teardown with error
/// reporting.
pub struct PostgresRunner {
    container: Option<ContainerAsync<GenericImage>>,
    client: Option<Client>,
    connection_task: Option<JoinHandle<()>>,
    socket_dir: Option<PathBuf>,
    boot_latency: Duration,
    config: RunnerConfig,
}

impl PostgresRunner {
    /// Starts an isolated `postgres:16.4-bookworm` container, connects over the
    /// bind-mounted Unix domain socket, and records `boot_latency`.
    ///
    /// # Cancellation Safety
    ///
    /// If the returned future is cancelled or times out at any `.await` point,
    /// the internal [`SocketDirGuard`] and `testcontainers::ContainerAsync`
    /// `Drop` implementations clean up the temporary socket directory and
    /// container automatically.
    pub async fn start(config: RunnerConfig) -> Result<Self, SimulatorError> {
        let boot_start = Instant::now();
        let mut socket_guard = SocketDirGuard::create()?;
        let socket_dir_path = socket_guard
            .path()
            .ok_or_else(|| SimulatorError::Io {
                operation: "create_socket_dir",
                message: "socket directory guard missing path".to_string(),
            })?
            .to_path_buf();

        let request = build_postgres_container_request(&config, &socket_dir_path)?;

        let container = tokio::time::timeout(config.boot_timeout, request.start())
            .await
            .map_err(|_| SimulatorError::ContainerBootTimeout {
                timeout_secs: config.boot_timeout.as_secs(),
            })?
            .map_err(|err| SimulatorError::ContainerStartFailed {
                message: err.to_string(),
            })?;

        let socket_file = socket_dir_path.join(".s.PGSQL.5432");
        let (client, connection_task) =
            wait_and_connect_unix_socket(&socket_file, config.boot_timeout).await?;

        let boot_latency = boot_start.elapsed();
        info!(
            container_id = %container.id(),
            boot_latency_ms = boot_latency.as_millis() as u64,
            "ephemeral PostgreSQL simulation container ready"
        );

        Ok(Self {
            container: Some(container),
            client: Some(client),
            connection_task: Some(connection_task),
            socket_dir: socket_guard.take(),
            boot_latency,
            config,
        })
    }

    /// Returns the measured container boot and readiness latency.
    pub fn boot_latency(&self) -> Duration {
        self.boot_latency
    }

    /// Returns the runner configuration.
    pub fn config(&self) -> &RunnerConfig {
        &self.config
    }

    /// Returns a reference to the active `tokio-postgres` client.
    pub fn client(&self) -> Result<&Client, SimulatorError> {
        self.client
            .as_ref()
            .ok_or_else(|| SimulatorError::ConnectionFailed {
                message: "PostgreSQL client has already been shut down".to_string(),
            })
    }

    /// Opens a new independent `tokio-postgres` connection to the running
    /// simulation container over its bind-mounted Unix domain socket.
    ///
    /// Callers are responsible for aborting or awaiting the returned
    /// `JoinHandle` when finished.
    pub async fn connect(&self) -> Result<(Client, JoinHandle<()>), SimulatorError> {
        let socket_dir =
            self.socket_dir
                .as_ref()
                .ok_or_else(|| SimulatorError::ConnectionFailed {
                    message: "simulation container socket directory is not available".to_string(),
                })?;
        let socket_file = socket_dir.join(".s.PGSQL.5432");
        connect_unix_socket(&socket_file, self.config.boot_timeout).await
    }

    /// Returns the Docker container ID if the container is still active.
    pub fn container_id(&self) -> Option<&str> {
        self.container.as_ref().map(|c| c.id())
    }

    /// Explicitly shuts down the client connection and removes the container
    /// within [`RunnerConfig::cleanup_timeout`].
    pub async fn shutdown(mut self) -> Result<(), SimulatorError> {
        self.client.take();
        if let Some(handle) = self.connection_task.take() {
            handle.abort();
        }

        let mut remove_result = Ok(());
        if let Some(container) = self.container.take() {
            let container_id = container.id().to_string();
            debug!(container_id = %container_id, "explicitly removing simulation container");
            match tokio::time::timeout(self.config.cleanup_timeout, container.rm()).await {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    remove_result = Err(SimulatorError::ContainerCleanupFailed {
                        container_id,
                        message: err.to_string(),
                    });
                }
                Err(_) => {
                    remove_result = Err(SimulatorError::OperationTimeout {
                        operation: "container_rm",
                        timeout_secs: self.config.cleanup_timeout.as_secs(),
                    });
                }
            }
        }

        if let Some(dir) = self.socket_dir.take() {
            if let Err(err) = fs::remove_dir_all(&dir) {
                warn!(path = %dir.display(), error = %err, "failed to remove socket directory on shutdown");
            }
        }

        remove_result
    }
}

impl Drop for PostgresRunner {
    fn drop(&mut self) {
        self.client.take();
        if let Some(handle) = self.connection_task.take() {
            handle.abort();
        }
        // Dropping `ContainerAsync` triggers testcontainers' built-in RAII
        // container removal if `shutdown()` was not already called.
        if let Some(container) = self.container.take() {
            debug!(
                container_id = %container.id(),
                "PostgresRunner dropped without explicit shutdown; triggering RAII container cleanup"
            );
            drop(container);
        }
        if let Some(dir) = self.socket_dir.take() {
            if let Err(err) = fs::remove_dir_all(&dir) {
                warn!(path = %dir.display(), error = %err, "failed to remove socket directory on drop");
            }
        }
    }
}

/// Polls the PostgreSQL Unix domain socket until it accepts a connection or
/// `timeout` elapses.
///
/// Handles the brief window after `docker-entrypoint.sh` `initdb` where the
/// socket file may not yet be created or listening.
async fn wait_and_connect_unix_socket(
    socket_path: &Path,
    timeout: Duration,
) -> Result<(Client, JoinHandle<()>), SimulatorError> {
    let poll_fut = async {
        loop {
            match connect_unix_socket(socket_path, timeout).await {
                Ok((client, handle)) => match client.simple_query("SELECT 1").await {
                    Ok(_) => return Ok((client, handle)),
                    Err(_) => {
                        handle.abort();
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                },
                Err(SimulatorError::ConnectionFailed { .. }) => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(other) => return Err(other),
            }
        }
    };

    tokio::time::timeout(timeout, poll_fut).await.map_err(|_| {
        SimulatorError::ContainerBootTimeout {
            timeout_secs: timeout.as_secs(),
        }
    })?
}

/// Connects to PostgreSQL over a Unix domain socket with an explicit timeout.
///
/// # Cancellation Safety
///
/// If cancelled or timed out, no container state is mutated and any partial
/// socket stream is dropped immediately.
async fn connect_unix_socket(
    socket_path: &Path,
    timeout: Duration,
) -> Result<(Client, JoinHandle<()>), SimulatorError> {
    let connect_fut = async {
        let stream = UnixStream::connect(socket_path).await.map_err(|e| {
            SimulatorError::ConnectionFailed {
                message: format!("unix socket connect failed: {e}"),
            }
        })?;

        let mut pg_config = tokio_postgres::Config::new();
        pg_config
            .user(SIM_DB_USER)
            .password(SIM_DB_PASSWORD)
            .dbname(SIM_DB_NAME);

        let (client, connection) = pg_config.connect_raw(stream, NoTls).await.map_err(|e| {
            SimulatorError::ConnectionFailed {
                message: format!("postgres handshake failed: {e}"),
            }
        })?;

        let handle = tokio::spawn(async move {
            if let Err(err) = connection.await {
                warn!(error = %err, "simulation postgres connection terminated");
            }
        });

        Ok::<_, SimulatorError>((client, handle))
    };

    tokio::time::timeout(timeout, connect_fut)
        .await
        .map_err(|_| SimulatorError::OperationTimeout {
            operation: "connect_unix_socket",
            timeout_secs: timeout.as_secs(),
        })?
}

#[cfg(test)]
mod tests {
    use super::*;
    use testcontainers::Image;

    #[test]
    fn test_runner_default_config_matches_security_limits() {
        let cfg = RunnerConfig::default();
        assert_eq!(cfg.cpu_nano_limit, 2_000_000_000);
        assert_eq!(cfg.memory_bytes_limit, 512 * 1024 * 1024);
        assert_eq!(cfg.tmpfs_bytes_limit, 1024 * 1024 * 1024);
        assert!(cfg.disable_outbound_network);
        assert_eq!(cfg.boot_timeout, Duration::from_secs(60));
        assert_eq!(POSTGRES_IMAGE_NAME, "postgres");
        assert_eq!(POSTGRES_IMAGE_TAG, "16.4-bookworm");
    }

    #[test]
    fn test_runner_build_request_sets_pinned_image_and_isolation() {
        let cfg = RunnerConfig::default();
        let socket_dir = PathBuf::from("/tmp/migrationx-test-sock");
        let req = build_postgres_container_request(&cfg, &socket_dir).expect("build request");

        assert_eq!(req.image().name(), "postgres");
        assert_eq!(req.image().tag(), "16.4-bookworm");
        assert_eq!(req.network().as_deref(), Some("none"));
        assert!(req.host_config_modifier().is_some());

        let mut host_config = testcontainers::bollard::models::HostConfig::default();
        if let Some(modifier) = req.host_config_modifier() {
            modifier(&mut host_config);
        }
        assert_eq!(host_config.nano_cpus, Some(2_000_000_000));
        assert_eq!(host_config.memory, Some(512 * 1024 * 1024));
        assert_eq!(host_config.memory_swap, Some(512 * 1024 * 1024));
        assert_eq!(host_config.network_mode.as_deref(), Some("none"));

        let mounts: Vec<_> = req.mounts().collect();
        assert_eq!(mounts.len(), 2);
        assert!(mounts.iter().any(|m| m.target() == Some(PGDATA_MOUNT_PATH)));
        assert!(mounts
            .iter()
            .any(|m| m.target() == Some(PG_SOCKET_CONTAINER_DIR)));
    }

    #[test]
    fn test_runner_socket_dir_guard_cleans_up_on_drop() {
        let created_path = {
            let guard = SocketDirGuard::create().expect("create guard");
            let path = guard.path().expect("guard path").to_path_buf();
            assert!(path.exists());
            path
        };
        assert!(!created_path.exists());
    }

    #[tokio::test]
    async fn test_runner_connect_unix_socket_missing_path_returns_connection_failed() {
        let missing = PathBuf::from("/tmp/migrationx-nonexistent-pg-sock-test.sock");
        let err = connect_unix_socket(&missing, Duration::from_secs(2))
            .await
            .expect_err("missing socket must fail");
        assert!(matches!(err, SimulatorError::ConnectionFailed { .. }));
    }

    #[tokio::test]
    #[ignore = "requires docker"]
    async fn test_runner_docker_lifecycle_boot_and_raii_cleanup() {
        let cfg = RunnerConfig::default();
        let runner = PostgresRunner::start(cfg).await.expect("start container");
        assert!(runner.boot_latency() > Duration::ZERO);
        let client = runner.client().expect("client");
        let row = client
            .query_one("SELECT 1::int4", &[])
            .await
            .expect("SELECT 1");
        let val: i32 = row.get(0);
        assert_eq!(val, 1);
        runner.shutdown().await.expect("shutdown");
    }
}
