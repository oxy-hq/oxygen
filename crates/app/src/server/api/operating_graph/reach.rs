//! Reach: the places a caller may act on, derived from their assignments.
//!
//! The one rule, settled 2026-09-07 (`internal-docs/operating-graph.md` §3.3),
//! decided in order:
//!
//! 1. A system invocation reaches everywhere — it runs as the org owner.
//! 2. An app admin reaches everywhere.
//! 3. Somebody holding an org-wide position (Area Manager, Corporate) reaches
//!    everywhere, and still carries the places they are also assigned at.
//! 4. Somebody assigned somewhere reaches exactly those places.
//! 5. Unassigned: an org member reaches everywhere — an office user nobody
//!    has rostered is the account owner setting the app up — and a frontline
//!    worker reaches nowhere, because "nowhere" is the honest answer and a
//!    roster row is one click away in Settings.
//!
//! Facts and filters, not a ring: the platform states the reach, an app
//! applies it in its own WHERE (`@oxy-hq/sdk/ops`), and may tighten it but
//! never widen it. Computed per invocation from one bounded read — a person
//! holds a handful of positions — rather than loaded into `PrincipalFacts`,
//! where no ring consumes it yet.

use entity::{org_role_members, org_roles};
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder};
use serde::Serialize;
use uuid::Uuid;

/// What `ctx.user.reach` carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Reach {
    pub everywhere: bool,
    /// Why everywhere, when it is; `None` when scoped or nowhere.
    pub via: Option<&'static str>,
    /// Where the caller is assigned, in id order. Kept for an everywhere
    /// caller too — a label ("your store") needs it — and empty for one
    /// assigned nowhere.
    pub locations: Vec<Uuid>,
}

impl Reach {
    pub fn everywhere(via: &'static str, locations: Vec<Uuid>) -> Self {
        Self {
            everywhere: true,
            via: Some(via),
            locations,
        }
    }

    /// The fail-closed answer: a lookup that errored must not widen anything.
    pub fn nowhere() -> Self {
        Self {
            everywhere: false,
            via: None,
            locations: Vec::new(),
        }
    }

    pub fn reaches(&self, location: Uuid) -> bool {
        self.everywhere || self.locations.contains(&location)
    }
}

/// The shape of the caller, as the host already knows it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Caller {
    pub system: bool,
    pub app_admin: bool,
    pub member: bool,
}

/// One held position: where (`None` = org-wide) and whether it is org-wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Held {
    pub location_id: Option<Uuid>,
    pub org_wide: bool,
}

/// The decision, pure. `held` is the caller's positions in any order.
pub fn reach_of(caller: Caller, held: &[Held]) -> Reach {
    if caller.system {
        return Reach::everywhere("system", Vec::new());
    }
    let mut locations: Vec<Uuid> = held.iter().filter_map(|h| h.location_id).collect();
    locations.sort_unstable();
    locations.dedup();
    if caller.app_admin {
        return Reach::everywhere("app-admin", locations);
    }
    if held.iter().any(|h| h.org_wide) {
        return Reach::everywhere("org-wide-position", locations);
    }
    if !locations.is_empty() {
        return Reach {
            everywhere: false,
            via: None,
            locations,
        };
    }
    if caller.member {
        return Reach::everywhere("org-member", Vec::new());
    }
    Reach::nowhere()
}

/// The caller's positions in this org, as the decision wants them. One
/// bounded read; the join to `org_roles` is what says whether a position is
/// org-wide, and a position whose role row is gone counts as nothing.
pub async fn held_by(
    db: &DatabaseConnection,
    org_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<Held>, DbErr> {
    let rows = org_role_members::Entity::find()
        .filter(org_role_members::Column::OrgId.eq(org_id))
        .filter(org_role_members::Column::UserId.eq(user_id))
        .order_by_asc(org_role_members::Column::LocationId)
        .all(db)
        .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let role_ids: Vec<Uuid> = rows.iter().map(|r| r.role_id).collect();
    let roles = org_roles::Entity::find()
        .filter(org_roles::Column::Id.is_in(role_ids))
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let role = roles.iter().find(|x| x.id == r.role_id)?;
            Some(Held {
                location_id: r.location_id,
                org_wide: !role.is_location_scoped(),
            })
        })
        .collect())
}

/// Reach for a real caller: the positions read, then the rule.
pub async fn load_reach(
    db: &DatabaseConnection,
    org_id: Uuid,
    user_id: Uuid,
    caller: Caller,
) -> Result<Reach, DbErr> {
    if caller.system {
        return Ok(reach_of(caller, &[]));
    }
    let held = held_by(db, org_id, user_id).await?;
    Ok(reach_of(caller, &held))
}

