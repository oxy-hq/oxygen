//! Who counts as the same identity, and what the one production function
//! refuses under the lock.

use oxy_auth::token::StoredKind;

use super::*;
use crate::custom_apps_agent_fixture as fixture;
use crate::custom_apps_promote::blockers::same_person_rollback;
use crate::custom_apps_promote::error::SAME_PERSON_PROMOTE;

const ALICE: Uuid = Uuid::from_u128(0xA11CE);
const BOB: Uuid = Uuid::from_u128(0xB0B);
const SA: Uuid = Uuid::from_u128(0x5A);
const WORKFLOW: &str = "github-oidc:acme/app/.github/workflows/publish.yml@refs/heads/main";
const BUILD: Uuid = Uuid::from_u128(0xB1D);
const LIVE: Uuid = Uuid::from_u128(0x11FE);

fn user(id: Uuid) -> Identity {
    Identity::new(Some(id), None)
}

fn machine(via: &str) -> Identity {
    Identity::new(None, Some(via.to_string()))
}

#[test]
fn the_same_user_is_the_same_identity_and_another_user_is_not() {
    assert!(user(ALICE).is(&user(ALICE)));
    assert!(!user(ALICE).is(&user(BOB)));
}

/// A trusted publisher's workflow, which no user published as, is the same
/// identity only as itself.
#[test]
fn a_workflow_is_the_same_identity_as_itself_and_not_as_a_person() {
    assert!(machine(WORKFLOW).is(&machine(WORKFLOW)));
    assert!(!machine(WORKFLOW).is(&machine("github-oidc:acme/other")));
    assert!(!machine(WORKFLOW).is(&user(ALICE)));
    assert!(!user(ALICE).is(&machine(WORKFLOW)));
}

/// A CI token's service account: two of them sharing a token name are two
/// identities, because both name a user and the users differ.
#[test]
fn users_decide_before_machine_identities_do() {
    let ci = |id| Identity::new(Some(id), Some("ci".to_string()));
    assert!(ci(SA).is(&ci(SA)));
    assert!(!ci(SA).is(&ci(BOB)));
}

/// The machine principal's nil id and an empty name are nobody; nobody is not
/// the same as nobody, or every unattributed build would refuse every mover.
#[test]
fn nobody_is_never_the_same_identity() {
    let nil = Identity::new(Some(Uuid::nil()), Some(String::new()));
    assert_eq!(nil, Identity::default());
    assert!(!nil.is(&Identity::default()));
    assert!(!Identity::default().is(&user(ALICE)));
}

/// What `oxyc promote` and the console send: the user, and a CI token's name.
#[test]
fn a_request_carries_its_user_and_a_ci_tokens_name() {
    let session = fixture::user(None);
    assert_eq!(Identity::of_user(&session), user(session.id));
    let mut credential = fixture::credential(Uuid::from_u128(1), ALICE, BOB, SA);
    credential.kind = StoredKind::Ci;
    credential.name = "deploy".into();
    let ci = fixture::user(Some(credential));
    assert_eq!(via_of(&ci).as_deref(), Some("deploy"));
    // Any other kind of token is the person who holds it.
    assert_eq!(
        via_of(&fixture::user(Some(fixture::credential(
            Uuid::from_u128(1),
            ALICE,
            BOB,
            SA
        )))),
        None
    );
}

fn build(published_by: Option<Uuid>, published_via: Option<&str>) -> app_builds::Model {
    serde_json::from_value(serde_json::json!({
        "id": BUILD,
        "app_id": Uuid::from_u128(9),
        "build_id": "b-7",
        "s3_prefix": "customer-apps/x/builds/b-7/",
        "manifest_json": null,
        "created_at": chrono::Utc::now().fixed_offset(),
        "published_by": published_by,
        "published_via": published_via,
        "source_repo": null,
        "commit_sha": null,
        "source_branch": null,
        "validation_status": "passed",
        "validation_detail": null,
        "semantic_revision_id": null,
    }))
    .expect("an app_builds row")
}

fn app(rule: bool) -> apps::Model {
    let mut row = fixture::app(Uuid::from_u128(9), Uuid::from_u128(7), Uuid::nil());
    row.require_second_person_to_promote = rule;
    row
}

fn moved_by(action: EnvAction, actor: Option<Uuid>, via: Option<&str>) -> ProductionMove {
    ProductionMove {
        app_id: Uuid::from_u128(9),
        build: BUILD.into(),
        action,
        actor,
        actor_via: via.map(str::to_string),
        expect_current: None,
        refuse_same: false,
    }
}

/// Promote × rule on/off × same/different mover: only the build's publisher
/// moving it with the rule on is asked for a second person.
#[test]
fn a_promote_by_its_publisher_is_asked_only_while_the_rule_is_on() {
    let published = build(Some(ALICE), None);
    let by_alice = moved_by(EnvAction::Promote, Some(ALICE), None);
    let by_bob = moved_by(EnvAction::Promote, Some(BOB), None);
    let live = Some(LIVE);
    assert_eq!(
        asks(&app(true), &published, &by_alice, live),
        Some(EnvAction::Promote)
    );
    assert_eq!(asks(&app(true), &published, &by_bob, live), None);
    assert_eq!(asks(&app(false), &published, &by_alice, live), None);
    assert_eq!(asks(&app(false), &published, &by_bob, live), None);
}

/// A rollback by the build's publisher is asked too: production's history
/// decides it (exempt only to a build production served before, which the
/// integration test `promote_rollback::second_person` drives through the
/// console's rollback). Someone else's rollback is never asked.
#[test]
fn a_rollback_by_its_publisher_is_left_to_productions_history() {
    let published = build(Some(ALICE), None);
    let by_alice = moved_by(EnvAction::Rollback, Some(ALICE), None);
    let by_bob = moved_by(EnvAction::Rollback, Some(BOB), None);
    assert_eq!(
        asks(&app(true), &published, &by_alice, Some(LIVE)),
        Some(EnvAction::Rollback)
    );
    assert_eq!(asks(&app(true), &published, &by_bob, Some(LIVE)), None);
    assert_eq!(asks(&app(false), &published, &by_alice, Some(LIVE)), None);
}

/// A promoting publish moves as its own publisher: a trusted publisher's
/// workflow (no user) is asked as surely as a person.
#[test]
fn a_promoting_publish_by_a_workflow_is_asked_with_the_rule_on() {
    let published = build(None, Some(WORKFLOW));
    let publish = moved_by(EnvAction::Promote, None, Some(WORKFLOW));
    assert_eq!(
        asks(&app(true), &published, &publish, None),
        Some(EnvAction::Promote)
    );
    assert_eq!(asks(&app(false), &published, &publish, None), None);
}

/// The console's Re-publish of the build production already serves moves
/// nothing new into production, so the rule has nothing to ask.
#[test]
fn re_stamping_the_live_build_is_not_asked() {
    let published = build(Some(ALICE), None);
    for action in [EnvAction::Promote, EnvAction::Rollback] {
        let restamp = moved_by(action, Some(ALICE), None);
        assert_eq!(asks(&app(true), &published, &restamp, Some(BUILD)), None);
    }
}

/// The two sentences share a code, and the rollback one says why a rollback
/// was refused and what is still open.
#[test]
fn a_refused_rollback_says_it_is_a_promote_by_another_name() {
    let rollback = same_person_rollback("b-7");
    assert_eq!(rollback.code, SAME_PERSON_PROMOTE);
    assert!(
        rollback.message.contains("never served"),
        "{}",
        rollback.message
    );
    assert!(
        rollback.message.contains("served before"),
        "{}",
        rollback.message
    );
}
