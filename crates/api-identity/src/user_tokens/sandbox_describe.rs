//! What a **sandbox agent token** is told about itself (`GET /api/auth/token`):
//! who minted it, the apps it may run the sandbox loop on, and for each
//! whether it was also granted the app's staging.
//!
//! Read from the grants the token holds **now**, the way admission reads
//! them: an app is the token's while its `app_sandbox` grant is live and the
//! app is still in the org the grant names, and its staging is the token's
//! while an `app_staging` grant beside it is live too.

use entity::prelude::Apps;
use entity::{api_token_grants, api_tokens, apps};
use oxy_app_core::audit::RequestActor;
use oxy_auth::token::sandbox as mint;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use serde::Serialize;
use uuid::Uuid;

use super::error::TokenError;
use super::sandbox::{option_of, orgs_of};

/// Who minted a sandbox agent token — the user every request on it acts as.
#[derive(Debug, PartialEq, Serialize)]
pub struct MinterDto {
    pub user_id: Uuid,
    pub email: Option<String>,
}

/// One app a sandbox agent token names, as the token itself is told.
#[derive(Debug, PartialEq, Serialize)]
pub struct GrantedAppDto {
    pub id: Uuid,
    pub org_slug: String,
    pub slug: String,
    pub name: String,
    /// Whether the token may also publish a draft to, and exercise, this
    /// app's staging environment. `false` for a token minted without it.
    pub staging: bool,
}

/// What `GET /api/auth/token` adds for a sandbox agent token: its minter and
/// the apps it may run the loop on. `oxyc` resolves `<org>/<app>` against this
/// list, since the token reaches no app directory.
#[derive(Debug, PartialEq, Serialize)]
pub struct SelfDescription {
    pub minter: MinterDto,
    pub apps: Vec<GrantedAppDto>,
}

/// `(app, org)` of each live grant of `kind`. An app that left the org its
/// grant names is not the token's, exactly as admission reads it.
fn live_of(grants: &[api_token_grants::Model], kind: &str) -> Vec<(Uuid, Uuid)> {
    grants
        .iter()
        .filter(|g| g.revoked_at.is_none() && g.kind == kind)
        .filter_map(|g| g.app_id.map(|app_id| (app_id, g.org_id)))
        .collect()
}

/// The token's own apps, by the grants it holds **now**: a grant that was
/// revoked, or whose app is gone, is not listed.
pub(super) async fn describe(
    db: &DatabaseConnection,
    actor: &RequestActor,
    row: &api_tokens::Model,
) -> Result<Option<SelfDescription>, TokenError> {
    if !mint::is_sandbox_agent(row) {
        return Ok(None);
    }
    let grants = oxy_auth::token::personal::grants_for(db, &[row.id]).await?;
    let live = live_of(&grants, api_token_grants::KIND_APP_SANDBOX);
    let staged = live_of(&grants, api_token_grants::KIND_APP_STAGING);
    let apps: Vec<apps::Model> = if live.is_empty() {
        Vec::new()
    } else {
        Apps::find()
            .filter(apps::Column::Id.is_in(live.iter().map(|(app_id, _)| *app_id)))
            .order_by_asc(apps::Column::Slug)
            .all(db)
            .await?
            .into_iter()
            .filter(|app| live.contains(&(app.id, app.org_id)))
            .collect()
    };
    let orgs = orgs_of(db, &apps).await?;
    let apps = apps
        .iter()
        .filter_map(|app| option_of(app, &orgs))
        .map(|app| GrantedAppDto {
            staging: staged.contains(&(app.id, app.org_id)),
            id: app.id,
            org_slug: app.org_slug,
            slug: app.slug,
            name: app.name,
        })
        .collect();
    Ok(Some(SelfDescription {
        minter: MinterDto {
            user_id: actor.id,
            email: actor.email.clone(),
        },
        apps,
    }))
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;

    const ORG: Uuid = Uuid::from_u128(0xA);
    const APP: Uuid = Uuid::from_u128(0xAA);
    const OTHER_APP: Uuid = Uuid::from_u128(0xAB);

    fn grant(kind: &str, app_id: Option<Uuid>, revoked: bool) -> api_token_grants::Model {
        let now = Utc::now().fixed_offset();
        api_token_grants::Model {
            id: Uuid::new_v4(),
            token_id: Uuid::from_u128(1),
            kind: kind.to_string(),
            org_id: ORG,
            workspace_id: None,
            role_ceiling: None,
            app_id,
            created_at: now,
            revoked_at: revoked.then_some(now),
            revoked_by: None,
        }
    }

    /// An app is listed by its `app_sandbox` grant and marked by its
    /// `app_staging` one: a staging row lists no app by itself, and a revoked
    /// one marks none.
    #[test]
    fn apps_come_from_the_sandbox_grants_and_staging_from_the_rows_beside_them() {
        let grants = [
            grant("app_sandbox", Some(APP), false),
            grant("app_staging", Some(APP), false),
            grant("app_sandbox", Some(OTHER_APP), false),
            grant("app_staging", Some(OTHER_APP), true),
            grant("app_sandbox", Some(Uuid::from_u128(0xAC)), true),
            grant("app_staging", None, false),
        ];
        assert_eq!(
            live_of(&grants, api_token_grants::KIND_APP_SANDBOX),
            vec![(APP, ORG), (OTHER_APP, ORG)]
        );
        assert_eq!(
            live_of(&grants, api_token_grants::KIND_APP_STAGING),
            vec![(APP, ORG)]
        );
    }

    #[test]
    fn an_app_says_whether_its_staging_is_the_tokens() {
        let app = GrantedAppDto {
            id: APP,
            org_slug: "acme".into(),
            slug: "store-ops".into(),
            name: "Store Ops".into(),
            staging: true,
        };
        let told = serde_json::to_value(&app).unwrap();
        assert_eq!(told["staging"], serde_json::json!(true));
        assert_eq!(told["slug"], serde_json::json!("store-ops"));
    }
}
