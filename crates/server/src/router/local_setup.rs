//! Local-mode workspace setup endpoints.
//!
//! Mounted only on the local router. Write `config.yml` (and demo files for
//! the demo endpoint) into the server's `AppState::startup_cwd`. Refuse with
//! 409 if a `config.yml` already exists at that path.

use axum::{extract::State, http::StatusCode, response::Json};
use oxy_project::{DemoCopyResult, copy_demo_files_to_with_skip, write_minimal_config_yml};
use serde::Serialize;
use tracing::{error, info};

use crate::router::AppState;

#[derive(Debug, Serialize)]
pub struct SetupEmptyResponse {
    pub path: String,
    pub config_created: bool,
}

#[derive(Debug, Serialize)]
pub struct SetupDemoResponse {
    pub path: String,
    pub files_written: Vec<String>,
    pub files_skipped: Vec<String>,
    pub files_failed: Vec<FailedFile>,
}

#[derive(Debug, Serialize)]
pub struct FailedFile {
    pub path: String,
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct SetupErrorResponse {
    pub error: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

pub async fn setup_empty(
    State(app_state): State<AppState>,
) -> Result<Json<SetupEmptyResponse>, (StatusCode, Json<SetupErrorResponse>)> {
    let dir = app_state.startup_cwd.clone();
    if dir.join("config.yml").exists() {
        return Err((
            StatusCode::CONFLICT,
            Json(SetupErrorResponse {
                error: "setup_already_completed",
                path: Some(dir.to_string_lossy().into_owned()),
                details: None,
            }),
        ));
    }
    info!("local setup: creating empty workspace at {:?}", dir);
    if let Err(e) = write_minimal_config_yml(&dir).await {
        error!(
            "local setup: failed to write config.yml at {:?}: {}",
            dir, e
        );
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(SetupErrorResponse {
                error: "workspace_not_writable",
                path: Some(dir.to_string_lossy().into_owned()),
                details: Some(e.to_string()),
            }),
        ));
    }
    Ok(Json(SetupEmptyResponse {
        path: dir.to_string_lossy().into_owned(),
        config_created: true,
    }))
}

