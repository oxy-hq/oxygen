//! The weekly custom-app usage report on the flat tree: `oxy-api-billing`'s two
//! admin sections since admin split PR 8b, which `flat_api_surface` mounted
//! in-tree before. A staff session reads the report and its recipients; with
//! either section left out of [`super::stack::flat_api`], its path is the
//! outer router's 404.

use axum::http::StatusCode;

use super::fixture;
use super::stack::{get_in_session, make_staff};

#[tokio::test]
async fn a_staff_session_reaches_both_usage_report_sections() {
    let fx = fixture().await;
    make_staff(&fx.db, fx.user.email.as_deref().unwrap()).await;

    let (status, body) = get_in_session(&fx, "/admin/usage-report").await;
    assert_eq!(status, StatusCode::OK, "the report section: {body}");
    assert!(body["report"].is_null(), "no report written yet: {body}");

    let (status, body) = get_in_session(&fx, "/admin/usage-report/recipients").await;
    assert_eq!(status, StatusCode::OK, "the recipients section: {body}");
    assert!(body["recipients"].is_array(), "{body}");
}
