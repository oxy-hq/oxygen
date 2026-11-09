use super::*;
use axum::body::to_bytes;

const WS: &str = "/api/7a4e3c1e-0b5f-4c36-9d55-3f1c2b8a9e01";

fn ide(method: &str, relative: &str) -> bool {
    refuses(method, &format!("{WS}{relative}"), RouteRole::IdeOnly)
}

#[test]
fn file_git_compile_and_builder_routes_are_refused() {
    for (method, relative) in [
        ("GET", "/files"),
        ("GET", "/files/Zm9vLnltbA"),
        ("POST", "/files/Zm9vLnltbA"),
        ("PUT", "/files/Zm9vLnltbA/rename-file"),
        ("POST", "/push-changes"),
        ("GET", "/git-state"),
        ("GET", "/revision-info"),
        ("POST", "/compile"),
        ("POST", "/databases/sync"),
        ("POST", "/databases/build"),
        ("POST", "/app-integrations"),
        ("POST", "/analytics/builder/runs"),
        ("POST", "/analytics/builder/runs/abc/answer"),
        ("POST", "/analytics/runs/abc/revert-file-changes"),
        ("POST", "/experiments/price-test/write-back/apply"),
    ] {
        assert!(ide(method, relative), "{method} {relative} must be refused");
    }
}

#[test]
fn reads_data_flow_chat_and_provisioning_stay() {
    for (method, relative) in [
        ("GET", "/compile/status"),
        ("POST", "/semantic/anomalies/scan"),
        ("POST", "/databases/test-connection"),
        ("GET", "/integrations/looker"),
        ("POST", "/agentic-airway/chunked-backfill"),
        ("POST", "/agentic-airway/sources/discover"),
        ("PATCH", "/analytics/runs/abc/thinking_mode"),
        ("POST", "/agentic-automations/whatever"),
        ("POST", "/onboarding/github-setup"),
        ("GET", "/onboarding-readiness"),
    ] {
        assert!(!ide(method, relative), "{method} {relative} must stay");
    }
}

#[test]
fn fleet_routes_and_routes_outside_the_workspace_tree_are_never_refused() {
    assert!(!refuses(
        "POST",
        &format!("{WS}/push-changes"),
        RouteRole::FleetOk
    ));
    assert!(!refuses(
        "DELETE",
        "/api/orgs/7a4e3c1e-0b5f-4c36-9d55-3f1c2b8a9e01/workspaces/x",
        RouteRole::IdeOnly
    ));
    assert!(!refuses("POST", "/api/documents/ask", RouteRole::IdeOnly));
}

#[test]
fn the_external_mirror_and_a_trailing_slash_resolve_the_same() {
    assert!(refuses(
        "GET",
        "/external/api/7a4e3c1e-0b5f-4c36-9d55-3f1c2b8a9e01/files/",
        RouteRole::IdeOnly
    ));
    assert!(!refuses(
        "GET",
        "/external/api/7a4e3c1e-0b5f-4c36-9d55-3f1c2b8a9e01/compile/status/",
        RouteRole::IdeOnly
    ));
}

fn workspace(remote: Option<&str>, branch: Option<&str>) -> workspaces::Model {
    let now = chrono::Utc::now().fixed_offset();
    workspaces::Model {
        id: uuid::Uuid::nil(),
        name: "acme".into(),
        git_namespace_id: None,
        git_remote_url: remote.map(str::to_string),
        created_at: now,
        updated_at: now,
        path: None,
        last_opened_at: None,
        created_by: None,
        org_id: None,
        status: workspaces::WorkspaceStatus::Ready,
        error: None,
        monthly_vlm_budget_micros: None,
        current_revision_id: None,
        default_branch: branch.map(str::to_string),
        repo_subdir: None,
    }
}

async fn body_of(response: Response) -> serde_json::Value {
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn the_refusal_names_the_repository_and_how_to_clone_it() {
    let ws = workspace(Some("https://github.com/acme/repo.git"), Some("main"));
    let response = gone(Some(&ws));
    assert_eq!(response.status(), StatusCode::GONE);
    let body = body_of(response).await;
    assert_eq!(body["error"], REFUSAL_CODE);
    assert_eq!(body["repo_url"], "https://github.com/acme/repo.git");
    assert_eq!(body["default_branch"], "main");
    assert_eq!(
        body["hint"],
        "git clone --branch 'main' -- 'https://github.com/acme/repo.git'"
    );
}

#[tokio::test]
async fn a_workspace_with_no_remote_is_told_to_connect_one() {
    let body = body_of(gone(Some(&workspace(None, None)))).await;
    assert_eq!(body["error"], REFUSAL_CODE);
    assert!(body["repo_url"].is_null());
    assert!(body["default_branch"].is_null());
    assert!(body["hint"].as_str().unwrap().contains("Settings"));
}

#[test]
fn the_clone_hint_quotes_what_the_workspace_row_holds() {
    let repo = Repository {
        repo_url: Some("https://github.com/acme/repo.git".into()),
        default_branch: Some("x; rm -rf ~".into()),
        repo_subdir: None,
    };
    assert_eq!(
        repo.clone_command().unwrap(),
        "git clone --branch 'x; rm -rf ~' -- 'https://github.com/acme/repo.git'"
    );
    assert_eq!(shell_quote("it's"), "'it'\\''s'");
}

#[tokio::test]
async fn the_refusal_never_echoes_credentials_in_the_remote() {
    let ws = workspace(
        Some("https://x-access-token:ghs_abc@github.com/acme/repo.git"),
        None,
    );
    let body = body_of(gone(Some(&ws))).await;
    assert_eq!(body["repo_url"], "https://github.com/acme/repo.git");
    assert_eq!(
        body["hint"],
        "git clone -- 'https://github.com/acme/repo.git'"
    );
}
