//! What each kind of grant stores, and that both tables store it alike.

use super::*;
use chrono::{TimeZone, Utc};
use oxy_authz::RoleCeiling;

const ORG: Uuid = Uuid::from_u128(7);
const WORKSPACE: Uuid = Uuid::from_u128(70);
const APP: Uuid = Uuid::from_u128(0xA99);
const PARENT: Uuid = Uuid::from_u128(1);

fn spec(workspace_id: Option<Uuid>, ceiling: RoleCeiling) -> GrantSpec {
    GrantSpec {
        org_id: ORG,
        workspace_id,
        ceiling,
    }
}

fn when() -> DateTime<FixedOffset> {
    Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0)
        .unwrap()
        .fixed_offset()
}

#[test]
fn a_workspace_grant_stores_its_org_its_workspace_and_its_ceiling() {
    let row = GrantRow::workspace(&spec(Some(WORKSPACE), RoleCeiling::Admin));
    let expected = GrantRow {
        kind: "workspace",
        org_id: ORG,
        workspace_id: Some(WORKSPACE),
        role_ceiling: Some("admin".into()),
        app_id: None,
    };
    assert_eq!(row, expected);
}

#[test]
fn an_org_wide_grant_names_no_workspace() {
    let row = GrantRow::workspace(&spec(None, RoleCeiling::Viewer));
    assert_eq!(row.workspace_id, None);
    assert_eq!(row.role_ceiling.as_deref(), Some("viewer"));
}

#[test]
fn an_app_publish_grant_stores_its_app_and_no_ceiling() {
    let row = GrantRow::of(&PolicyGrant::AppPublish {
        org_id: ORG,
        app_id: APP,
    });
    let expected = GrantRow {
        kind: "app_publish",
        org_id: ORG,
        workspace_id: None,
        role_ceiling: None,
        app_id: Some(APP),
    };
    assert_eq!(row, expected);
}

#[test]
fn a_policys_workspace_grant_is_the_workspace_grant() {
    let grant = spec(Some(WORKSPACE), RoleCeiling::Member);
    assert_eq!(
        GrantRow::of(&PolicyGrant::Workspace(grant.clone())),
        GrantRow::workspace(&grant)
    );
}

#[test]
fn a_tokens_row_is_live_and_hangs_off_the_token() {
    let row = GrantRow::workspace(&spec(None, RoleCeiling::Member)).for_token(PARENT, when());
    assert_eq!(row.token_id, Set(PARENT));
    assert_eq!(row.created_at, Set(when()));
    assert_eq!(row.revoked_at, Set(None));
    assert_eq!(row.revoked_by, Set(None));
}

#[test]
fn both_tables_store_a_grant_in_the_same_columns() {
    let grants = [
        PolicyGrant::Workspace(spec(Some(WORKSPACE), RoleCeiling::Owner)),
        PolicyGrant::AppPublish {
            org_id: ORG,
            app_id: APP,
        },
    ];
    for grant in &grants {
        let expected = GrantRow::of(grant);
        let token = GrantRow::of(grant).for_token(PARENT, when());
        let policy = GrantRow::of(grant).for_policy(PARENT, when());
        assert_eq!(policy.policy_id, Set(PARENT));
        assert_eq!(policy.created_at, Set(when()));
        for (kind, org_id, workspace_id, role_ceiling, app_id) in [
            (
                token.kind,
                token.org_id,
                token.workspace_id,
                token.role_ceiling,
                token.app_id,
            ),
            (
                policy.kind,
                policy.org_id,
                policy.workspace_id,
                policy.role_ceiling,
                policy.app_id,
            ),
        ] {
            assert_eq!(kind, Set(expected.kind.to_string()));
            assert_eq!(org_id, Set(expected.org_id));
            assert_eq!(workspace_id, Set(expected.workspace_id));
            assert_eq!(role_ceiling, Set(expected.role_ceiling.clone()));
            assert_eq!(app_id, Set(expected.app_id));
        }
    }
}
