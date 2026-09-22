#![allow(deprecated)] // bollard container/image options are deprecated but still functional

use bollard::Docker;
use bollard::exec::{CreateExecOptions, StartExecResults};
use bollard::models::{
    ContainerCreateBody, ContainerStateStatusEnum, ContainerSummaryStateEnum, HostConfig,
    PortBinding,
};
use bollard::query_parameters::{
    CreateContainerOptions, CreateImageOptions, InspectContainerOptions, ListContainersOptions,
    RemoveContainerOptions, RemoveVolumeOptions, StartContainerOptions, StopContainerOptions,
};
use futures::StreamExt;
use oxy_shared::errors::OxyError;
use std::collections::HashMap;
use std::default::Default;
use tracing::{info, warn};

mod ownership;
pub use ownership::{ShutdownCleanup, cleanup_owned_containers};

/// Docker PostgreSQL configuration constants
const POSTGRES_CONTAINER_NAME: &str = "oxy-postgres";
const POSTGRES_IMAGE: &str = "postgres:18-alpine";
const POSTGRES_PORT: u16 = 15432;
const POSTGRES_USERNAME: &str = "postgres";
const POSTGRES_PASSWORD: &str = "postgres";
const POSTGRES_DATABASE: &str = "oxy";
const POSTGRES_VOLUME: &str = "oxy-postgres-data";
pub const POSTGRES_READY_TIMEOUT_SECS: u64 = 30;

/// Docker ClickHouse configuration constants (used when observability backend is ClickHouse)
pub const CLICKHOUSE_CONTAINER_NAME: &str = "oxy-clickhouse";
const CLICKHOUSE_IMAGE: &str = "clickhouse/clickhouse-server:25.12.5.44";
pub const CLICKHOUSE_HTTP_PORT: u16 = 8123;
pub const CLICKHOUSE_NATIVE_PORT: u16 = 9000;
pub const CLICKHOUSE_USER: &str = "default";
pub const CLICKHOUSE_PASSWORD: &str = "default";
pub const CLICKHOUSE_DATABASE: &str = "observability";
const CLICKHOUSE_VOLUME: &str = "oxy-clickhouse-data";
pub const CLICKHOUSE_READY_TIMEOUT_SECS: u64 = 30;

/// Get Docker client connection
///
/// This function attempts to connect to a Docker-compatible container runtime.
/// It works with Docker Desktop, Rancher Desktop (moby mode), Colima, Podman (with Docker socket),
/// and other Docker API-compatible runtimes.
///
/// Connection is determined by the DOCKER_HOST environment variable (if set) or system defaults.
/// Supported DOCKER_HOST formats:
/// - `unix:///path/to/docker.sock` - Unix socket (Linux/macOS)
/// - `npipe:////./pipe/docker_engine` - Named pipe (Windows)
/// - `tcp://host:port` - TCP connection
async fn get_docker_client() -> Result<Docker, OxyError> {
    // Try default connection first (respects DOCKER_HOST env var)
    if let Ok(docker) = Docker::connect_with_local_defaults() {
        return Ok(docker);
    }

    // Fallback: try common alternative socket paths (especially for macOS)
    // These paths are used when Docker Desktop or other runtimes are installed without admin
    #[cfg(unix)]
    {
        let home_dir = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let alternative_paths = vec![
            // Docker Desktop (user installation on macOS)
            format!("{}/.docker/run/docker.sock", home_dir),
            format!("{}/.colima/default/docker.sock", home_dir),
            // Rancher Desktop
            format!("{}/.rd/docker.sock", home_dir),
            // Podman (macOS machine)
            format!(
                "{}/.local/share/containers/podman/machine/podman.sock",
                home_dir
            ),
            // OrbStack
            format!("{}/.orbstack/run/docker.sock", home_dir),
            // Standard system paths
            "/var/run/docker.sock".to_string(),
            "/run/docker.sock".to_string(),
        ];

        for socket_path in alternative_paths {
            // Check if socket file exists before attempting connection
            if std::path::Path::new(&socket_path).exists()
                && let Ok(docker) =
                    Docker::connect_with_unix(&socket_path, 120, bollard::API_DEFAULT_VERSION)
            {
                tracing::debug!("Connected to Docker using socket: {}", socket_path);
                return Ok(docker);
            }
        }
    }

    Err(OxyError::InitializationError(
        "Failed to connect to container runtime.\n\n\
         💡 Troubleshooting:\n\
         • Ensure a Docker-compatible container runtime is installed and running\n\
         • Supported: Docker Desktop, Rancher Desktop, Colima, Podman, OrbStack, etc.\n\
         • Verify with: docker ps\n\
         • Set DOCKER_HOST environment variable for custom socket location\n\
         • On macOS, common socket paths:\n\
           - Docker Desktop (admin): /var/run/docker.sock\n\
           - Docker Desktop (user): ~/.docker/run/docker.sock\n\
           - Colima: ~/.colima/default/docker.sock\n\
           - Rancher Desktop: ~/.rd/docker.sock\n\
           - OrbStack: ~/.orbstack/run/docker.sock"
            .to_string(),
    ))
}

