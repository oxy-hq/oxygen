//! `/admin/assume/history` — the impersonation log, which names the tenant
//! that was entered. (`GET /admin/audit` moved with its handlers to
//! `oxy-api-admin`'s `admin_staff_scope::audit`, admin split 2026-11-20; this
//! file was `audit_and_assume.rs`, and `audit_and_explorer.rs` before that.)

use axum::extract::{OriginalUri, Query};
use axum::http::{StatusCode, Uri};
use chrono::{Duration, Utc};
use entity::admin_assume_sessions;
use oxy_app::server::api::admin::assume::{HistoryQuery, history};
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection};
use uuid::Uuid;

use super::fixture::{as_actor, reply, world};

fn uri(path: &'static str) -> OriginalUri {
    OriginalUri(Uri::from_static(path))
}

async fn assume_session(db: &DatabaseConnection, actor: &AuthenticatedUser, org: Uuid) -> String {
    let now = Utc::now().fixed_offset();
    let id = Uuid::new_v4();
    admin_assume_sessions::ActiveModel {
        id: ActiveValue::Set(id),
        actor_user_id: ActiveValue::Set(actor.id),
        actor_email: ActiveValue::Set(actor.email.clone().unwrap_or_default()),
        org_id: ActiveValue::Set(org),
        reason: ActiveValue::Set("staff-scope probe".into()),
        started_at: ActiveValue::Set(now - Duration::minutes(90)),
        expires_at: ActiveValue::Set(now - Duration::minutes(30)),
        ended_at: ActiveValue::Set(Some(now - Duration::minutes(60))),
        token_id: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed assume session");
    id.to_string()
}

/// The impersonation log names the org that was entered. Found by the audit of the
/// rest of the console, not by the review.
#[tokio::test]
async fn a_bounded_grant_reads_only_assume_sessions_into_its_own_orgs() {
    let w = world().await;
    let mine = assume_session(&w.db, &w.unbounded, w.org_a).await;
    let theirs = assume_session(&w.db, &w.unbounded, w.org_b).await;
    let query = || Query(HistoryQuery::default());

    let listed = reply(
        history(
            as_actor(&w.bounded),
            uri("/api/admin/assume/history"),
            query(),
        )
        .await,
    )
    .await;
    assert_eq!(listed.status, StatusCode::OK);
    assert_eq!(
        listed.column(None, "id"),
        vec![mine.clone()],
        "a grant bounded to org A read who impersonated org B"
    );

    for (who, actor) in w.everything_readers() {
        let ids = reply(history(as_actor(actor), uri("/api/admin/assume/history"), query()).await)
            .await
            .column(None, "id");
        assert!(ids.contains(&mine) && ids.contains(&theirs), "{who}");
    }
}
