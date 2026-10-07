//! Admission of the `app_staging` grant: the option a sandbox agent token may
//! be minted with, beside the `app_sandbox` grant of the same app.
//!
//! It belongs to that kind alone, widens nothing without its twin, and a
//! reader that does not know the kind refuses the token whole — which is what
//! a binary one release back does with it.

use super::*;
use crate::token::credential::{SandboxAppHome, place_sandbox_apps, source};

const ORG: Uuid = Uuid::from_u128(7);
const ELSEWHERE: Uuid = Uuid::from_u128(8);
const APP: Uuid = Uuid::from_u128(0xA99);
const OTHER_APP: Uuid = Uuid::from_u128(0xA9A);
const WORKSPACE: Uuid = Uuid::from_u128(0x77);
const MINTER: Uuid = Uuid::from_u128(1);

fn row(kind: &str, prefix: &str) -> api_tokens::Model {
    let now = Utc::now().fixed_offset();
    api_tokens::Model {
        id: Uuid::from_u128(0x5B),
        kind: kind.into(),
        principal_user_id: MINTER,
        name: "agent".into(),
        display_prefix: prefix.into(),
        last_four: "wxyz".into(),
        token_hash: vec![0; 32],
        all_access: false,
        platform: true,
        partner: false,
        expires_at: Some((Utc::now() + chrono::Duration::hours(8)).fixed_offset()),
        last_used_at: None,
        created_at: now,
        created_by: Some(MINTER),
        revoked_at: None,
        revoked_by: None,
        revoke_reason: None,
        source: source::UI.into(),
        legacy_api_key_id: None,
        trust_policy_id: None,
        oidc_claims: None,
    }
}

fn sandbox_row() -> api_tokens::Model {
    row("sandbox_agent", "oxy_sbx_Ab3x")
}

fn grant(kind: &str, org_id: Uuid, app_id: Option<Uuid>) -> api_token_grants::Model {
    api_token_grants::Model {
        id: Uuid::new_v4(),
        token_id: sandbox_row().id,
        kind: kind.into(),
        org_id,
        workspace_id: None,
        role_ceiling: (kind == "workspace").then(|| "admin".to_string()),
        app_id,
        created_at: Utc::now().fixed_offset(),
        revoked_at: None,
        revoked_by: None,
    }
}

fn sandbox(app_id: Uuid) -> api_token_grants::Model {
    grant("app_sandbox", ORG, Some(app_id))
}

fn staging(app_id: Uuid) -> api_token_grants::Model {
    grant("app_staging", ORG, Some(app_id))
}

fn revoked(mut grant: api_token_grants::Model) -> api_token_grants::Model {
    grant.revoked_at = Some(Utc::now().fixed_offset());
    grant
}

fn admit_as(
    row: &api_tokens::Model,
    grants: &[api_token_grants::Model],
    presented: TokenFormat,
) -> Result<CredentialContext, Refusal> {
    admit(row, grants, presented, Links::NONE, Utc::now())
}

fn admit_sandbox(grants: &[api_token_grants::Model]) -> Result<CredentialContext, Refusal> {
    admit_as(&sandbox_row(), grants, TokenFormat::SandboxAgent)
}

fn unknown_grant(result: Result<CredentialContext, Refusal>) -> String {
    match result {
        Err(Refusal::UnknownGrant(what)) => what,
        other => panic!("expected an unknown-grant refusal, got {other:?}"),
    }
}

#[test]
fn the_kind_is_the_one_the_entity_names() {
    assert_eq!(api_token_grants::KIND_APP_STAGING, "app_staging");
}

#[test]
fn a_staging_grant_marks_the_app_it_names_and_no_other() {
    let cred = admit_sandbox(&[sandbox(APP), sandbox(OTHER_APP), staging(APP)]).unwrap();
    assert_eq!(
        cred.app_sandbox,
        vec![
            AppSandboxGrant {
                org_id: ORG,
                app_id: APP,
                staging: true,
            },
            AppSandboxGrant {
                org_id: ORG,
                app_id: OTHER_APP,
                staging: false,
            },
        ]
    );
    assert!(cred.stages_app(APP) && !cred.stages_app(OTHER_APP));
    assert!(cred.stages_any_app());
    // It is one more fact on an app's grant: no workspace grant, no publish
    // grant, and the same kind of credential.
    assert!(cred.grants.is_empty() && cred.app_publish.is_empty());
    assert!(cred.is_sandbox_agent());
}

#[test]
fn the_order_of_the_rows_does_not_matter() {
    let cred = admit_sandbox(&[staging(APP), sandbox(APP)]).unwrap();
    assert!(cred.stages_app(APP));
}

