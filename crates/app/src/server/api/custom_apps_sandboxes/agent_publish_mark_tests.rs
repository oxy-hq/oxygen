//! Which publish marks its build with a sandbox agent token
//! (`agent_publish::author`, `app_builds.published_token_id`): every one the
//! token makes, and no one else's. What reads the mark — the promote paths
//! and production's fallback — is exercised against a database
//! (`tests/custom_apps/sandbox_agent_token/{staging_promote, sandbox_promote}`).

use super::*;
use crate::server::api::custom_apps_agent_fixture as fixture;
use oxy_server_authz::Caller;

const TOKEN: Uuid = Uuid::from_u128(0x70);

fn input(publisher: Option<Caller>) -> PublishInput {
    PublishInput {
        org_ref: None,
        app_slug: "ops".into(),
        project_id: Uuid::from_u128(3),
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
        publisher,
        machine_app_id: None,
        published_via: None,
        semantic_revision_id: None,
    }
}

/// The token, minted with staging or without: the mark does not ask.
fn token(staging: bool) -> Caller {
    let workspace = Uuid::from_u128(3);
    let mut credential =
        fixture::credential(TOKEN, Uuid::from_u128(2), Uuid::from_u128(1), workspace);
    credential.app_sandbox[0].staging = staging;
    Caller::from_user(&fixture::user(Some(credential)))
}

/// Every build a sandbox agent token publishes carries the token's id — a
/// publish to a sandbox of its own as much as a draft to staging, and for a
/// token minted without staging as much as one with. The mark reads the
/// publisher and nothing else, so no target is left out of it.
#[test]
fn every_build_a_token_publishes_is_marked_with_the_token() {
    for staging in [false, true] {
        let by_token = input(Some(token(staging)));
        assert_eq!(author(&by_token), Some(TOKEN), "staging={staging}");
    }
}

/// Every build a person, a CI job or a machine publishes is unmarked: the
/// column stays `NULL`, and neither rule that reads it is asked anything.
#[test]
fn no_one_elses_publish_marks_its_build() {
    let person = Caller::from_user(&fixture::user(None));
    assert_eq!(author(&input(Some(person))), None);
    // A machine publish (OIDC trusted publishing) carries no caller at all.
    assert_eq!(author(&input(None)), None);
    let machine = PublishInput {
        published_by: None,
        published_by_email: None,
        machine_app_id: Some(Uuid::from_u128(1)),
        published_via: Some("github-oidc:acme/app".into()),
        ..input(None)
    };
    assert_eq!(author(&machine), None);
}
