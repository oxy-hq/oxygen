use super::*;
use chrono::Utc;
use oxy_authz::RoleCeiling;

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

const ORG: u128 = 1;
const WS_A: u128 = 10;
const WS_B: u128 = 11;
const APP: u128 = 50;

fn row(row_id: u128, kind: &str) -> api_token_grants::Model {
    api_token_grants::Model {
        id: id(row_id),
        token_id: id(999),
        kind: kind.to_string(),
        org_id: id(ORG),
        workspace_id: None,
        role_ceiling: None,
        app_id: None,
        created_at: Utc::now().fixed_offset(),
        revoked_at: None,
        revoked_by: None,
    }
}

fn workspace_row(row_id: u128, workspace: Option<u128>, ceiling: &str) -> api_token_grants::Model {
    api_token_grants::Model {
        workspace_id: workspace.map(id),
        role_ceiling: Some(ceiling.to_string()),
        ..row(row_id, KIND_WORKSPACE)
    }
}

fn app_row(row_id: u128, app: u128) -> api_token_grants::Model {
    api_token_grants::Model {
        app_id: Some(id(app)),
        ..row(row_id, KIND_APP_PUBLISH)
    }
}

fn revoked(mut grant: api_token_grants::Model) -> api_token_grants::Model {
    grant.revoked_at = Some(Utc::now().fixed_offset());
    grant.revoked_by = Some(id(7));
    grant
}

fn want(workspace: Option<u128>, ceiling: RoleCeiling) -> GrantWant {
    GrantWant::Workspace {
        org_id: id(ORG),
        workspace_id: workspace.map(id),
        ceiling,
    }
}

fn spec(workspace: Option<u128>, ceiling: RoleCeiling) -> GrantSpec {
    GrantSpec {
        org_id: id(ORG),
        workspace_id: workspace.map(id),
        ceiling,
    }
}

#[test]
fn the_wanted_set_replaces_the_live_one() {
    let existing = [
        workspace_row(1, Some(WS_A), "admin"),
        workspace_row(2, Some(WS_B), "viewer"),
    ];
    // A stays as it is, B goes, the org-wide grant is new.
    let plan = replace(
        &existing,
        &[
            want(Some(WS_A), RoleCeiling::Admin),
            want(None, RoleCeiling::Member),
        ],
    )
    .unwrap();
    assert_eq!(plan.kept, vec![id(1)]);
    assert_eq!(plan.delete, vec![id(2)]);
    assert_eq!(plan.insert, vec![spec(None, RoleCeiling::Member)]);
    assert_eq!(plan.live_after(), 2);
}

#[test]
fn sending_back_what_is_held_changes_nothing() {
    let existing = [
        workspace_row(1, Some(WS_A), "admin"),
        workspace_row(2, None, "owner"),
        app_row(3, APP),
    ];
    let plan = replace(
        &existing,
        &[
            want(None, RoleCeiling::Owner),
            GrantWant::AppPublish { app_id: id(APP) },
            want(Some(WS_A), RoleCeiling::Admin),
        ],
    )
    .unwrap();
    assert!(plan.changes_nothing(), "{plan:?}");
    assert_eq!(plan.live_after(), 3);
}

#[test]
fn a_changed_ceiling_replaces_the_row() {
    let existing = [workspace_row(1, Some(WS_A), "admin")];
    let plan = replace(&existing, &[want(Some(WS_A), RoleCeiling::Viewer)]).unwrap();
    assert_eq!(plan.delete, vec![id(1)]);
    assert_eq!(plan.insert, vec![spec(Some(WS_A), RoleCeiling::Viewer)]);
    assert!(plan.kept.is_empty());
}