/// Check if Docker is available on the system
pub async fn check_docker_available() -> Result<(), OxyError> {
    let docker = get_docker_client().await?;

    // Ping Docker daemon to verify it's responsive
    docker.ping().await.map_err(|e| {
        OxyError::InitializationError(format!(
            "Container runtime is not responding.\n\
             Error: {}\n\n\
             💡 Common solutions:\n\
             • Start your container runtime (Docker Desktop, Rancher Desktop, Colima, etc.)\n\
             • Wait 30-60 seconds for the runtime to fully initialize\n\
             • Check runtime status: docker ps\n\
             • For Rancher Desktop: ensure 'dockerd (moby)' is selected, not 'containerd'\n\
             • For Colima: verify with: colima status\n\
             • For Podman: ensure Docker socket is enabled",
            e
        ))
    })?;

    Ok(())
}

/// Pull a Docker image if not already present
async fn pull_image(docker: &Docker, image: &str) -> Result<(), OxyError> {
    let create_image_options = CreateImageOptions {
        from_image: Some(image.to_string()),
        ..Default::default()
    };

    let mut pull_stream = docker.create_image(Some(create_image_options), None, None);
    while let Some(result) = pull_stream.next().await {
        match result {
            Ok(info) => {
                if let Some(status) = info.status
                    && (status.contains("Downloaded") || status.contains("Pulling"))
                {
                    tracing::debug!("{}", status);
                }
            }
            Err(e) => {
                warn!("Image pull warning: {}", e);
            }
        }
    }
    Ok(())
}

/// Check if the PostgreSQL container is running
pub async fn is_postgres_container_running() -> Result<bool, OxyError> {
    let docker = get_docker_client().await?;

    let mut filters = HashMap::new();
    filters.insert(
        "name".to_string(),
        vec![POSTGRES_CONTAINER_NAME.to_string()],
    );

    let options = ListContainersOptions {
        filters: Some(filters),
        ..Default::default()
    };

    let containers = docker
        .list_containers(Some(options))
        .await
        .map_err(|e| OxyError::InitializationError(format!("Failed to list containers: {}", e)))?;

    // Check if any container is running
    Ok(containers.iter().any(|c| {
        c.state
            .as_ref()
            .is_some_and(|s| *s == ContainerSummaryStateEnum::RUNNING)
    }))
}

