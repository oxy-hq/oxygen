//! The Compliance review queue.
//!
//! One route for all three transitions — submit, approve, reject — because they
//! share every field and differ only in which one they set. Three endpoints
//! would be three places to forget that a decision carries a timestamp and a
//! submission does not, and the schema constraint that enforces it would then be
//! discovered by a `500` rather than honoured by design.
//!
//! # What this deliberately does not do
//!
//! **Four-eyes review.** Every write here is behind `OrgAdmin`, so the officer
//! who submits a document can approve it. A real separation of duties needs a
//! second authority to exist first — a reviewer who is not an admin — and
//! inventing one here would put an authority model inside a Compliance feature.
//! The queue is a workflow, not a control.
//!
//! **Clearing a review.** There is no transition back to "nobody is asking". A
//! document that was approved and is now unreviewed has lost the record that it
//! ever was, and that is exactly the record Compliance exists to keep.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use chrono::Utc;
use entity::documents;
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{ActiveModelTrait, Set};
use tracing::{info, instrument};
use uuid::Uuid;

use super::dto::{DocumentSummary, ReviewDecision};
use super::handlers::db_err;
use super::hydrate;
use super::manage::owned_document;
use crate::server::api::middlewares::role_guards::OrgAdmin;

/// Undecided, or decided. The only place that mapping is written down.
fn is_decision(status: &str) -> Option<bool> {
    match status {
        "in_review" => Some(false),
        "approved" | "rejected" => Some(true),
        _ => None,
    }
}

/// `POST /api/orgs/{org_id}/documents/{id}/review`
#[instrument(skip_all, fields(org = %org_id, document = %id, status = %body.status))]
pub async fn decide(
    OrgAdmin(_ctx): OrgAdmin,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path((org_id, id)): Path<(Uuid, Uuid)>,
    Json(body): Json<ReviewDecision>,
) -> Result<Json<DocumentSummary>, StatusCode> {
    let Some(decided) = is_decision(&body.status) else {
        return Err(StatusCode::BAD_REQUEST);
    };

    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let existing = owned_document(&db, org_id, id).await?;

    let now = Utc::now().fixed_offset();
    let mut am: documents::ActiveModel = existing.into();
    am.review_status = Set(Some(body.status.clone()));
    // The timestamp and the reviewer move together with the decision, and both
    // are cleared on a resubmission. `documents_review_decision_is_whole` would
    // reject the mismatch anyway; setting them here means the caller never sees
    // it do so.
    am.reviewed_at = Set(decided.then_some(now));
    am.reviewed_by = Set(decided.then_some(user.id));
    am.review_note = Set(body.note.clone());
    am.updated_at = Set(now);

    let saved = am.update(&db).await.map_err(db_err)?;
    info!(status = %body.status, "document review decided");
    // Hydrated, for the reason `update_document` is: the bare conversion has no
    // caller to ask, so it ships `is_favorite: false` and null names. Approving
    // a document from the Compliance table came back with its category tab
    // label, its version badge, its file-type badge and its star all blanked,
    // and a screen that trusts the response redraws them that way.
    let names = hydrate::names_for(&db, user.id, std::slice::from_ref(&saved))
        .await
        .map_err(db_err)?;
    Ok(Json(hydrate::summary(saved, &names)))
}
