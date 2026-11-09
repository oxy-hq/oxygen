//! A local Airhouse for `oxy start`: MinIO, the control plane and one data
//! plane, with the control plane's database and the tenant catalogs in the
//! `oxy-postgres` container `oxy start` already runs.
//!
//! The same shape as `docker-compose.airhouse.yml`, less its HAProxy (one data
//! plane needs no balancing) and its own Postgres. The control plane and the
//! wire listen on the ports that file publishes, 8080 and 5445, because that is
//! where `airhouse::config::autodetect_local_airhouse` looks: a `serve --local`,
//! `oxy seed` or `oxy compile` run beside `oxy start` finds this Airhouse with
//! no `AIRHOUSE_*` set. MinIO publishes nothing; only Airhouse reads it.

use bollard::Docker;
use bollard::models::{
    ContainerCreateBody, ContainerStateStatusEnum, HostConfig, NetworkCreateRequest,
};
use bollard::query_parameters::{
    CreateContainerOptions, InspectContainerOptions, StartContainerOptions,
};
use oxy_shared::errors::OxyError;
use std::time::{Duration, Instant};
use tracing::info;

use super::{
    NETWORK, POSTGRES_CONTAINER_NAME, POSTGRES_USERNAME, exec_in, get_docker_client, ownership,
    probe_host_tcp, pull_image,
};

mod plane;
use plane::{Plane, start_plane};

/// The release dev and prod run, as in `docker-compose.airhouse.yml`.
/// `AIRHOUSE_IMAGE_TAG` overrides it there and here.
const AIRHOUSE_IMAGE_TAG: &str = "0.1.51";
const AIRHOUSE_IMAGE_REPO: &str = "ghcr.io/oxy-hq/airhouse";
/// Our copy of Chainguard's MinIO, pinned by digest — the image
/// `docker-compose.airhouse.yml` uses; move both together.
const MINIO_IMAGE: &str = "ghcr.io/oxy-hq/minio:RELEASE.2026-09-22T19-25-18Z@sha256:4692462f35d97d7e82c30371d82f057703c5d9489bcae726010594c812f2d285";

pub const MINIO_CONTAINER_NAME: &str = "oxy-airhouse-minio";
pub const CP_CONTAINER_NAME: &str = "oxy-airhouse-cp";
pub const DP_CONTAINER_NAME: &str = "oxy-airhouse-dp";
pub(super) const MINIO_VOLUME: &str = "oxy-airhouse-minio-data";
pub(super) const CONTAINER_NAMES: [&str; 3] =
    [DP_CONTAINER_NAME, CP_CONTAINER_NAME, MINIO_CONTAINER_NAME];

/// The control plane's admin API on the host.
pub const ADMIN_PORT: u16 = 8080;
/// The data plane's Postgres wire protocol on the host.
pub const WIRE_PORT: u16 = 5445;
/// Not a secret: the token `docker-compose.airhouse.yml` uses, for an Airhouse
/// that listens only on this machine.
pub const ADMIN_TOKEN: &str = "airhouse-local-token";
/// How to reset an Airhouse that `oxy start` adopted instead of starting:
/// the one `docker-compose.airhouse.yml` runs, with its own volumes.
pub const RESET_EXTERNAL_HINT: &str = "docker compose -f docker-compose.airhouse.yml down -v";
pub const READY_TIMEOUT_SECS: u64 = 120;

const MINIO_USER: &str = "minioadmin";
const MINIO_PASSWORD: &str = "minioadmin";
const BUCKET: &str = "airhouse";
const CP_DATABASE: &str = "airhouse_cp";
/// The database the control plane connects to when it creates and drops each
/// tenant's catalog database.
const CATALOG_ADMIN_DATABASE: &str = "airhouse";

/// What [`start`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalAirhouse {
    /// Started the containers.
    Started,
    /// An Airhouse already answered on [`ADMIN_PORT`] (the compose stack, most
    /// likely), so none was started and that one is used.
    AlreadyRunning,
}

/// `http://localhost:8080` — where the admin API is reached from the host.
pub fn admin_url() -> String {
    format!("http://localhost:{ADMIN_PORT}")
}

