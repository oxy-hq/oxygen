//! The guards of a sandbox agent token's draft that need no database: the
//! request's shape (1, 5), the app's state (3, 4), and how each refusal is
//! told. Guard 2, the row left unwritten and the pointer move are exercised
//! through the route (`tests/custom_apps/sandbox_agent_token/staging_draft`).

use std::collections::HashSet;

use super::*;
use crate::server::api::custom_apps_agent_fixture as fixture;
use crate::server::api::custom_apps_publish::PublishTarget;
use crate::server::api::custom_apps_publish_refusal::PublishRefusal;
use crate::server::api::custom_apps_sandboxes::agent_publish::AgentPublish;
use crate::server::api::custom_apps_sandboxes::publish::PublishCredential::{
    Other, PublishToken, SandboxAgent, StagingAgent,
};
use crate::server::api::custom_apps_sandboxes::publish::{PublishCredential, target_of};
use crate::server::api::custom_apps_sandboxes::retention;
use axum::response::IntoResponse;
use oxy_server_authz::Caller;

const APP: Uuid = Uuid::from_u128(1);
const ORG: Uuid = Uuid::from_u128(2);
const WORKSPACE: Uuid = Uuid::from_u128(3);

fn input() -> PublishInput {
    PublishInput {
        org_ref: None,
        app_slug: "ops".into(),
        project_id: WORKSPACE,
        branch: None,
        build_id: "b1".into(),
        name: None,
        promote: false,
        tarball: Vec::new(),
        manifest: None,
        source_repo: None,
        commit_sha: None,
        published_by: Some(Uuid::from_u128(0x5B00)),
        published_by_email: Some("minter@oxy.tech".into()),
        publisher: None,
        machine_app_id: None,
        published_via: None,
        semantic_revision_id: None,
    }
}

/// The app, live: a production build and the moment it was published.
fn live_app() -> apps::Model {
    apps::Model {
        published_build_id: Some(Uuid::from_u128(0xB1)),
        published_at: Some(chrono::Utc::now().fixed_offset()),
        ..fixture::app(APP, ORG, WORKSPACE)
    }
}

fn draft_refusal(result: Result<(), PublishError>) -> AgentDraftRefusal {
    match result {
        Err(PublishError::AgentDraft(refused)) => refused,
        other => panic!("expected a draft refusal, got {other:?}"),
    }
}

/// Guard 1: a draft that promotes is the token moving production's pointer.
#[test]
fn a_draft_that_promotes_is_refused_as_the_token_always_was() {
    let promoting = PublishInput {
        promote: true,
        ..input()
    };
    let refused = refuse_shape(&promoting).expect_err("refused");
    assert!(matches!(refused, PublishError::SandboxTokenRefused));
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(refused.code(), Some("sandbox_token_refused"));
    // Promote is answered before a field is: the wider refusal first.
    let both = PublishInput {
        promote: true,
        name: Some("Renamed".into()),
        ..input()
    };
    assert!(matches!(
        refuse_shape(&both),
        Err(PublishError::SandboxTokenRefused)
    ));
    assert!(refuse_shape(&input()).is_ok());
}

/// Guard 5: each field that rewrites the app, or pins what staging reads.
#[test]
fn each_field_that_changes_the_app_is_refused_by_name() {
    let named = PublishInput {
        name: Some("Renamed".into()),
        ..input()
    };
    let branched = PublishInput {
        branch: Some("feature".into()),
        ..input()
    };
    let pinned = PublishInput {
        semantic_revision_id: Some(Uuid::from_u128(9)),
        ..input()
    };
    for (sent, field) in [
        (named, "name"),
        (branched, "branch"),
        (pinned, "semantic_revision_id"),
    ] {
        let refused = draft_refusal(refuse_shape(&sent));
        assert_eq!(refused, AgentDraftRefusal::FieldRefused { field });
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
        assert_eq!(refused.code(), "publish_field_refused");
        assert!(refused.to_string().contains(&format!("'{field}'")));
    }
    // The fields a draft may carry: where the build came from.
    let sourced = PublishInput {
        source_repo: Some("oxy-hq/customer-apps".into()),
        commit_sha: Some("abc123".into()),
        ..input()
    };
    assert!(refuse_shape(&sourced).is_ok());
}

/// Guard 3: live is both columns. An app never promoted has neither; one
/// unpublished keeps neither; a row with only one is not live either.
#[test]
fn an_app_is_live_only_with_a_production_build_and_its_moment() {
    assert!(is_live(&live_app()));
    let never_promoted = fixture::app(APP, ORG, WORKSPACE);
    let no_moment = apps::Model {
        published_at: None,
        ..live_app()
    };
    let no_build = apps::Model {
        published_build_id: None,
        ..live_app()
    };
    for not_live in [never_promoted, no_moment, no_build] {
        assert!(!is_live(&not_live));
        let refused = refuse_app_state(&not_live, &input()).expect_err("refused");
        assert_eq!(
            refused,
            AgentDraftRefusal::AppNotLive {
                app_slug: "ops".into()
            }
        );
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(refused.code(), "app_not_live");
    }
    assert_eq!(refuse_app_state(&live_app(), &input()), Ok(()));
}