/// A signed-in viewer's reach on a customer-app surface: the same rule as
/// `ctx.user.reach`, with app-admin standing resolved only when the surface
/// names its app. Every lookup fails closed to nowhere — a blip must not
/// widen what a screen shows or a query returns.
pub async fn reach_for_viewer(
    db: &DatabaseConnection,
    org_id: Uuid,
    user: &oxy_auth::types::AuthenticatedUser,
    project_id: Uuid,
    app_id: Option<Uuid>,
) -> Reach {
    // Three independent facts, read together: this sits on the app-bootstrap
    // path, which is per-viewer and uncached, so a serial chain here is paid
    // on every app load.
    let (app_admin, member, held) = tokio::join!(
        async {
            let Some(id) = app_id else { return false };
            let app = entity::apps::Entity::find_by_id(id)
                .filter(entity::apps::Column::ProjectId.eq(project_id))
                .one(db)
                .await
                .ok()
                .flatten();
            match app {
                Some(app) => {
                    // The user WITH their credential: an API token's grants and
                    // standing flags decide app-admin here as everywhere else.
                    crate::server::api::custom_apps_auth::resolve_app_role(
                        db,
                        &oxy_server_authz::Caller::from_user(user),
                        &app,
                    )
                    .await
                    .ok()
                    .flatten()
                        == Some("admin")
                }
                None => false,
            }
        },
        async {
            crate::server::api::custom_apps_auth::resolve_org_role(db, user.id, org_id)
                .await
                .map(|role| role.is_some())
                .unwrap_or(false)
        },
        held_by(db, org_id, user.id),
    );
    match held {
        Ok(held) => reach_of(
            Caller {
                system: false,
                app_admin,
                member,
            },
            &held,
        ),
        Err(e) => {
            tracing::error!("reach lookup failed: {e}");
            Reach::nowhere()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(id: u128) -> Held {
        Held {
            location_id: Some(Uuid::from_u128(id)),
            org_wide: false,
        }
    }
    const ORG_WIDE: Held = Held {
        location_id: None,
        org_wide: true,
    };
    const MEMBER: Caller = Caller {
        system: false,
        app_admin: false,
        member: true,
    };
    const WORKER: Caller = Caller {
        system: false,
        app_admin: false,
        member: false,
    };

    #[test]
    fn a_system_invocation_reaches_everywhere_before_anything_else() {
        // A tick runs as the org owner and would read as an app admin; the
        // reason recorded must be the tick, or an audit names a person.
        let caller = Caller {
            system: true,
            app_admin: true,
            member: true,
        };
        assert_eq!(
            reach_of(caller, &[at(1)]),
            Reach::everywhere("system", vec![])
        );
    }

    #[test]
    fn an_app_admin_reaches_everywhere_and_keeps_their_own_places() {
        let caller = Caller {
            app_admin: true,
            ..MEMBER
        };
        let r = reach_of(caller, &[at(2), at(1)]);
        assert_eq!(r.via, Some("app-admin"));
        assert!(r.everywhere);
        assert_eq!(r.locations, vec![Uuid::from_u128(1), Uuid::from_u128(2)]);
    }

    #[test]
    fn an_assigned_person_reaches_exactly_their_places_in_id_order() {
        for caller in [MEMBER, WORKER] {
            let r = reach_of(caller, &[at(2), at(1), at(2)]);
            assert!(!r.everywhere);
            assert_eq!(r.via, None);
            assert_eq!(r.locations, vec![Uuid::from_u128(1), Uuid::from_u128(2)]);
            assert!(r.reaches(Uuid::from_u128(1)));
            assert!(!r.reaches(Uuid::from_u128(3)));
        }
    }

    #[test]
    fn one_org_wide_position_reaches_everywhere_and_still_names_the_places() {
        let r = reach_of(WORKER, &[at(1), ORG_WIDE]);
        assert_eq!(r.via, Some("org-wide-position"));
        assert!(r.everywhere);
        assert_eq!(r.locations, vec![Uuid::from_u128(1)]);
    }

    #[test]
    fn unassigned_a_member_reaches_everywhere_and_a_worker_nowhere() {
        assert_eq!(
            reach_of(MEMBER, &[]),
            Reach::everywhere("org-member", vec![])
        );
        assert_eq!(reach_of(WORKER, &[]), Reach::nowhere());
        assert!(!Reach::nowhere().reaches(Uuid::from_u128(1)));
    }

    #[test]
    fn reach_serialises_as_the_sdk_reads_it() {
        let json = serde_json::to_value(reach_of(WORKER, &[at(1)])).unwrap();
        assert_eq!(json["everywhere"], false);
        assert_eq!(json["via"], serde_json::Value::Null);
        assert_eq!(json["locations"][0], Uuid::from_u128(1).to_string());
        let json = serde_json::to_value(Reach::everywhere("org-member", vec![])).unwrap();
        assert_eq!(json["via"], "org-member");
    }
}
