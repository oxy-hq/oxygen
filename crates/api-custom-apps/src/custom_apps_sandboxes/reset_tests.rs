use std::collections::BTreeSet;

use uuid::Uuid;

use super::*;
use crate::custom_apps_agent_fixture as fixture;

fn keys(keys: &[&str]) -> Vec<String> {
    keys.iter().map(|key| key.to_string()).collect()
}

#[test]
fn an_empty_list_is_refused() {
    let error = requested_planes(&[]).expect_err("nothing to reset");
    assert_eq!(error.code(), "invalid_planes");
}

#[test]
fn an_unknown_key_is_refused_by_name() {
    let error = requested_planes(&keys(&["storage", "storege"])).expect_err("a typo");
    assert_eq!(error.code(), "invalid_planes");
    assert!(error.to_string().contains("storege"), "{error}");
    // Plane keys are the wire spelling: kebab-case, not the Rust name.
    requested_planes(&keys(&["WorkspaceActions"])).expect_err("not a key");
    requested_planes(&keys(&["workspace-actions"])).expect("a key");
}

/// The planes come back in the order given, each once.
#[test]
fn planes_keep_their_order_and_are_deduplicated() {
    let planes = requested_planes(&keys(&["storage", "oltp", "storage"])).expect("valid");
    assert_eq!(planes, [Plane::Storage, Plane::Oltp]);
}

fn target<'a>(
    db: &'a DatabaseConnection,
    app: &'a apps::Model,
    environment: &'a AppEnvironment,
    actor: &'a RequestActor,
) -> ResetTarget<'a> {
    ResetTarget {
        db,
        app,
        environment,
        actor,
        at: None,
    }
}

/// OLTP, storage, secrets, messages and airhouse have a reset (`RESETTABLE`,
/// the shipped list); every other plane still answers
/// `409 plane_not_resettable`, naming its key. A phase that ships a reset adds
/// its plane to `RESETTABLE`. (OLTP's and Airhouse's arms read the sandbox's
/// row, so their refusals are the integration tests'.)
#[tokio::test]
async fn the_other_planes_cannot_be_reset_yet() {
    let db = DatabaseConnection::default();
    let app = fixture::app(Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3));
    let actor = fixture::actor(None);
    let sandbox = AppEnvironment::parse("dev-a1").expect("a sandbox");
    let mut refused = BTreeSet::new();
    for plane in Plane::ALL {
        if RESETTABLE.contains(plane) {
            continue;
        }
        let error = reset_plane(&target(&db, &app, &sandbox, &actor), *plane)
            .await
            .expect_err(plane.key());
        assert_eq!(error.status().as_u16(), 409, "{}", plane.key());
        assert_eq!(error.code(), "plane_not_resettable");
        assert!(error.to_string().contains(plane.key()), "{error}");
        refused.insert(plane.key());
    }
    assert_eq!(refused.len(), Plane::ALL.len() - RESETTABLE.len());
}

/// Staging's files and secrets are the team's: neither is reset from here,
/// before anything is read or deleted. Its outbox is not the team's, and is
/// reset like a sandbox's (the integration test holds that it works).
#[tokio::test]
async fn staging_storage_and_secrets_are_not_reset_from_here() {
    let db = DatabaseConnection::default();
    let app = fixture::app(Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3));
    let actor = fixture::actor(None);
    for plane in [Plane::Storage, Plane::Secrets] {
        let error = reset_plane(&target(&db, &app, &AppEnvironment::Staging, &actor), plane)
            .await
            .expect_err(plane.key());
        assert_eq!(error.status().as_u16(), 409, "{}", plane.key());
        assert_eq!(error.code(), "plane_not_resettable");
        let words = error.to_string();
        assert!(words.contains(plane.key()), "{words}");
        assert!(words.contains("shared by the team"), "{words}");
    }
}

/// Staging has no branch of its own to reset, and says so before it reads a
/// row.
#[tokio::test]
async fn staging_oltp_is_not_resettable() {
    let db = DatabaseConnection::default();
    let app = fixture::app(Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3));
    let actor = fixture::actor(None);
    let error = reset_plane(
        &target(&db, &app, &AppEnvironment::Staging, &actor),
        Plane::Oltp,
    )
    .await
    .expect_err("staging");
    assert_eq!(error.code(), "plane_not_resettable");
    assert_eq!(
        error.to_string(),
        crate::custom_apps_sandboxes::reset_oltp::STAGING_REFUSAL
    );
}

/// A sandbox on `schema` is not resettable, and the refusal says what to do
/// instead; staging's says why it has none. (The route reads the sandbox's row
/// for the first; its text is held here, its use by the integration test.)
#[test]
fn the_oltp_refusals_say_what_to_do_instead() {
    use crate::custom_apps_sandboxes::reset_oltp::{SCHEMA_MODE_REFUSAL, STAGING_REFUSAL};
    for says in ["publish to the sandbox again", "--plane oltp=branch"] {
        assert!(SCHEMA_MODE_REFUSAL.contains(says), "{SCHEMA_MODE_REFUSAL}");
    }
    for says in [
        "oxyc oltp reset --org <org> --branch staging",
        "/api/admin/orgs/{org_id}/oltp/branches/staging/reset",
    ] {
        assert!(STAGING_REFUSAL.contains(says), "{STAGING_REFUSAL}");
    }
    assert!(!STAGING_REFUSAL.contains("console"), "{STAGING_REFUSAL}");
    let error = SandboxError::PlaneNotResettableHere(SCHEMA_MODE_REFUSAL.to_string());
    assert_eq!(error.status().as_u16(), 409);
    assert_eq!(error.code(), "plane_not_resettable");
    assert_eq!(error.to_string(), SCHEMA_MODE_REFUSAL);
}

#[test]
fn the_answer_is_name_and_planes_with_a_status_and_a_run_id() {
    let answer = ResetAnswer {
        name: "dev-a1".into(),
        planes: vec![
            PlaneReset {
                plane: "storage",
                status: ResetStatus::Done,
                run_id: None,
            },
            PlaneReset {
                plane: "oltp",
                status: ResetStatus::Queued,
                run_id: Some("r-1".into()),
            },
        ],
    };
    assert_eq!(
        serde_json::to_value(&answer).expect("json"),
        serde_json::json!({
            "name": "dev-a1",
            "planes": [
                {"plane": "storage", "status": "done", "run_id": null},
                {"plane": "oltp", "status": "queued", "run_id": "r-1"},
            ],
        })
    );
}

/// `at` forks the OLTP branch and nothing else: a reset that names it without
/// `oltp` is refused, not obeyed with `at` dropped.
#[test]
fn at_without_the_oltp_plane_is_refused() {
    let at = Some(chrono::Utc::now());
    let error = refuse_at_without_oltp(at, &[Plane::Storage, Plane::Airhouse])
        .expect_err("at without oltp");
    assert_eq!(error.code(), "invalid_fork_at");
    assert_eq!(error.status().as_u16(), 400);
    assert!(error.to_string().contains("oltp"), "{error}");
    refuse_at_without_oltp(at, &[Plane::Storage, Plane::Oltp]).expect("with oltp");
    refuse_at_without_oltp(None, &[Plane::Storage]).expect("no at");
}
