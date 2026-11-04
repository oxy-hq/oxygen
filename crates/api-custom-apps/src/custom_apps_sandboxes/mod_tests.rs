use super::*;

/// The contract's table (`internal-docs/custom-app-sandboxes.md` §5.1):
/// one status and one code per refusal, and no two refusals share a code.
#[test]
fn every_refusal_has_its_status_and_code() {
    let table = [
        (
            SandboxError::InvalidName("x".into()),
            400,
            "invalid_environment_name",
        ),
        (
            SandboxError::NotASandbox("staging".into()),
            400,
            "not_a_sandbox",
        ),
        (SandboxError::PublishToken, 403, "publish_token_refused"),
        (SandboxError::NotStaff, 403, "non_production_refused"),
        (SandboxError::AppNotFound, 404, "app_not_found"),
        (
            SandboxError::NotFound("dev-a".into()),
            404,
            "environment_not_found",
        ),
        (
            SandboxError::Exists("dev-a".into()),
            409,
            "environment_exists",
        ),
        (
            SandboxError::Deleting("dev-a".into()),
            409,
            "environment_deleting",
        ),
        (SandboxError::Limit(20), 409, "environment_limit"),
        (SandboxError::TokenLimit(3), 409, "token_sandbox_limit"),
        (
            SandboxError::Reserved {
                name: "dev-a".into(),
                app: "x--dev-a".into(),
            },
            409,
            "environment_reserved",
        ),
        (
            SandboxError::InvalidParent("dev-x".into()),
            400,
            "invalid_parent",
        ),
        (
            SandboxError::InvalidForkAt("later".into()),
            400,
            "invalid_fork_at",
        ),
        (
            SandboxError::InvalidPlanes("olpt".into()),
            400,
            "invalid_planes",
        ),
        (
            SandboxError::ProductionHasNoPlanes,
            409,
            "production_has_no_planes",
        ),
        (
            SandboxError::EnvironmentHasNoRow("staging".into()),
            409,
            "environment_has_no_row",
        ),
        (
            SandboxError::PlaneNotResettable("storage".into()),
            409,
            "plane_not_resettable",
        ),
        (SandboxError::Busy("dev-a".into()), 409, "environment_busy"),
        (SandboxError::Internal("boom".into()), 500, "internal"),
    ];
    let mut codes = std::collections::BTreeSet::new();
    for (error, status, code) in table {
        assert_eq!(error.status().as_u16(), status, "{error:?}");
        assert_eq!(error.code(), code, "{error:?}");
        assert!(codes.insert(code), "{code} names two refusals");
    }
    assert_eq!(codes.len(), 19);
}

/// A plane that resets on another kind of environment answers the same code
/// as one with no reset at all, in its own words.
#[test]
fn a_plane_that_resets_elsewhere_is_still_plane_not_resettable() {
    let error = SandboxError::PlaneNotResettableHere("staging's files are shared".into());
    assert_eq!(
        (error.status().as_u16(), error.code()),
        (409, "plane_not_resettable")
    );
    assert_eq!(error.to_string(), "staging's files are shared");
}

#[tokio::test]
async fn a_refusal_answers_error_and_message() {
    let response = SandboxError::Limit(20).into_response();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("body");
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(body["error"], "environment_limit");
    assert!(body["message"].as_str().unwrap().contains("20 sandboxes"));
    assert_eq!(body.as_object().unwrap().len(), 2, "error and message only");
}

/// An internal failure's detail is logged, never answered.
#[test]
fn an_internal_error_does_not_leak_its_detail() {
    let error = SandboxError::Internal("relation \"apps\" does not exist".into());
    assert_eq!(error.to_string(), "internal error");
}

/// One week by default; the override is whole days, never under one.
/// nextest runs each test in its own process, so the env writes are its own.
#[test]
fn idle_ttl_is_a_week_unless_overridden_and_never_under_a_day() {
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::remove_var(IDLE_DAYS_ENV) };
    assert_eq!(idle_ttl(), chrono::Duration::days(7));
    for (raw, days) in [("3", 3), (" 14 ", 14), ("0", 1), ("-5", 1), ("soon", 7)] {
        unsafe { std::env::set_var(IDLE_DAYS_ENV, raw) };
        assert_eq!(idle_ttl(), chrono::Duration::days(days), "{raw:?}");
    }
}

#[test]
fn the_environment_object_always_carries_every_field() {
    let now = Utc::now();
    let dto = EnvironmentDto {
        name: "dev-a1".into(),
        kind: "dev".into(),
        status: "active".into(),
        build_id: None,
        build_uuid: None,
        semantic_revision_id: None,
        owner: None,
        created_at: now,
        updated_at: now,
        last_activity_at: None,
        expires_at: None,
        url: None,
        oltp_schema: None,
        oltp_branch: None,
        copies_expire_at: None,
        parent: None,
        fork_at: None,
        planes: None,
        planes_error: None,
        planes_set: None,
    };
    let json = serde_json::to_value(&dto).expect("json");
    let object = json.as_object().expect("an object");
    for field in [
        "name",
        "kind",
        "status",
        "build_id",
        "build_uuid",
        "semantic_revision_id",
        "owner",
        "created_at",
        "updated_at",
        "last_activity_at",
        "expires_at",
        "url",
        "oltp_schema",
        "oltp_branch",
        "copies_expire_at",
        "parent",
        "fork_at",
        "planes",
        "planes_error",
        "planes_set",
    ] {
        assert!(object.contains_key(field), "{field} is missing");
    }
    assert_eq!(object.len(), 20);
    assert!(json["build_id"].is_null(), "null, not omitted");
}