/// Start the PostgreSQL container and return the connection string.
/// Containers are cleaned before startup, so this always creates fresh.
pub async fn start_postgres_container() -> Result<String, OxyError> {
    info!("Creating PostgreSQL container...");
    let docker = get_docker_client().await?;

    // Pull the image (if not already present)
    pull_image(&docker, POSTGRES_IMAGE).await?;

    let mut port_bindings = HashMap::new();
    port_bindings.insert(
        "5432/tcp".to_string(),
        Some(vec![PortBinding {
            host_ip: Some("0.0.0.0".to_string()),
            host_port: Some(POSTGRES_PORT.to_string()),
        }]),
    );

    // Configure volume bindings
    // PostgreSQL 18+ uses /var/lib/postgresql as the volume mount point
    // with PGDATA at /var/lib/postgresql/18/docker (version-specific subdirectory)
    // See: https://hub.docker.com/_/postgres ("PGDATA" section for 18+)
    let binds = vec![format!("{}:/var/lib/postgresql", POSTGRES_VOLUME)];

    let host_config = HostConfig {
        port_bindings: Some(port_bindings),
        binds: Some(binds),
        ..Default::default()
    };

    // Configure environment variables
    // PostgreSQL 18+ uses /var/lib/postgresql/18/docker as the default PGDATA
    // This is version-specific and enables faster pg_upgrade with --link
    // See: https://hub.docker.com/_/postgres ("PGDATA" section for 18+)
    let env: Vec<String> = vec![
        format!("POSTGRES_USER={}", POSTGRES_USERNAME),
        format!("POSTGRES_PASSWORD={}", POSTGRES_PASSWORD),
        format!("POSTGRES_DB={}", POSTGRES_DATABASE),
        "PGDATA=/var/lib/postgresql/18/docker".to_string(),
    ];

    let config = ContainerCreateBody {
        image: Some(POSTGRES_IMAGE.to_string()),
        env: Some(env),
        host_config: Some(host_config),
        ..Default::default()
    };

    let options = CreateContainerOptions {
        name: Some(POSTGRES_CONTAINER_NAME.to_string()),
        ..Default::default()
    };

    // Create and start the container. It is claimed by the ID the runtime
    // returned, never by its fixed name: a later `oxy start` can replace the
    // container behind the name, and this process must not remove that one.
    let created = docker
        .create_container(Some(options), config)
        .await
        .map_err(|e| OxyError::InitializationError(format!("Failed to create container: {}", e)))?;
    ownership::OWNED.record(created.id);

    docker
        .start_container(POSTGRES_CONTAINER_NAME, None::<StartContainerOptions>)
        .await
        .map_err(|e| OxyError::InitializationError(format!("Failed to start container: {}", e)))?;

    info!("PostgreSQL container started successfully");

    let connection_string = format!(
        "postgresql://{}:{}@localhost:{}/{}",
        POSTGRES_USERNAME, POSTGRES_PASSWORD, POSTGRES_PORT, POSTGRES_DATABASE
    );

    Ok(connection_string)
}

/// Wait for PostgreSQL to be ready to accept connections
pub async fn wait_for_postgres_ready(timeout_secs: u64) -> Result<(), OxyError> {
    info!(
        "Waiting for PostgreSQL to be ready (max {} seconds)...",
        timeout_secs
    );

    let docker = get_docker_client().await?;
    let start = std::time::Instant::now();
    let mut retry_count = 0;

    loop {
        // Check if timeout exceeded
        if start.elapsed().as_secs() >= timeout_secs {
            return Err(OxyError::InitializationError(format!(
                "PostgreSQL did not become ready within {} seconds",
                timeout_secs
            )));
        }

        // Check container status first
        let inspect_result = docker
            .inspect_container(POSTGRES_CONTAINER_NAME, None::<InspectContainerOptions>)
            .await;

        match inspect_result {
            Ok(container) => {
                if let Some(state) = container.state
                    && state.status != Some(ContainerStateStatusEnum::RUNNING)
                {
                    return Err(OxyError::InitializationError(format!(
                        "PostgreSQL container is not running (status: {:?})",
                        state.status
                    )));
                }

                // Try pg_isready command
                let exec_config = CreateExecOptions {
                    cmd: Some(vec![
                        "pg_isready",
                        "-U",
                        POSTGRES_USERNAME,
                        "-d",
                        POSTGRES_DATABASE,
                    ]),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    ..Default::default()
                };

                let exec_result = docker
                    .create_exec(POSTGRES_CONTAINER_NAME, exec_config)
                    .await;

                if let Ok(exec_response) = exec_result
                    && let Ok(StartExecResults::Attached { mut output, .. }) =
                        docker.start_exec(&exec_response.id, None).await
                {
                    // Read output to check exit code
                    let mut stdout = Vec::new();
                    while let Some(Ok(msg)) = output.next().await {
                        stdout.extend_from_slice(&msg.into_bytes());
                    }

                    // If we got here, pg_isready executed successfully
                    if String::from_utf8_lossy(&stdout).contains("accepting connections") {
                        // pg_isready checks the unix socket inside the container. The host
                        // still needs Docker's port publisher to forward 15432 -> 5432.
                        // Probe the host-side TCP port to close that race before returning.
                        if probe_host_tcp("127.0.0.1", POSTGRES_PORT).await {
                            info!("PostgreSQL is ready!");
                            return Ok(());
                        }
                    }
                }
            }
            Err(e) => {
                return Err(OxyError::InitializationError(format!(
                    "Failed to inspect container: {}",
                    e
                )));
            }
        }

        retry_count += 1;
        if retry_count == 1 || retry_count % 5 == 0 {
            info!(
                "PostgreSQL not ready yet, retrying... ({} seconds elapsed)",
                start.elapsed().as_secs()
            );
        }

        // Wait before retrying (exponential backoff with cap at 2 seconds)
        let wait_ms = std::cmp::min(100 * 2u64.pow(retry_count.min(4)), 2000);
        tokio::time::sleep(tokio::time::Duration::from_millis(wait_ms)).await;
    }
}

