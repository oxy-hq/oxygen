use oxy_authz::RoleCeiling;
use serde_json::json;

use super::*;

const ORG: Uuid = Uuid::from_u128(0xA);
const ELSEWHERE: Uuid = Uuid::from_u128(0xB);
const WS: Uuid = Uuid::from_u128(0x1);
const APP: Uuid = Uuid::from_u128(0x2);

fn body(value: serde_json::Value) -> CreatePolicyBody {
    serde_json::from_value(value).expect("a create body")
}

fn patch(value: serde_json::Value) -> PatchPolicyBody {
    serde_json::from_value(value).expect("a patch body")
}

fn whole_org(ceiling: &str) -> serde_json::Value {
    json!({ "kind": "workspace", "workspace_id": null, "role_ceiling": ceiling })
}

fn base() -> serde_json::Value {
    json!({
        "repository": "acme/app",
        "workflow_path": ".github/workflows/release.yml",
        "environment": "production",
        "grants": [whole_org("member")],
    })
}

fn with(mut value: serde_json::Value, key: &str, field: serde_json::Value) -> serde_json::Value {
    value[key] = field;
    value
}

#[test]
fn a_full_body_parses_to_what_it_says() {
    let want = body(base()).want(ORG, AccountRole::Member).expect("valid");
    assert_eq!(want.repository(), "acme/app");
    assert_eq!(want.workflow_path, ".github/workflows/release.yml");
    assert_eq!(want.environment.as_deref(), Some("production"));
    assert_eq!(want.ref_pattern, None);
    assert!(!want.allow_self_hosted);
    assert_eq!(want.explicit_ids, None);
    assert_eq!(
        want.grants,
        vec![PolicyGrant::Workspace(GrantSpec {
            org_id: ORG,
            workspace_id: None,
            ceiling: RoleCeiling::Member,
        })]
    );
}

#[test]
fn a_repository_is_owner_slash_repo_in_githubs_characters() {
    for good in ["acme/app", "Acme-Corp/my.repo_name", "a/b", " acme/app "] {
        assert!(parse_repository(good).is_ok(), "{good}");
    }
    for bad in [
        "acme",
        "acme/",
        "/app",
        "acme/app/extra",
        "acme/..",
        "acme/.",
        "-acme/app",
        "acme-/app",
        "acme/app?x=1",
        "acme/app#frag",
        "acme/ap p",
        "acme/app%2f..",
        "ac me/app",
        "acme@evil.example/app",
        "https://github.com/acme/app",
        "",
    ] {
        assert!(parse_repository(bad).is_err(), "{bad:?} must be refused");
    }
}

#[test]
fn a_workflow_path_is_a_workflow_file_here_or_a_reusable_one_elsewhere() {
    for good in [
        ".github/workflows/release.yml",
        ".github/workflows/deploy.yaml",
        ".github/workflows/sub/dir.yml",
        "acme/shared/.github/workflows/deploy.yml",
    ] {
        assert_eq!(clean_workflow_path(good).as_deref(), Ok(good), "{good}");
    }
    for bad in [
        "release.yml",
        ".github/workflows/",
        ".github/workflows/.yml",
        ".github/workflows/release.yml@refs/heads/main",
        ".github/workflows/../../etc/passwd.yml",
        ".github/workflows/release.sh",
        ".github/actions/release.yml",
        "acme/.github/workflows/release.yml",
        "acme/shared/workflows/deploy.yml",
        "acme/shared/.github/workflows/deploy.yml@v1",
        ".github/workflows/re lease.yml",
        "",
    ] {
        assert!(clean_workflow_path(bad).is_err(), "{bad:?} must be refused");
    }
}

#[test]
fn a_blank_environment_or_ref_pattern_is_none() {
    for blank in [json!(null), json!(""), json!("   ")] {
        let value = with(
            with(base(), "environment", blank.clone()),
            "ref_pattern",
            blank,
        );
        let want = body(value).want(ORG, AccountRole::Member).expect("valid");
        assert_eq!(want.environment, None);
        assert_eq!(want.ref_pattern, None);
    }
    // Left out is the same as null on a create.
    let mut value = base();
    value.as_object_mut().unwrap().remove("environment");
    let want = body(value).want(ORG, AccountRole::Member).expect("valid");
    assert_eq!(want.environment, None);
}

#[test]
fn a_ref_pattern_is_a_full_ref() {
    for good in ["refs/heads/main", "refs/tags/v*", "refs/*"] {
        assert_eq!(clean_ref_pattern(Some(good)), Ok(Some(good.to_string())));
    }
    for bad in ["main", "*", "heads/main", "refs/heads/ma in"] {
        assert!(clean_ref_pattern(Some(bad)).is_err(), "{bad:?}");
    }
}

#[test]
fn grants_are_required_and_never_defaulted() {
    let mut value = base();
    value.as_object_mut().unwrap().remove("grants");
    assert!(body(value).want(ORG, AccountRole::Admin).is_err());
    let empty = with(base(), "grants", json!([]));
    assert!(body(empty).want(ORG, AccountRole::Admin).is_err());
}

#[test]
fn a_workspace_grant_states_its_ceiling() {
    let silent = with(base(), "grants", json!([{ "kind": "workspace" }]));
    assert!(body(silent).want(ORG, AccountRole::Admin).is_err());
}