#[test]
fn a_grant_its_org_revoked_is_neither_lost_nor_resurrected() {
    let existing = [
        revoked(workspace_row(1, Some(WS_A), "admin")),
        workspace_row(2, Some(WS_B), "member"),
        revoked(app_row(3, APP)),
    ];
    // The owner asks for both revoked targets again, and keeps B.
    let plan = replace(
        &existing,
        &[
            want(Some(WS_A), RoleCeiling::Admin),
            want(Some(WS_B), RoleCeiling::Member),
            GrantWant::AppPublish { app_id: id(APP) },
        ],
    )
    .unwrap();
    assert!(plan.changes_nothing(), "{plan:?}");
    assert_eq!(plan.kept, vec![id(2)]);

    // Dropping everything else still never deletes a revoked row.
    let narrower = replace(&existing, &[want(None, RoleCeiling::Viewer)]).unwrap();
    assert_eq!(narrower.delete, vec![id(2)]);
    assert_eq!(narrower.insert, vec![spec(None, RoleCeiling::Viewer)]);

    // And neither does going all-access.
    assert_eq!(clear(&existing).delete, vec![id(2)]);
}

#[test]
fn a_revoked_grant_blocks_only_its_own_target() {
    let existing = [revoked(workspace_row(1, Some(WS_A), "admin"))];
    let plan = replace(&existing, &[want(Some(WS_B), RoleCeiling::Admin)]).unwrap();
    assert_eq!(plan.insert, vec![spec(Some(WS_B), RoleCeiling::Admin)]);
    assert!(plan.delete.is_empty());
}

#[test]
fn an_app_publish_grant_is_kept_or_dropped_never_added() {
    let existing = [workspace_row(1, None, "owner"), app_row(2, APP)];

    // Left out of the set, it goes with everything else that was left out.
    let dropped = replace(&existing, &[want(None, RoleCeiling::Owner)]).unwrap();
    assert_eq!(dropped.delete, vec![id(2)]);
    assert_eq!(dropped.kept, vec![id(1)]);

    // One the token does not hold cannot be asked for here.
    let other = id(51);
    assert_eq!(
        replace(&existing, &[GrantWant::AppPublish { app_id: other }]),
        Err(NotHeld { app_id: other })
    );
    assert_eq!(
        replace(&[], &[GrantWant::AppPublish { app_id: id(APP) }]),
        Err(NotHeld { app_id: id(APP) })
    );
}

#[test]
fn a_row_of_an_unknown_kind_is_replaced_away() {
    let existing = [row(1, "something_newer"), workspace_row(2, None, "owner")];
    let plan = replace(&existing, &[want(None, RoleCeiling::Owner)]).unwrap();
    assert_eq!(plan.delete, vec![id(1)]);
    assert_eq!(plan.kept, vec![id(2)]);
}

#[test]
fn clearing_drops_every_live_grant() {
    let existing = [
        workspace_row(1, Some(WS_A), "admin"),
        app_row(2, APP),
        revoked(workspace_row(3, Some(WS_B), "admin")),
    ];
    let plan = clear(&existing);
    assert_eq!(plan.delete, vec![id(1), id(2)]);
    assert!(plan.insert.is_empty() && plan.kept.is_empty());
    assert_eq!(plan.live_after(), 0);
    assert!(clear(&[]).changes_nothing());
}

#[test]
fn an_org_wide_block_bars_every_target_in_that_org() {
    // The org ended the token's reach into it (the inventory's revoke-grant):
    // no workspace in it, and not the org itself, is granted again.
    let block = revoked(workspace_row(1, None, "owner"));
    let wanted = [
        GrantWant::Workspace {
            org_id: id(ORG),
            workspace_id: Some(id(WS_A)),
            ceiling: RoleCeiling::Viewer,
        },
        GrantWant::Workspace {
            org_id: id(ORG),
            workspace_id: None,
            ceiling: RoleCeiling::Member,
        },
        GrantWant::Workspace {
            org_id: id(2),
            workspace_id: None,
            ceiling: RoleCeiling::Member,
        },
    ];
    let plan = replace(&[block], &wanted).unwrap();
    assert_eq!(plan.insert.len(), 1, "only the other org is granted");
    assert_eq!(plan.insert[0].org_id, id(2));
    assert!(plan.delete.is_empty(), "the block row itself stays");
}
