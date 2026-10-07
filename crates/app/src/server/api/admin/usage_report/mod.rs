//! The weekly custom-app usage report: how each organization used its custom
//! apps last week, what changed, and which apps are worth a look.
//!
//! One report a week covers every org. It is stored, emailed to the staff who
//! want it, and readable in the console — where, as in the mail, each reader
//! sees the orgs their grant reaches and no others.
//!
//! - [`period`] names the week; [`collect`] reads it out of Postgres into a
//!   [`model::Snapshot`]; [`store`] keeps it.
//! - [`highlights`] is the judgement — what counts as quiet, falling, failing,
//!   growing — and is pure, so it is where the rules are tested.
//! - [`job`] is the weekly pass; [`delivery`] decides who is mailed and sends;
//!   [`email`] writes the mail.
//! - [`view`] is the console's read of a report, and `handlers` the routes;
//!   [`recipients`] lists who gets it and lets an admin switch one person off.
//!
//! Operator notes: `internal-docs/custom-app-usage-report.md`.

pub mod collect;
pub mod delivery;
pub mod email;
mod handlers;
pub mod highlights;
pub mod job;
pub mod model;
pub mod period;
pub mod recipients;
pub mod store;
pub mod view;

use axum::Router;
use axum::routing::{get, post};

use crate::server::router::AppState;

/// Mounted under `/api/admin`, behind `Action::PlatformOperate` — the same
/// capability [`delivery::AUDIENCE`] mails.
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/usage-report", get(handlers::latest_report))
        .route(
            "/usage-report/email-preference",
            get(handlers::email_preference).put(handlers::set_email_preference),
        )
        .route("/usage-report/send-to-me", post(handlers::send_to_me))
}
