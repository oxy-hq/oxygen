//! The health probes' unit tests, moved with them from `oxy-app`'s
//! `server::api::healthcheck` (2026-11-16). That the served router mounts
//! them is `served_router_tests`'.

use axum::Json;

use crate::core_routes::healthcheck::version_info;

#[tokio::test]
async fn the_version_route_names_the_image_and_reports_no_semver() {
    let Json(body) = version_info().await;
    let v = serde_json::to_value(body).unwrap();
    assert!(v.get("version").is_none(), "no semver: {v}");
    assert!(
        v.as_object().unwrap().contains_key("image_tag"),
        "image_tag is always present, null outside an image: {v}"
    );
    assert!(v["build_info"]["git_commit_short"].is_string());
    assert!(v["build_info"]["sentry_release"].is_string());
}