#[test]
fn a_token_minted_without_staging_holds_none() {
    let cred = admit_sandbox(&[sandbox(APP), sandbox(OTHER_APP)]).unwrap();
    assert!(!cred.stages_app(APP) && !cred.stages_app(OTHER_APP));
    assert!(!cred.stages_any_app());
    let reach = cred.reach().unwrap().sandbox_agent.unwrap();
    assert!(reach.apps.iter().all(|app| !app.staging));
}

#[test]
fn the_reach_carries_staging_per_app() {
    let mut cred = admit_sandbox(&[sandbox(APP), sandbox(OTHER_APP), staging(OTHER_APP)]).unwrap();
    let home = |app_id| SandboxAppHome {
        app_id,
        org_id: ORG,
        workspace_id: WORKSPACE,
    };
    place_sandbox_apps(&mut cred, &[home(APP), home(OTHER_APP)]);
    let reach = cred.reach().unwrap().sandbox_agent.unwrap();
    let staged: Vec<(Uuid, bool)> = reach.apps.iter().map(|a| (a.app_id, a.staging)).collect();
    assert_eq!(staged, vec![(APP, false), (OTHER_APP, true)]);
    // An app that left the org its grant names takes its staging with it.
    place_sandbox_apps(&mut cred, &[home(APP)]);
    assert!(!cred.stages_app(OTHER_APP) && !cred.stages_any_app());
}

#[test]
fn a_staging_grant_with_no_sandbox_twin_refuses_the_token() {
    // The app was never granted.
    let alone = unknown_grant(admit_sandbox(&[sandbox(APP), staging(OTHER_APP)]));
    assert_eq!(alone, "app_staging with no app_sandbox grant for its app");
    // It is the token's only grant.
    unknown_grant(admit_sandbox(&[staging(APP)]));
    // The twin names another org.
    let abroad = grant("app_staging", ELSEWHERE, Some(APP));
    unknown_grant(admit_sandbox(&[sandbox(APP), abroad]));
    // The twin was revoked: staging does not outlive the app's grant.
    unknown_grant(admit_sandbox(&[revoked(sandbox(APP)), staging(APP)]));
}

#[test]
fn a_revoked_staging_grant_is_skipped_and_the_app_grant_stands() {
    let cred = admit_sandbox(&[sandbox(APP), revoked(staging(APP))]).unwrap();
    assert!(cred.sandboxes_app(APP) && !cred.stages_app(APP));
}

#[test]
fn a_staging_grant_naming_no_app_is_refused() {
    let what = unknown_grant(admit_sandbox(&[
        sandbox(APP),
        grant("app_staging", ORG, None),
    ]));
    assert_eq!(what, "app_staging naming no app");
}

#[test]
fn the_grant_is_refused_on_every_other_kind_of_token() {
    let personal = row("personal", "oxy_pat_Ab3x");
    let what = unknown_grant(admit_as(&personal, &[staging(APP)], TokenFormat::Personal));
    assert_eq!(what, "kind 'app_staging' on a personal token");
    // Beside a workspace grant, and beside an `app_sandbox` grant, too.
    let workspace = grant("workspace", ORG, None);
    unknown_grant(admit_as(
        &personal,
        &[workspace, staging(APP)],
        TokenFormat::Personal,
    ));
    let what = unknown_grant(admit_as(
        &personal,
        &[sandbox(APP), staging(APP)],
        TokenFormat::Personal,
    ));
    assert_eq!(what, "kind 'app_sandbox' on a personal token");
}

#[test]
fn a_legacy_row_carrying_one_is_refused_as_narrowed() {
    let legacy = api_tokens::Model {
        all_access: true,
        partner: true,
        ..row("legacy_key", "oxy_")
    };
    let refused = admit_as(&legacy, &[staging(APP)], TokenFormat::LegacyKey);
    assert_eq!(refused, Err(Refusal::Narrowed("grants")));
}

/// What a binary one release back does with the kind: it reaches the arm for
/// a kind it does not know, and refuses the token whole. Stated here with a
/// kind this release does not know either, on the same row shape.
#[test]
fn a_reader_that_does_not_know_a_grant_kind_refuses_the_whole_token() {
    let unknown = grant("app_something_newer", ORG, Some(APP));
    let what = unknown_grant(admit_sandbox(&[sandbox(APP), unknown]));
    assert_eq!(what, "kind 'app_something_newer'");
    // And the inventories read no grant from such a token, rather than some.
    let rows = [sandbox(APP), staging(APP)];
    assert_eq!(readable_grants(&rows), Ok(Vec::new()));
    let rows = [sandbox(APP), grant("app_something_newer", ORG, Some(APP))];
    assert!(readable_grants(&rows).is_err());
}