fn airhouse_image() -> String {
    let tag = std::env::var("AIRHOUSE_IMAGE_TAG")
        .ok()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| AIRHOUSE_IMAGE_TAG.to_string());
    format!("{AIRHOUSE_IMAGE_REPO}:{tag}")
}

/// Start the local Airhouse and wait until both planes are ready. Needs
/// `oxy-postgres` running and ready.
pub async fn start() -> Result<LocalAirhouse, OxyError> {
    if admin_healthy().await {
        return Ok(LocalAirhouse::AlreadyRunning);
    }
    let docker = get_docker_client().await?;
    let image = airhouse_image();
    pull_image(&docker, &image).await?;
    pull_image(&docker, MINIO_IMAGE).await?;

    create_databases().await?;

    start_minio(&docker).await?;
    wait_until(
        "MinIO",
        MINIO_CONTAINER_NAME,
        || async { exec_ok(MINIO_CONTAINER_NAME, vec!["mc", "ready", "local"]).await },
        READY_TIMEOUT_SECS,
    )
    .await?;
    create_bucket().await?;

    start_plane(&docker, &image, Plane::Control).await?;
    wait_until(
        "the Airhouse control plane",
        CP_CONTAINER_NAME,
        admin_healthy,
        READY_TIMEOUT_SECS,
    )
    .await?;

    start_plane(&docker, &image, Plane::Data).await?;
    wait_until(
        "the Airhouse data plane",
        DP_CONTAINER_NAME,
        || async {
            exec_ok(
                DP_CONTAINER_NAME,
                vec!["curl", "-fsS", "http://localhost:8081/healthz"],
            )
            .await
                && probe_host_tcp("127.0.0.1", WIRE_PORT).await
        },
        READY_TIMEOUT_SECS,
    )
    .await?;

    info!("local Airhouse is ready");
    Ok(LocalAirhouse::Started)
}

/// The control plane answers its health check on the host.
async fn admin_healthy() -> bool {
    let Ok(client) = oxy_shared::http::builder()
        .timeout(Duration::from_secs(2))
        .build()
    else {
        return false;
    };
    client
        .get(format!("{}/healthz", admin_url()))
        .send()
        .await
        .is_ok_and(|r| r.status().is_success())
}

/// The network `oxy-postgres` and the Airhouse containers share, so the
/// planes reach Postgres and MinIO by container name.
pub(super) async fn ensure_network(docker: &Docker) -> Result<(), OxyError> {
    if docker
        .inspect_network(
            NETWORK,
            None::<bollard::query_parameters::InspectNetworkOptions>,
        )
        .await
        .is_ok()
    {
        return Ok(());
    }
    match docker
        .create_network(NetworkCreateRequest {
            name: NETWORK.to_string(),
            ..Default::default()
        })
        .await
    {
        Ok(_) => Ok(()),
        // Created by another `oxy start` between the inspect and the create.
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 409, ..
        }) => Ok(()),
        Err(e) => Err(OxyError::InitializationError(format!(
            "Failed to create the '{NETWORK}' container network: {e}"
        ))),
    }
}

/// The control plane's database and the database it administers tenant
/// catalogs from, in `oxy-postgres`. Idempotent.
async fn create_databases() -> Result<(), OxyError> {
    for db in [CP_DATABASE, CATALOG_ADMIN_DATABASE] {
        let script = format!(
            "psql -U {POSTGRES_USERNAME} -d postgres -tAc \"SELECT 1 FROM pg_database WHERE datname='{db}'\" \
             | grep -q 1 || psql -U {POSTGRES_USERNAME} -d postgres -c 'CREATE DATABASE {db}'"
        );
        let (code, output) = exec_in(POSTGRES_CONTAINER_NAME, vec!["sh", "-c", &script]).await?;
        if code != 0 {
            return Err(OxyError::InitializationError(format!(
                "Failed to create the '{db}' database for the local Airhouse: {output}"
            )));
        }
    }
    Ok(())
}

