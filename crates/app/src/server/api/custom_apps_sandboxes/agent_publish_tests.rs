use super::*;
use crate::server::api::custom_apps_agent_fixture as fixture;
use oxy_server_authz::Caller;

const TOKEN: Uuid = Uuid::from_u128(0x70);
const APP: Uuid = Uuid::from_u128(1);

fn input(publisher: Option<Caller>, promote: bool) -> PublishInput {
    PublishInput {
        org_ref: None,
        app_slug: "ops".into(),
        project_id: Uuid::from_u128(3),
        branch: None,
        build_id: "b1".into(),
        name: None,
        promote,
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

fn agent() -> Caller {
    let credential = fixture::credential(TOKEN, Uuid::from_u128(2), APP, Uuid::from_u128(3));
    Caller::from_user(&fixture::user(Some(credential)))
}

fn person() -> Caller {
    Caller::from_user(&fixture::user(None))
}

fn sandbox() -> PublishTarget {
    PublishTarget::Sandbox(AppEnvironment::parse("dev-a").expect("a sandbox"))
}

/// The second refusal of a production write: asked of the publisher and the
/// target alone, so it holds with no route allow-list in front of it.
#[test]
fn the_token_is_refused_the_channels_and_promote() {
    for (target, promote) in [
        (PublishTarget::Channels, false),
        (PublishTarget::Channels, true),
        (sandbox(), true),
    ] {
        let refused = AgentPublish::of(&input(Some(agent()), promote), &target);
        assert!(
            matches!(refused, Err(PublishError::SandboxTokenRefused)),
            "{target:?} promote={promote}: {refused:?}"
        );
    }
    let admitted = AgentPublish::of(&input(Some(agent()), false), &sandbox());
    assert_eq!(
        admitted.expect("a sandbox publish"),
        Some(AgentPublish {
            environment: "dev-a".into()
        })
    );
}

/// Every other publisher is none of this module's business.
#[test]
fn no_other_publisher_is_held_to_a_sandbox() {
    for publisher in [Some(person()), None] {
        for (target, promote) in [
            (PublishTarget::Channels, false),
            (PublishTarget::Channels, true),
            (sandbox(), false),
            (sandbox(), true),
        ] {
            let held = AgentPublish::of(&input(publisher.clone(), promote), &target);
            assert_eq!(held.expect("not refused here"), None);
        }
    }
    assert_eq!(token_of(&input(Some(person()), false)), None);
    assert_eq!(token_of(&input(Some(agent()), false)), Some(TOKEN));
}

/// "Not yours" is told to the token as "no such sandbox"; what describes its
/// own request is passed through.
#[test]
fn a_token_is_told_not_yours_as_not_found() {
    let agent = AgentPublish {
        environment: "dev-a".into(),
    };
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
        let seen = agent.sees(refused);
        assert!(
            matches!(&seen, PublishError::UnknownEnvironment { name } if name == "dev-a"),
            "{seen}"
        );
        assert_eq!(seen.status(), axum::http::StatusCode::NOT_FOUND);
    }
    let passed = agent.sees(PublishError::InvalidBuildId("..".into()));
    assert!(matches!(passed, PublishError::InvalidBuildId(_)));
    let passed = agent.sees(PublishError::EnvironmentDeleting {
        name: "dev-a".into(),
    });
    assert!(matches!(passed, PublishError::EnvironmentDeleting { .. }));
}
