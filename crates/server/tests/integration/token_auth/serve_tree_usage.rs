//! Usage on the two custom-app paths that authenticate inside their handlers,
//! and so sit outside `/api`'s usage layer: `/fn` and `/logs`.
//!
//! Every new-format API token is counted there as it is on `/api` — a
//! personal token and a service account's here; the sandbox agent token was
//! already. A session and a legacy key are not, exactly as before: the probe
//! reads the presented prefix and leaves their requests alone.

use axum::http::StatusCode;
use entity::api_token_usage_daily as usage_daily;
use oxy_auth::token::usage;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::json;
use uuid::Uuid;

use super::deactivated_owner::{api_key, app_fixture, bearer, fn_and_logs};
use super::service_accounts::{create_account, mint_account_token};
use super::{legacy_key, minted_pat};

/// `router/public.rs`'s template for the log read; the last route each token
/// called below.
const LOGS_ROUTE: &str = "/api/customer-apps/{org_slug}/{app_slug}/logs";

async fn usage_rows(db: &DatabaseConnection, token_id: Uuid) -> Vec<usage_daily::Model> {
    usage_daily::Entity::find()
        .filter(usage_daily::Column::TokenId.eq(token_id))
        .all(db)
        .await
        .expect("read usage rows")
}

#[tokio::test]
async fn a_new_format_token_is_counted_on_fn_and_logs_and_no_other_credential_is() {
    let (fx, paths) = app_fixture().await;
    let (pat_id, pat) = minted_pat(&fx, None).await;
    let account = create_account(&fx, "reader", "admin").await;
    let (sat_id, sat) = mint_account_token(&fx, account, json!({ "name": "t" })).await;
    let (key_id, key) = legacy_key(&fx, None).await;
    // Nothing the setup did is left to be flushed with what follows.
    usage::flush(&fx.db).await.expect("flush the setup");

    // One call to each path, per credential. The tokens authenticate.
    for token in [&pat, &sat] {
        for status in fn_and_logs(&paths, &bearer(token)).await {
            assert_ne!(status, StatusCode::UNAUTHORIZED, "a live token is admitted");
        }
    }
    fn_and_logs(&paths, &api_key(&key)).await;
    fn_and_logs(&paths, &("cookie", fx.cookie.clone())).await;
    // A token nobody minted presents the prefix and authenticates as no one.
    let unknown = oxy_auth::token::format::generate_personal().plaintext;
    assert_eq!(
        fn_and_logs(&paths, &bearer(&unknown)).await,
        [StatusCode::UNAUTHORIZED; 2]
    );

    assert_eq!(
        usage::flush(&fx.db).await.expect("flush"),
        2,
        "one row per token that authenticated, and no other"
    );
    for (what, token_id) in [("personal", pat_id), ("service_account", sat_id)] {
        let rows = usage_rows(&fx.db, token_id).await;
        assert_eq!(rows.len(), 1, "{what}");
        assert_eq!(rows[0].requests, 2, "{what}: /fn and /logs");
        assert_eq!(rows[0].last_route.as_deref(), Some(LOGS_ROUTE), "{what}");
    }
    assert!(
        usage_rows(&fx.db, key_id).await.is_empty(),
        "a legacy key on the serve tree is counted no more than it ever was"
    );
}
