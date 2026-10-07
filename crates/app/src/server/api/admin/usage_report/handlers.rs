//! The usage report's console endpoints. Transport only: each one resolves the
//! caller, calls into the module, and serializes.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AuthenticatedUser;
use oxy_authz::Scope;
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};

use crate::emails::usage_report::{DeliveryMode, delivery_mode};
use crate::server::api::admin::internal_jobs::{connect, db_err, error_body};
use crate::server::api::admin::scope;

use super::delivery::{self, CopyError};
use super::store;
use super::view::ReportView;

#[derive(Serialize)]
pub struct LatestReport {
    /// `null` until the first report has been written.
    report: Option<ReportView>,
}

#[derive(Serialize)]
pub struct EmailPreference {
    /// The caller's own address — where the report goes.
    email: String,
    enabled: bool,
    /// How this deployment delivers: `email`, `preview` (browser only), `off`.
    delivery: DeliveryMode,
}

#[derive(Deserialize)]
pub struct SetEmailPreference {
    enabled: bool,
}

#[derive(Serialize)]
pub struct CopySent {
    /// `sent`, or `previewed` where this deployment shows mail in the browser.
    outcome: &'static str,
    to: String,
}

/// The newest weekly custom-app usage report: totals, highlights and per-org
/// counts for the last completed week. A grant bounded to some orgs sees those
/// orgs only, with totals and highlights worked out for them. Pure Postgres
/// read of a stored row (FleetOk).
pub async fn latest_report(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
) -> Result<Json<LatestReport>, Response> {
    let db = connect().await?;
    let scope = reader_scope(&db, &actor).await?;
    let report = store::latest(&db).await.map_err(db_err)?;
    Ok(Json(LatestReport {
        report: report.map(|r| ReportView::of(&r, &scope)),
    }))
}

/// Whether the caller is emailed the weekly usage report, and how this
/// deployment delivers it. On unless they turned it off.
pub async fn email_preference(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
) -> Result<Json<EmailPreference>, Response> {
    let db = connect().await?;
    preference_of(&db, &actor).await.map(Json)
}

/// Turn the caller's own weekly usage report email on or off. Nobody's
/// preference but the caller's can be set here.
pub async fn set_email_preference(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Json(body): Json<SetEmailPreference>,
) -> Result<Json<EmailPreference>, Response> {
    let db = connect().await?;
    let own = address(&actor)?;
    store::set_wants_email(&db, own, body.enabled, own)
        .await
        .map_err(db_err)?;
    preference_of(&db, &actor).await.map(Json)
}

/// Email the newest usage report to the caller now — a copy on request,
/// whatever their weekly preference says. Previews in the browser instead
/// where the deployment does not send mail.
pub async fn send_to_me(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
) -> Result<Json<CopySent>, Response> {
    let db = connect().await?;
    let to = address(&actor)?.to_string();
    let scope = reader_scope(&db, &actor).await?;
    let console = oxy_app_core::custom_apps_host_dispatch::admin_base_url();
    let mode = delivery::send_copy(&db, &to, &scope, console.as_deref())
        .await
        .map_err(copy_error)?;
    let outcome = match mode {
        DeliveryMode::Preview => "previewed",
        DeliveryMode::Email | DeliveryMode::Off => "sent",
    };
    Ok(Json(CopySent { outcome, to }))
}

async fn preference_of(
    db: &DatabaseConnection,
    actor: &AuthenticatedUser,
) -> Result<EmailPreference, Response> {
    let email = address(actor)?;
    Ok(EmailPreference {
        enabled: store::wants_email(db, email).await.map_err(db_err)?,
        email: email.to_string(),
        delivery: delivery_mode(),
    })
}

/// Where the caller's grant reaches, read through the console's one scope
/// door. An unreadable grant is a 500 there, never "sees everything".
async fn reader_scope(
    db: &DatabaseConnection,
    actor: &AuthenticatedUser,
) -> Result<Scope, Response> {
    let reach = scope::list_scope(db, actor)
        .await
        .map_err(IntoResponse::into_response)?;
    Ok(match reach {
        None => Scope::All,
        Some(orgs) => Scope::Orgs(orgs),
    })
}

fn address(actor: &AuthenticatedUser) -> Result<&str, Response> {
    actor
        .email
        .as_deref()
        .filter(|e| !e.trim().is_empty())
        .ok_or_else(|| {
            error_body(
                StatusCode::BAD_REQUEST,
                "no_email",
                Some("This account has no email address.".into()),
            )
        })
}

fn copy_error(error: CopyError) -> Response {
    let (status, code) = match &error {
        CopyError::NoReport => (StatusCode::NOT_FOUND, "no_report"),
        CopyError::NoSender => (StatusCode::SERVICE_UNAVAILABLE, "email_not_configured"),
        CopyError::Send(_) => (StatusCode::BAD_GATEWAY, "send_failed"),
        CopyError::Db(e) => {
            tracing::error!(error = %e, "usage report: sending a copy failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "db_error")
        }
    };
    error_body(status, code, Some(error.to_string()))
}