#[test]
fn a_ceiling_above_the_accounts_role_is_refused() {
    let admin = with(base(), "grants", json!([whole_org("admin")]));
    assert!(body(admin.clone()).want(ORG, AccountRole::Member).is_err());
    assert!(body(admin).want(ORG, AccountRole::Admin).is_ok());
    // `owner` is never valid for a service account.
    let owner = with(base(), "grants", json!([whole_org("owner")]));
    assert!(body(owner).want(ORG, AccountRole::Admin).is_err());
    // Below the role is fine.
    let viewer = with(base(), "grants", json!([whole_org("viewer")]));
    assert!(body(viewer).want(ORG, AccountRole::Member).is_ok());
}

#[test]
fn a_grant_in_another_org_is_refused() {
    let elsewhere = json!([{ "kind": "workspace", "org_id": ELSEWHERE, "role_ceiling": "member" }]);
    let value = with(base(), "grants", elsewhere);
    assert!(body(value).want(ORG, AccountRole::Member).is_err());
    let app_elsewhere = json!([{ "kind": "app_publish", "org_id": ELSEWHERE, "app_id": APP }]);
    let value = with(base(), "grants", app_elsewhere);
    assert!(body(value).want(ORG, AccountRole::Member).is_err());
}

#[test]
fn an_app_publish_grant_names_one_app_and_nothing_else() {
    let grants = json!([
        { "kind": "app_publish", "app_id": APP },
        { "kind": "app_publish", "app_id": APP },
        { "kind": "workspace", "workspace_id": WS, "role_ceiling": "viewer" },
    ]);
    let want = body(with(base(), "grants", grants))
        .want(ORG, AccountRole::Member)
        .expect("valid");
    assert_eq!(
        want.grants,
        vec![
            PolicyGrant::Workspace(GrantSpec {
                org_id: ORG,
                workspace_id: Some(WS),
                ceiling: RoleCeiling::Viewer,
            }),
            PolicyGrant::AppPublish {
                org_id: ORG,
                app_id: APP,
            },
        ],
        "the duplicate app grant is one grant"
    );

    for bad in [
        json!([{ "kind": "app_publish" }]),
        json!([{ "kind": "app_publish", "app_id": APP, "role_ceiling": "admin" }]),
        json!([{ "kind": "app_publish", "app_id": APP, "workspace_id": WS }]),
        json!([{ "kind": "workspace", "role_ceiling": "member", "app_id": APP }]),
        json!([{ "kind": "everything" }]),
    ] {
        let value = with(base(), "grants", bad.clone());
        assert!(
            body(value).want(ORG, AccountRole::Admin).is_err(),
            "{bad} must be refused"
        );
    }
}

#[test]
fn explicit_ids_come_together_or_not_at_all() {
    let both = with(
        with(base(), "repository_id", json!(987)),
        "repository_owner_id",
        json!(42),
    );
    let want = body(both).want(ORG, AccountRole::Member).expect("valid");
    assert_eq!(
        want.explicit_ids,
        Some(RepoIds {
            repository_id: 987,
            repository_owner_id: 42,
        })
    );
    let one = with(base(), "repository_id", json!(987));
    assert!(body(one).want(ORG, AccountRole::Member).is_err());
    let negative = with(
        with(base(), "repository_id", json!(-1)),
        "repository_owner_id",
        json!(42),
    );
    assert!(body(negative).want(ORG, AccountRole::Member).is_err());
}

#[test]
fn a_patch_changes_only_what_it_names() {
    let edit = patch(json!({ "allow_self_hosted": true }))
        .edit(ORG, AccountRole::Member)
        .expect("valid");
    assert_eq!(
        edit,
        PolicyEdit {
            allow_self_hosted: Some(true),
            ..PolicyEdit::default()
        }
    );
}

#[test]
fn a_patch_tells_null_from_absent() {
    let cleared = patch(json!({ "environment": null, "ref_pattern": "" }))
        .edit(ORG, AccountRole::Member)
        .expect("valid");
    assert_eq!(cleared.environment, Some(None));
    assert_eq!(cleared.ref_pattern, Some(None));
    let untouched = patch(json!({}))
        .edit(ORG, AccountRole::Member)
        .expect("valid");
    assert_eq!(untouched.environment, None);
    assert_eq!(untouched.ref_pattern, None);
}

#[test]
fn a_patch_replaces_the_grant_set_and_may_not_empty_it() {
    let edit = patch(json!({ "grants": [whole_org("viewer")] }))
        .edit(ORG, AccountRole::Member)
        .expect("valid");
    assert_eq!(edit.grants.map(|g| g.len()), Some(1));
    assert!(
        patch(json!({ "grants": [] }))
            .edit(ORG, AccountRole::Member)
            .is_err()
    );
    assert!(
        patch(json!({ "grants": [whole_org("admin")] }))
            .edit(ORG, AccountRole::Member)
            .is_err()
    );
}

#[test]
fn a_patch_cannot_move_a_policy_to_another_repository() {
    for body in [
        json!({ "repository": "acme/other" }),
        json!({ "repository_id": 1 }),
        json!({ "repository_owner_id": 1 }),
    ] {
        assert!(patch(body).edit(ORG, AccountRole::Admin).is_err());
    }
}