pub async fn setup_demo(
    State(app_state): State<AppState>,
) -> Result<Json<SetupDemoResponse>, (StatusCode, Json<SetupErrorResponse>)> {
    let dir = app_state.startup_cwd.clone();
    if dir.join("config.yml").exists() {
        return Err((
            StatusCode::CONFLICT,
            Json(SetupErrorResponse {
                error: "setup_already_completed",
                path: Some(dir.to_string_lossy().into_owned()),
                details: None,
            }),
        ));
    }
    info!("local setup: copying demo workspace into {:?}", dir);
    let result: DemoCopyResult = match copy_demo_files_to_with_skip(&dir).await {
        Ok(r) => r,
        Err(e) => {
            error!("local setup: failed to prepare directory {:?}: {}", dir, e);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(SetupErrorResponse {
                    error: "workspace_not_writable",
                    path: Some(dir.to_string_lossy().into_owned()),
                    details: Some(e.to_string()),
                }),
            ));
        }
    };

    // Hard constraint: config.yml must exist after the demo copy. If the
    // embedded demo is missing config.yml or it failed to write, the
    // workspace is invalid; return 500.
    let config_landed = result.files_written.iter().any(|p| p == "config.yml")
        || result.files_skipped.iter().any(|p| p == "config.yml");
    if !config_landed {
        let demo_err = result
            .files_failed
            .iter()
            .find(|(p, _)| p == "config.yml")
            .map(|(_, e)| e.clone())
            .unwrap_or_else(|| "config.yml not produced by demo copy".to_string());
        error!("local setup: demo did not produce config.yml: {}", demo_err);
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(SetupErrorResponse {
                error: "demo_setup_failed",
                path: Some(dir.to_string_lossy().into_owned()),
                details: Some(demo_err),
            }),
        ));
    }

    Ok(Json(SetupDemoResponse {
        path: dir.to_string_lossy().into_owned(),
        files_written: result.files_written,
        files_skipped: result.files_skipped,
        files_failed: result
            .files_failed
            .into_iter()
            .map(|(path, error)| FailedFile { path, error })
            .collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use oxy_app_core::serve_mode::ServeMode;
    use tempfile::TempDir;

    fn test_state(startup_cwd: std::path::PathBuf) -> AppState {
        AppState {
            enterprise: false,
            internal: false,
            mode: ServeMode::Local,
            observability: None,
            preagg_cache: None,
            preagg_renewal_threshold_secs: None,
            startup_cwd,
            agentic_state: None,
            semantic_layer_cache: oxy_app_core::workspace_cache::new_semantic_layer_cache(),
            semantic_engine_cache: oxy_app_core::workspace_cache::new_semantic_engine_cache(),
            authz: oxy_server_authz::enforcer(),
        }
    }

    #[tokio::test]
    async fn setup_empty_writes_minimal_config() {
        let tmp = TempDir::new().expect("tempdir");
        let state = test_state(tmp.path().to_path_buf());

        let resp = setup_empty(State(state)).await.expect("should succeed");
        assert!(resp.0.config_created);
        assert_eq!(resp.0.path, tmp.path().to_string_lossy());
        assert!(tmp.path().join("config.yml").exists());
    }

    #[tokio::test]
    async fn setup_empty_returns_409_when_config_exists() {
        let tmp = TempDir::new().expect("tempdir");
        std::fs::write(tmp.path().join("config.yml"), "preexisting").expect("seed");
        let state = test_state(tmp.path().to_path_buf());

        let err = setup_empty(State(state)).await.expect_err("should 409");
        assert_eq!(err.0, StatusCode::CONFLICT);
        assert_eq!(err.1.0.error, "setup_already_completed");
        let contents = std::fs::read_to_string(tmp.path().join("config.yml")).unwrap();
        assert_eq!(
            contents, "preexisting",
            "existing file must not be clobbered"
        );
    }

    #[tokio::test]
    async fn setup_demo_writes_config_and_demo_files() {
        let tmp = TempDir::new().expect("tempdir");
        let state = test_state(tmp.path().to_path_buf());

        let resp = setup_demo(State(state)).await.expect("should succeed");
        assert!(resp.0.files_written.iter().any(|p| p == "config.yml"));
        assert!(resp.0.files_skipped.is_empty());
        assert!(tmp.path().join("config.yml").exists());
    }

    #[tokio::test]
    async fn setup_demo_skips_existing_files() {
        let tmp = TempDir::new().expect("tempdir");
        // Seed with a non-config file so we land in 200 (config.yml still written).
        let demo_entries = oxy_project::copy_demo_files_to_with_skip(&tmp.path().join("inspect"))
            .await
            .expect("probe");
        let sample = demo_entries
            .files_written
            .iter()
            .find(|p| p.as_str() != "config.yml")
            .cloned()
            .expect("demo should contain a non-config file");
        std::fs::remove_dir_all(tmp.path().join("inspect")).ok();

        let seeded = tmp.path().join(&sample);
        if let Some(parent) = seeded.parent() {
            std::fs::create_dir_all(parent).expect("mkdir -p");
        }
        std::fs::write(&seeded, "user's copy").expect("seed");

        let state = test_state(tmp.path().to_path_buf());
        let resp = setup_demo(State(state)).await.expect("should succeed");
        assert!(
            resp.0.files_skipped.contains(&sample),
            "expected {} in files_skipped, got {:?}",
            sample,
            resp.0.files_skipped
        );
        let contents = std::fs::read_to_string(&seeded).unwrap();
        assert_eq!(
            contents, "user's copy",
            "seeded file must not be overwritten"
        );
    }

    #[tokio::test]
    async fn setup_demo_returns_409_when_config_exists() {
        let tmp = TempDir::new().expect("tempdir");
        std::fs::write(tmp.path().join("config.yml"), "preexisting").expect("seed");
        let state = test_state(tmp.path().to_path_buf());

        let err = setup_demo(State(state)).await.expect_err("should 409");
        assert_eq!(err.0, StatusCode::CONFLICT);
        assert_eq!(err.1.0.error, "setup_already_completed");
    }
}