async fn start_minio(docker: &Docker) -> Result<(), OxyError> {
    let config = ContainerCreateBody {
        image: Some(MINIO_IMAGE.to_string()),
        // The image runs as a non-root user; root keeps a volume written by an
        // older image readable, as in `docker-compose.airhouse.yml`.
        user: Some("0:0".to_string()),
        cmd: Some(
            ["server", "/data", "--console-address", ":9001"]
                .map(String::from)
                .to_vec(),
        ),
        env: Some(vec![
            format!("MINIO_ROOT_USER={MINIO_USER}"),
            format!("MINIO_ROOT_PASSWORD={MINIO_PASSWORD}"),
        ]),
        host_config: Some(HostConfig {
            binds: Some(vec![format!("{MINIO_VOLUME}:/data")]),
            network_mode: Some(NETWORK.to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    create_and_start(docker, MINIO_CONTAINER_NAME, config).await
}

async fn create_bucket() -> Result<(), OxyError> {
    let script = format!(
        "mc alias set local http://localhost:9000 {MINIO_USER} {MINIO_PASSWORD} >/dev/null \
         && mc mb local/{BUCKET} --ignore-existing"
    );
    let (code, output) = exec_in(MINIO_CONTAINER_NAME, vec!["sh", "-c", &script]).await?;
    if code != 0 {
        return Err(OxyError::InitializationError(format!(
            "Failed to create the '{BUCKET}' bucket for the local Airhouse: {output}"
        )));
    }
    Ok(())
}

/// Create `name` from `config`, claim it by ID (see `ownership`), and start it.
async fn create_and_start(
    docker: &Docker,
    name: &str,
    config: ContainerCreateBody,
) -> Result<(), OxyError> {
    let options = CreateContainerOptions {
        name: Some(name.to_string()),
        ..Default::default()
    };
    let created = docker
        .create_container(Some(options), config)
        .await
        .map_err(|e| {
            OxyError::InitializationError(format!("Failed to create container '{name}': {e}"))
        })?;
    ownership::OWNED.record(created.id);
    docker
        .start_container(name, None::<StartContainerOptions>)
        .await
        .map_err(|e| {
            OxyError::InitializationError(format!(
                "Failed to start container '{name}': {e}\n\
                 If port {ADMIN_PORT} or {WIRE_PORT} is taken, stop what holds it, or run \
                 `oxy start --no-airhouse`."
            ))
        })?;
    info!("started container '{name}'");
    Ok(())
}

async fn exec_ok(container: &str, cmd: Vec<&str>) -> bool {
    matches!(exec_in(container, cmd).await, Ok((0, _)))
}

/// Poll `ready` until it holds, failing at once when `container` stops
/// running (a bad image, a port already taken) instead of at the timeout.
async fn wait_until<F, Fut>(
    what: &str,
    container: &str,
    ready: F,
    timeout_secs: u64,
) -> Result<(), OxyError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let docker = get_docker_client().await?;
    let start = Instant::now();
    loop {
        if ready().await {
            return Ok(());
        }
        let state = docker
            .inspect_container(container, None::<InspectContainerOptions>)
            .await
            .map_err(|e| {
                OxyError::InitializationError(format!("Failed to inspect '{container}': {e}"))
            })?
            .state;
        if let Some(state) = state
            && state.status != Some(ContainerStateStatusEnum::RUNNING)
        {
            return Err(OxyError::InitializationError(format!(
                "{what} is not running (status: {:?}, exit code: {:?}). \
                 See `docker logs {container}`.",
                state.status, state.exit_code
            )));
        }
        if start.elapsed() >= Duration::from_secs(timeout_secs) {
            return Err(OxyError::InitializationError(format!(
                "{what} did not become ready within {timeout_secs} seconds. \
                 See `docker logs {container}`."
            )));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_image_follows_the_compose_file() {
        let compose = include_str!("../../../../../docker-compose.airhouse.yml");
        assert!(
            compose.contains(&format!("AIRHOUSE_IMAGE_TAG:-{AIRHOUSE_IMAGE_TAG}")),
            "docker-compose.airhouse.yml and AIRHOUSE_IMAGE_TAG name different releases"
        );
        assert!(
            compose.contains(MINIO_IMAGE),
            "docker-compose.airhouse.yml and MINIO_IMAGE name different MinIO images"
        );
    }
}