/// Guard 4: any other workspace is refused — there is no re-home for the
/// token, whatever state the app's own workspace is in.
#[test]
fn a_draft_names_the_apps_own_workspace() {
    let elsewhere = Uuid::from_u128(4);
    let moving = PublishInput {
        project_id: elsewhere,
        ..input()
    };
    let refused = refuse_app_state(&live_app(), &moving).expect_err("refused");
    assert_eq!(
        refused,
        AgentDraftRefusal::ProjectMismatch {
            app_slug: "ops".into(),
            existing_project: WORKSPACE,
            requested_project: elsewhere,
        }
    );
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    assert_eq!(refused.code(), "project_mismatch");
    // Not live is said first: it is the state of the app, whatever was sent.
    let refused = refuse_app_state(&fixture::app(APP, ORG, WORKSPACE), &moving);
    assert!(matches!(refused, Err(AgentDraftRefusal::AppNotLive { .. })));
}

/// Guard 2's answer is the one the token's other routes give an environment
/// that is not its own.
#[test]
fn not_found_is_the_tokens_environment_not_found() {
    let refused = AgentDraftRefusal::NotFound;
    assert_eq!(refused.status(), StatusCode::NOT_FOUND);
    assert_eq!(refused.code(), "environment_not_found");
    let as_publish: PublishError = refused.into();
    assert_eq!(as_publish.status(), StatusCode::NOT_FOUND);
    assert_eq!(as_publish.code(), Some("environment_not_found"));
}

async fn body_of(refused: PublishError) -> (StatusCode, serde_json::Value) {
    let response = PublishRefusal::told(refused, true).into_response();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("read the body");
    (status, serde_json::from_slice(&bytes).expect("a JSON body"))
}

/// Every refusal of this path leaves the route as the token's one body:
/// `{ code, error, message }`, the status the guard names.
#[tokio::test]
async fn each_refusal_is_the_tokens_one_json_body() {
    let refusals = [
        (AgentDraftRefusal::NotFound, 404, "environment_not_found"),
        (
            AgentDraftRefusal::AppNotLive {
                app_slug: "ops".into(),
            },
            409,
            "app_not_live",
        ),
        (
            AgentDraftRefusal::ProjectMismatch {
                app_slug: "ops".into(),
                existing_project: WORKSPACE,
                requested_project: Uuid::from_u128(4),
            },
            409,
            "project_mismatch",
        ),
        (
            AgentDraftRefusal::FieldRefused { field: "name" },
            400,
            "publish_field_refused",
        ),
    ];
    for (refused, status, code) in refusals {
        let message = refused.to_string();
        let (answered, body) = body_of(refused.into()).await;
        assert_eq!(answered.as_u16(), status, "{code}");
        assert_eq!(
            body,
            serde_json::json!({ "code": code, "error": code, "message": message })
        );
    }
    let (status, body) = body_of(PublishError::SandboxTokenRefused).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "sandbox_token_refused");
}

/// Guard 6, pinned where the other five are: the retention rule every
/// publish prunes by keeps the builds production served apart from drafts, so
/// a token publishing drafts all day leaves every rollback target.
#[test]
fn drafts_never_prune_a_build_production_served() {
    let served: Vec<Uuid> = (1..=3).map(Uuid::from_u128).collect();
    let drafts: Vec<Uuid> = (100..400).map(Uuid::from_u128).collect();
    let newest_first: Vec<Uuid> = served.iter().chain(&drafts).rev().copied().collect();
    let production: HashSet<Uuid> = served.iter().copied().collect();
    let beyond = retention::beyond_windows(&newest_first, &HashSet::new(), &production, 10);
    assert!(served.iter().all(|build| !beyond.contains(build)));
    assert_eq!(beyond.len(), drafts.len() - 10);
}

/// Guard 1, at the door: `staging` names a draft only for a token granted
/// staging, and never with `promote`. Every other credential — a token minted
/// without staging included — is told what it always was.
#[test]
fn only_a_token_granted_staging_may_name_staging() {
    for name in ["staging", " staging "] {
        let target = target_of(Some(name), false, StagingAgent);
        assert!(matches!(target, Ok(PublishTarget::AgentDraft)), "{name:?}");
    }
    let promoting = target_of(Some("staging"), true, StagingAgent).expect_err("refused");
    assert!(matches!(promoting, PublishError::SandboxTokenRefused));

    for credential in [Other, PublishToken, SandboxAgent] {
        for promote in [false, true] {
            let refused = target_of(Some("staging"), promote, credential).expect_err("refused");
            assert!(
                matches!(&refused, PublishError::InvalidEnvironment(name) if name == "staging"),
                "{credential:?} promote={promote}: {refused}"
            );
            assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
            assert_eq!(refused.code(), None);
        }
    }
}