async fn probe_host_tcp(host: &str, port: u16) -> bool {
    let addr = format!("{}:{}", host, port);
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        tokio::net::TcpStream::connect(&addr),
    )
    .await;
    match result {
        Ok(Ok(_)) => true,
        Ok(Err(e)) => {
            tracing::debug!("TCP probe to {} failed: {}", addr, e);
            false
        }
        Err(_) => {
            tracing::debug!("TCP probe to {} timed out after 1s", addr);
            false
        }
    }
}

/// Stop the PostgreSQL container (handles non-existent/stopped containers gracefully)
pub async fn stop_postgres_container() -> Result<(), OxyError> {
    let docker = get_docker_client().await?;
    let options = StopContainerOptions {
        t: Some(5),
        signal: None,
    };

    match docker
        .stop_container(POSTGRES_CONTAINER_NAME, Some(options))
        .await
    {
        Ok(_) => {
            info!("PostgreSQL container stopped");
            Ok(())
        }
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        })
        | Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 304, ..
        }) => {
            // 404 = not found, 304 = already stopped
            Ok(())
        }
        Err(e) => {
            warn!("Failed to stop PostgreSQL container: {}", e);
            Ok(()) // Don't fail on stop errors
        }
    }
}

/// Remove the PostgreSQL container (useful for cleanup or troubleshooting)
#[allow(dead_code)]
pub async fn remove_postgres_container() -> Result<(), OxyError> {
    let docker = get_docker_client().await?;
    let options = RemoveContainerOptions {
        force: true,
        ..Default::default()
    };

    docker
        .remove_container(POSTGRES_CONTAINER_NAME, Some(options))
        .await
        .map_err(|e| OxyError::InitializationError(format!("Failed to remove container: {}", e)))?;

    info!("PostgreSQL container removed successfully");
    Ok(())
}

/// Remove every oxy-managed container BY NAME, whoever created it.
///
/// For `oxy start`'s startup only: it clears whatever an earlier run left
/// behind before creating its own. Never call this on a shutdown path — the
/// names are fixed, so it removes another server's containers too. Shutdown
/// uses [`cleanup_owned_containers`], which removes only what this process
/// created.
/// Does NOT remove volumes or networks - use clean_all() for that.
/// Errors are logged but not propagated (cleanup should not block startup).
pub async fn cleanup_containers() {
    let docker = match get_docker_client().await {
        Ok(d) => d,
        Err(e) => {
            warn!("Could not connect to Docker during cleanup: {}", e);
            return;
        }
    };

    remove_container(&docker, POSTGRES_CONTAINER_NAME).await;
    remove_container(&docker, CLICKHOUSE_CONTAINER_NAME).await;
}

