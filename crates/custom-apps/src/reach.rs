//! A signed-in viewer's reach on a customer-app surface, resolved against the
//! app's own roles. `Reach` and the pure rule are `oxy_app_core::operating_graph::reach`;
//! only this function needs `auth::resolve_app_role` / `resolve_org_role`, which
//! sit here.

use entity::apps;
use oxy_app_core::operating_graph::reach::{Caller, Reach, held_by, reach_of};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::auth::{resolve_app_role, resolve_org_role};

/// A signed-in viewer's reach on a customer-app surface: the same rule as
/// `ctx.user.reach`, with app-admin standing resolved only when the surface
/// names its app. Every lookup fails closed to nowhere — a blip must not
/// widen what a screen shows or a query returns.
///
/// `viewer` is the request's caller, built by the surface from the signed-in
/// user WITH their credential (`oxy_server_authz::Caller::from_user(..)`,
/// `as_contract()`): its `user_id` is the user's id.
pub async fn reach_for_viewer(
    db: &DatabaseConnection,
    org_id: Uuid,
    viewer: &contracts_authz::Caller,
    project_id: Uuid,
    app_id: Option<Uuid>,
) -> Reach {
    // Three independent facts, read together: this sits on the app-bootstrap
    // path, which is per-viewer and uncached, so a serial chain here is paid
    // on every app load.
    let (app_admin, member, held) = tokio::join!(
        async {
            let Some(id) = app_id else { return false };
            let app = apps::Entity::find_by_id(id)
                .filter(apps::Column::ProjectId.eq(project_id))
                .one(db)
                .await
                .ok()
                .flatten();
            match app {
                Some(app) => {
                    // The user WITH their credential: an API token's grants and
                    // standing flags decide app-admin here as everywhere else.
                    resolve_app_role(db, viewer, &app).await.ok().flatten() == Some("admin")
                }
                None => false,
            }
        },
        async {
            resolve_org_role(db, viewer.user_id, org_id)
                .await
                .map(|role| role.is_some())
                .unwrap_or(false)
        },
        held_by(db, org_id, viewer.user_id),
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
