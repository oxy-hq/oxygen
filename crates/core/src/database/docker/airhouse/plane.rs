//! The Airhouse control and data plane containers: one image, two roles.

use bollard::Docker;
use bollard::models::{ContainerCreateBody, HealthConfig, HostConfig, PortBinding};
use oxy_shared::errors::OxyError;
use std::collections::HashMap;

use super::super::{NETWORK, POSTGRES_CONTAINER_NAME, POSTGRES_PASSWORD, POSTGRES_USERNAME};
use super::{
    ADMIN_PORT, ADMIN_TOKEN, BUCKET, CATALOG_ADMIN_DATABASE, CP_CONTAINER_NAME, CP_DATABASE,
    DP_CONTAINER_NAME, MINIO_CONTAINER_NAME, MINIO_PASSWORD, MINIO_USER, WIRE_PORT,
    create_and_start,
};

#[derive(Clone, Copy)]
pub(super) enum Plane {
    Control,
    Data,
}

impl Plane {
    fn container_name(self) -> &'static str {
        match self {
            Plane::Control => CP_CONTAINER_NAME,
            Plane::Data => DP_CONTAINER_NAME,
        }
    }

    fn role(self) -> &'static str {
        match self {
            Plane::Control => "--role=control",
            Plane::Data => "--role=data",
        }
    }

    /// The container port this plane serves on, and where it is on the host.
    fn ports(self) -> (&'static str, u16) {
        match self {
            Plane::Control => ("8080/tcp", ADMIN_PORT),
            Plane::Data => ("5446/tcp", WIRE_PORT),
        }
    }

    /// What this plane adds to [`shared_env`].
    fn env(self) -> Vec<String> {
        match self {
            Plane::Control => vec![
                "AIRHOUSE_HTTP_PORT=8080".to_string(),
                "RUST_LOG=airhouse_server=info,airhouse_controlplane=info".to_string(),
                format!(
                    "AIRHOUSE_CP_CATALOG_ADMIN_URL={}",
                    postgres_url(CATALOG_ADMIN_DATABASE)
                ),
                format!("AIRHOUSE_CP_ADMIN_TOKEN={ADMIN_TOKEN}"),
            ],
            // The limits `docker-compose.airhouse.yml` gives its data plane.
            Plane::Data => vec![
                "AIRHOUSE_PORT=5446".to_string(),
                "AIRHOUSE_HTTP_PORT=8081".to_string(),
                "RUST_LOG=airhouse_server=info".to_string(),
                "AIRHOUSE_MAX_CONCURRENT_QUERIES=32".to_string(),
                "AIRHOUSE_PER_USER_MAX_CONCURRENT=8".to_string(),
                "AIRHOUSE_QUERY_TIMEOUT_SECS=120".to_string(),
                "AIRHOUSE_MEMORY_PER_SESSION_GB=2".to_string(),
            ],
        }
    }

    /// The image's own health check probes 8080, the control plane's port, so a
    /// healthy data plane would show as unhealthy in `docker ps`.
    fn healthcheck(self) -> Option<HealthConfig> {
        matches!(self, Plane::Data).then(|| HealthConfig {
            test: Some(
                ["CMD", "curl", "-fsS", "http://localhost:8081/healthz"]
                    .map(String::from)
                    .to_vec(),
            ),
            interval: Some(10_000_000_000),
            timeout: Some(3_000_000_000),
            retries: Some(3),
            ..Default::default()
        })
    }
}

fn postgres_url(db: &str) -> String {
    format!(
        "postgres://{POSTGRES_USERNAME}:{POSTGRES_PASSWORD}@{POSTGRES_CONTAINER_NAME}:5432/{db}?sslmode=disable"
    )
}

/// What both planes need: the control plane's database and the bucket.
fn shared_env() -> Vec<String> {
    vec![
        "AIRHOUSE_HOST=0.0.0.0".to_string(),
        "AIRHOUSE_LOG_FORMAT=json".to_string(),
        format!("AIRHOUSE_CP_DATABASE_URL={}", postgres_url(CP_DATABASE)),
        format!("S3_ENDPOINT=http://{MINIO_CONTAINER_NAME}:9000"),
        format!("S3_BUCKET={BUCKET}"),
        format!("S3_ACCESS_KEY={MINIO_USER}"),
        format!("S3_SECRET_KEY={MINIO_PASSWORD}"),
        "S3_REGION=us-east-1".to_string(),
    ]
}

pub(super) async fn start_plane(
    docker: &Docker,
    image: &str,
    plane: Plane,
) -> Result<(), OxyError> {
    let mut env = shared_env();
    env.extend(plane.env());
    let (container_port, host_port) = plane.ports();
    let port_bindings = HashMap::from([(
        container_port.to_string(),
        Some(vec![PortBinding {
            host_ip: Some("127.0.0.1".to_string()),
            host_port: Some(host_port.to_string()),
        }]),
    )]);
    let config = ContainerCreateBody {
        image: Some(image.to_string()),
        cmd: Some(vec!["server".to_string(), plane.role().to_string()]),
        env: Some(env),
        healthcheck: plane.healthcheck(),
        host_config: Some(HostConfig {
            port_bindings: Some(port_bindings),
            network_mode: Some(NETWORK.to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    create_and_start(docker, plane.container_name(), config).await
}