/// Remove a container by name or by ID — the runtime accepts either (force remove,
/// handles non-existent containers gracefully). Startup cleanup passes the fixed
/// name; shutdown passes the ID this process was given, so a stale claim is a 404.
async fn remove_container(docker: &Docker, name: &str) {
    let remove_opts = RemoveContainerOptions {
        force: true,
        ..Default::default()
    };

    match docker.remove_container(name, Some(remove_opts)).await {
        Ok(_) => info!("Removed container '{}'", name),
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        }) => {} // Container doesn't exist, that's fine
        Err(e) => warn!("Failed to remove container '{}': {}", name, e),
    }
}

async fn remove_volume(docker: &Docker, name: &str) -> Result<(), OxyError> {
    let opts = RemoveVolumeOptions { force: true };
    match docker.remove_volume(name, Some(opts)).await {
        Ok(_) => {
            info!("Removed volume '{}'", name);
            Ok(())
        }
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        }) => {
            // Volume doesn't exist, that's fine
            Ok(())
        }
        Err(e) => Err(OxyError::InitializationError(format!(
            "Failed to remove volume '{}': {}",
            name, e
        ))),
    }
}

/// Clean all Oxy-managed Docker containers and volumes.
/// Used by `oxy start --clean` to start from a fresh state.
pub async fn clean_all() -> Result<(), OxyError> {
    let docker = get_docker_client().await?;

    remove_container(&docker, POSTGRES_CONTAINER_NAME).await;
    remove_volume(&docker, POSTGRES_VOLUME).await?;

    // Remove clickhouse container + volume (no-op if they don't exist)
    remove_container(&docker, CLICKHOUSE_CONTAINER_NAME).await;
    remove_volume(&docker, CLICKHOUSE_VOLUME).await?;

    Ok(())
}

// ── ClickHouse ────────────────────────────────────────────────────────────

/// Start a ClickHouse container for the observability backend.
///
/// Idempotent-ish: the caller should run `cleanup_containers()` or pass
/// `--clean` first. Listens on HTTP 8123 and native 9000. Credentials are
/// the defaults expected by `ClickHouseObservabilityStorage::from_env()`.
pub async fn start_clickhouse_container() -> Result<(), OxyError> {
    info!("Creating ClickHouse container...");
    let docker = get_docker_client().await?;

    pull_image(&docker, CLICKHOUSE_IMAGE).await?;

    let mut port_bindings = HashMap::new();
    port_bindings.insert(
        "8123/tcp".to_string(),
        Some(vec![PortBinding {
            host_ip: Some("0.0.0.0".to_string()),
            host_port: Some(CLICKHOUSE_HTTP_PORT.to_string()),
        }]),
    );
    port_bindings.insert(
        "9000/tcp".to_string(),
        Some(vec![PortBinding {
            host_ip: Some("0.0.0.0".to_string()),
            host_port: Some(CLICKHOUSE_NATIVE_PORT.to_string()),
        }]),
    );

    let binds = vec![format!("{}:/var/lib/clickhouse", CLICKHOUSE_VOLUME)];

    // ClickHouse requires a high file-descriptor ceiling — the server itself
    // recommends 262144. The default container limit is often lower and will
    // emit warnings at startup.
    let ulimits = vec![bollard::models::ResourcesUlimits {
        name: Some("nofile".to_string()),
        soft: Some(262144),
        hard: Some(262144),
    }];

    // ClickHouse can allocate aggressively and OOM on laptops. Cap at 2GB
    // which is plenty for observability volumes; set memory_swap equal to
    // memory to disable swap and fail fast instead of thrashing.
    const MEM_LIMIT: i64 = 2 * 1024 * 1024 * 1024;

    let host_config = HostConfig {
        port_bindings: Some(port_bindings),
        binds: Some(binds),
        ulimits: Some(ulimits),
        memory: Some(MEM_LIMIT),
        memory_swap: Some(MEM_LIMIT),
        ..Default::default()
    };

    let env: Vec<String> = vec![
        format!("CLICKHOUSE_USER={}", CLICKHOUSE_USER),
        format!("CLICKHOUSE_PASSWORD={}", CLICKHOUSE_PASSWORD),
        format!("CLICKHOUSE_DB={}", CLICKHOUSE_DATABASE),
        // Enable SQL access management so CLICKHOUSE_PASSWORD takes effect for
        // the default user (without this the password is ignored).
        "CLICKHOUSE_DEFAULT_ACCESS_MANAGEMENT=1".to_string(),
    ];

    let config = ContainerCreateBody {
        image: Some(CLICKHOUSE_IMAGE.to_string()),
        env: Some(env),
        host_config: Some(host_config),
        ..Default::default()
    };

    let options = CreateContainerOptions {
        name: Some(CLICKHOUSE_CONTAINER_NAME.to_string()),
        ..Default::default()
    };

    // Claimed by ID, not by name — see `start_postgres_container`.
    let created = docker
        .create_container(Some(options), config)
        .await
        .map_err(|e| {
            OxyError::InitializationError(format!("Failed to create ClickHouse container: {}", e))
        })?;
    ownership::OWNED.record(created.id);

    docker
        .start_container(CLICKHOUSE_CONTAINER_NAME, None::<StartContainerOptions>)
        .await
        .map_err(|e| {
            OxyError::InitializationError(format!("Failed to start ClickHouse container: {}", e))
        })?;

    info!("ClickHouse container started successfully");
    Ok(())
}

