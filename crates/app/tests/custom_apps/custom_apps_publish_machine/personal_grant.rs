//! `app_publish` on a **personal** token (API-tokens design §3.2): what the
//! combination does today, driven through the real `publish`.
//!
//! No route makes one — `POST` and `PATCH /api/user/tokens` answer 400
//! (`token_auth/personal_app_publish.rs` in `oxy-server`) — so the grant here
//! is a row written by hand, the only way one can exist. Admission accepts
//! such a row, and from there the grant **only narrows**:
//!
//! * alone it publishes nothing, its own app included: a person's token is
//!   authorized by the person's standing in the org, seen through the token,
//!   and a token with no workspace grant reaches no org;
//! * beside a workspace grant it holds the person to the app it names — the
//!   same token without the grant publishes every app the person may;
//! * it never lifts a ceiling: below admin the person may not publish, and
//!   the grant does not change that.
//!
//! A service account's token is the other half (`super`): there the grant
//! itself authorizes, because an admin of the app's own org put it there.

use entity::api_token_grants;
use oxy_auth::authenticator::Authenticator;
use oxy_auth::token::credential::source;
use oxy_auth::token::personal::{self, GrantSpec, NewToken};
use oxy_authz::RoleCeiling;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection};
use uuid::Uuid;

use super::{Tenant, human_first_publish, input_as, seed_tenant};
use crate::common::test_db;
use oxy_app::server::api::custom_apps_publish::{PublishError, Publisher, publish};

/// The org admin's own personal token, as a publisher: an org-wide workspace
/// grant at `ceiling` when one is given, and an `app_publish` grant on each
/// of `apps` — written straight to `api_token_grants`, since no route will.
async fn personal_publisher(
    db: &DatabaseConnection,
    t: &Tenant,
    ceiling: Option<RoleCeiling>,
    apps: &[Uuid],
) -> Publisher {
    oxy_auth::built_in::set_auth_configured(true);
    let grants = ceiling
        .map(|ceiling| GrantSpec {
            org_id: t.org_id,
            workspace_id: None,
            ceiling,
        })
        .into_iter()
        .collect();
    let minted = personal::create(
        db,
        NewToken {
            user_id: t.admin.id,
            name: "by hand".into(),
            all_access: false,
            platform: false,
            partner: false,
            grants,
            expires_at: None,
            source: source::UI,
        },
    )
    .await
    .expect("mint the personal token");
    for app_id in apps {
        api_token_grants::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            token_id: ActiveValue::Set(minted.row.id),
            kind: ActiveValue::Set(api_token_grants::KIND_APP_PUBLISH.to_string()),
            org_id: ActiveValue::Set(t.org_id),
            workspace_id: ActiveValue::Set(None),
            role_ceiling: ActiveValue::Set(None),
            app_id: ActiveValue::Set(Some(*app_id)),
            created_at: ActiveValue::Set(chrono::Utc::now().fixed_offset()),
            revoked_at: ActiveValue::Set(None),
            revoked_by: ActiveValue::Set(None),
        }
        .insert(db)
        .await
        .expect("write the app_publish grant by hand");
    }
    oxy_auth::token::cache::clear();

    let mut headers = axum::http::HeaderMap::new();
    let bearer = format!("Bearer {}", minted.secret);
    headers.insert("authorization", bearer.parse().unwrap());
    let (_identity, credential) =
        oxy_auth::built_in::BuiltInAuthenticator::new(oxy_auth::token::SandboxAgent::Refuse)
            .authenticate_with_credential(&headers)
            .await
            .expect("a personal token holding an app_publish grant is admitted");
    let user = t.admin.clone().with_credential(credential);
    Publisher::from_request(&user, None)
}

fn denied(outcome: Result<impl std::fmt::Debug, PublishError>, why: &str) {
    match outcome {
        Err(PublishError::OxyAccessDenied { .. }) => {}
        other => panic!("{why}: expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn alone_the_grant_publishes_nothing_even_its_own_app() {
    let db = test_db().await;
    let t = seed_tenant(&db).await;
    let granted = human_first_publish(&db, &t, "granted-app").await;
    human_first_publish(&db, &t, "other-app").await;

    let who = personal_publisher(&db, &t, None, &[granted]).await;
    let caller = who.caller.clone().expect("a person's publish has a caller");
    assert!(caller.holds_app_publish());
    assert!(caller.publishes_app(t.org_id, granted));
    assert_eq!(
        caller.org_reach_ceiling(t.org_id),
        None,
        "an app_publish grant is not reach into the app's org"
    );

    // The org's own admin, publishing the very app the grant names.
    denied(
        publish(input_as(&t, "granted-app", "pat-alone-1", who.clone())).await,
        "the grant alone",
    );
    denied(
        publish(input_as(&t, "other-app", "pat-alone-2", who)).await,
        "another app",
    );
}

#[tokio::test]
async fn beside_a_workspace_grant_it_holds_the_person_to_the_app_it_names() {
    let db = test_db().await;
    let t = seed_tenant(&db).await;
    let granted = human_first_publish(&db, &t, "granted-app").await;
    let other = human_first_publish(&db, &t, "other-app").await;

    // Without the grant, the admin's org-wide token publishes either app.
    let unconfined = personal_publisher(&db, &t, Some(RoleCeiling::Admin), &[]).await;
    let result = publish(input_as(&t, "other-app", "pat-wide-1", unconfined))
        .await
        .expect("an admin's org-wide token publishes any app of the org");
    assert_eq!(result.app_id, other);

    // With it: the app it names, and no other.
    let who = personal_publisher(&db, &t, Some(RoleCeiling::Admin), &[granted]).await;
    let result = publish(input_as(&t, "granted-app", "pat-grant-1", who.clone()))
        .await
        .expect("the person may publish, and the grant names this app");
    assert_eq!(result.app_id, granted);
    denied(
        publish(input_as(&t, "other-app", "pat-grant-2", who.clone())).await,
        "an app the grant does not name",
    );
    denied(
        publish(input_as(&t, "brand-new-app", "pat-grant-3", who)).await,
        "an app that does not exist yet",
    );
}

#[tokio::test]
async fn the_grant_lifts_no_ceiling() {
    let db = test_db().await;
    let t = seed_tenant(&db).await;
    let granted = human_first_publish(&db, &t, "granted-app").await;

    // An admin whose token is capped at member is a member through it, and a
    // member does not publish. The grant authorizes nothing by itself.
    let who = personal_publisher(&db, &t, Some(RoleCeiling::Member), &[granted]).await;
    denied(
        publish(input_as(&t, "granted-app", "pat-member-1", who)).await,
        "a member-ceiling token",
    );
}