/// Staging is all the grant adds at the door: no environment, a promote and
/// production are refused to it exactly as to a token without it, and a
/// sandbox is still a sandbox.
#[test]
fn the_grant_changes_nothing_else_a_publish_may_name() {
    for (environment, promote) in [
        (None, false),
        (None, true),
        (Some(""), false),
        (Some("  "), true),
        (Some("dev-a1"), true),
    ] {
        for credential in [SandboxAgent, StagingAgent] {
            let refused = target_of(environment, promote, credential).expect_err("refused");
            assert!(
                matches!(refused, PublishError::SandboxTokenRefused),
                "{credential:?} {environment:?} promote={promote}: {refused}"
            );
        }
    }
    for credential in [SandboxAgent, StagingAgent] {
        let production = target_of(Some("production"), false, credential).expect_err("refused");
        assert!(matches!(production, PublishError::InvalidEnvironment(_)));
        let sandbox = target_of(Some("dev-a1"), false, credential);
        assert!(matches!(sandbox, Ok(PublishTarget::Sandbox(_))));
    }
    assert!(StagingAgent.is_agent() && SandboxAgent.is_agent());
    assert!(!Other.is_agent() && !PublishToken.is_agent());
}

/// The credential admission builds for the token, with or without staging.
fn token_user(staging: bool) -> oxy_auth::types::AuthenticatedUser {
    let mut credential = fixture::credential(Uuid::from_u128(0x70), ORG, APP, WORKSPACE);
    credential.app_sandbox[0].staging = staging;
    fixture::user(Some(credential))
}

fn by_token(input: PublishInput) -> PublishInput {
    PublishInput {
        publisher: Some(Caller::from_user(&token_user(true))),
        ..input
    }
}

/// The credential of a request is the staging one only when its token holds
/// a staging grant.
#[test]
fn a_request_is_a_staging_agents_only_with_a_staging_grant() {
    assert_eq!(PublishCredential::of(&token_user(true), None), StagingAgent);
    assert_eq!(
        PublishCredential::of(&token_user(false), None),
        SandboxAgent
    );
    assert_eq!(PublishCredential::of(&fixture::user(None), None), Other);
}

/// The second refusal, for a draft: asked of the publisher and the target
/// alone, before anything is read. The channels stay refused to the token —
/// the draft target is its one way to staging — and the draft is held to its
/// shape.
#[test]
fn the_draft_target_is_the_tokens_only_way_to_staging() {
    let held = AgentPublish::of(&by_token(input()), &PublishTarget::AgentDraft);
    assert!(held.expect("a draft").is_some());

    for promote in [false, true] {
        let channels = PublishInput {
            promote,
            ..by_token(input())
        };
        let refused = AgentPublish::of(&channels, &PublishTarget::Channels);
        assert!(matches!(refused, Err(PublishError::SandboxTokenRefused)));
    }
    let promoting = PublishInput {
        promote: true,
        ..by_token(input())
    };
    let refused = AgentPublish::of(&promoting, &PublishTarget::AgentDraft);
    assert!(matches!(refused, Err(PublishError::SandboxTokenRefused)));
    let named = PublishInput {
        name: Some("Renamed".into()),
        ..by_token(input())
    };
    let refused = AgentPublish::of(&named, &PublishTarget::AgentDraft);
    assert!(matches!(
        refused,
        Err(PublishError::AgentDraft(AgentDraftRefusal::FieldRefused {
            field: "name"
        }))
    ));
    // Not the token: none of this module's business, as before.
    let person = PublishInput {
        publisher: Some(Caller::from_user(&fixture::user(None))),
        ..input()
    };
    let held = AgentPublish::of(&person, &PublishTarget::AgentDraft);
    assert!(held.expect("not refused here").is_none());
}

/// "Not yours" is told to a draft as the staging it was not granted, in the
/// code its other routes give; what describes its own request passes.
#[test]
fn a_draft_is_told_not_yours_as_no_such_environment() {
    let held = AgentPublish::of(&by_token(input()), &PublishTarget::AgentDraft)
        .expect("a draft")
        .expect("held as the token's");
    let hidden = [
        PublishError::UnknownOrg("acme".into()),
        PublishError::UnknownProject(Uuid::nil(), "acme".into()),
        PublishError::OxyAccessDenied {
            org: "acme".into(),
            project: Uuid::nil(),
        },
        PublishError::SandboxRefused,
    ];
    for refused in hidden {
        let seen = held.sees(refused);
        assert!(
            matches!(seen, PublishError::AgentDraft(AgentDraftRefusal::NotFound)),
            "{seen}"
        );
        assert_eq!(seen.status(), StatusCode::NOT_FOUND);
        assert_eq!(seen.code(), Some("environment_not_found"));
    }
    let passed = held.sees(PublishError::InvalidBuildId("..".into()));
    assert!(matches!(passed, PublishError::InvalidBuildId(_)));
    let passed = held.sees(AgentDraftRefusal::FieldRefused { field: "branch" }.into());
    assert!(matches!(passed, PublishError::AgentDraft(_)));
}