/// Wait for ClickHouse to accept HTTP queries (polling `/ping`).
///
/// Fails fast if the container exits or enters a non-running state — avoids
/// the 30-second timeout when ClickHouse fails to start (OOM, config error,
/// port conflict, etc).
pub async fn wait_for_clickhouse_ready(timeout_secs: u64) -> Result<(), OxyError> {
    use std::time::{Duration, Instant};

    info!(
        "Waiting for ClickHouse to be ready (max {} seconds)...",
        timeout_secs
    );

    let docker = get_docker_client().await?;
    let start = Instant::now();
    let timeout = Duration::from_secs(timeout_secs);
    let url = format!("http://localhost:{}/ping", CLICKHOUSE_HTTP_PORT);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .map_err(|e| OxyError::InitializationError(format!("http client build failed: {}", e)))?;

    loop {
        if start.elapsed() >= timeout {
            return Err(OxyError::InitializationError(format!(
                "ClickHouse not ready after {} seconds",
                timeout_secs
            )));
        }

        // Check container state first — fail fast if it exited.
        match docker
            .inspect_container(CLICKHOUSE_CONTAINER_NAME, None::<InspectContainerOptions>)
            .await
        {
            Ok(container) => {
                if let Some(state) = container.state
                    && state.status != Some(ContainerStateStatusEnum::RUNNING)
                {
                    return Err(OxyError::InitializationError(format!(
                        "ClickHouse container is not running (status: {:?}, exit_code: {:?}, error: {:?})",
                        state.status, state.exit_code, state.error
                    )));
                }
            }
            Err(e) => {
                return Err(OxyError::InitializationError(format!(
                    "Failed to inspect ClickHouse container: {}",
                    e
                )));
            }
        }

        // Ping the HTTP endpoint.
        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                info!("ClickHouse is ready");
                return Ok(());
            }
            _ => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_string_format() {
        let expected = format!(
            "postgresql://{}:{}@localhost:{}/{}",
            POSTGRES_USERNAME, POSTGRES_PASSWORD, POSTGRES_PORT, POSTGRES_DATABASE
        );
        assert_eq!(
            expected,
            "postgresql://postgres:postgres@localhost:15432/oxy"
        );
    }

    #[test]
    fn test_constants() {
        assert_eq!(POSTGRES_CONTAINER_NAME, "oxy-postgres");
        assert_eq!(POSTGRES_IMAGE, "postgres:18-alpine");
        assert_eq!(POSTGRES_PORT, 15432);
        assert_eq!(POSTGRES_USERNAME, "postgres");
        assert_eq!(POSTGRES_DATABASE, "oxy");
    }
}
