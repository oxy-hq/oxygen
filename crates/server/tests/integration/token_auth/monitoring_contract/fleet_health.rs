//! `GET /api/customer-apps/fleet-health` — the verdict ferret's `app-fleet`
//! check reads instead of computing its own.
//!
//! **ferret depends on this; changing it needs a matching change in
//! oxy-hq/infrastructure `services/ferret`** (`fleetRow` / `fleetPage` in
//! `src/uptime/staff-client.ts`, `checkAppFleet` and `FLEET_FAILING` in
//! `src/uptime/checks.ts`).
//!
//! ferret walks the pages with `limit=200&offset=page*200` until `has_more` is
//! false, finds each app's row by `org_slug` + `app_slug`, and fails the check
//! on `health` of `down` or `degraded` — any other value passes. So the
//! vocabulary is the contract as much as the shape: a `down` renamed to
//! anything else turns every outage green.

use oxy_observability::fleet_health::AppHealth;

use super::super::stack::published_app;
use super::{Field, assert_fields, ferret_get, query, staff_fixture, staff_keys};

const FLEET_ROW: &[(&str, Field)] = &[
    ("app_id", Field::Str),
    ("app_slug", Field::Str),
    ("org_slug", Field::Str),
    ("health", Field::Str),
    ("reason", Field::OptStr),
    ("requests", Field::Num),
    ("failed", Field::Num),
];

const FLEET_PAGE: &[(&str, Field)] = &[
    ("apps", Field::Array),
    ("has_more", Field::Bool),
    ("evaluated_at", Field::OptStr),
    ("observability_configured", Field::Bool),
];

/// What ferret does with each verdict: `true` fails the check. An exhaustive
/// match, so a new verdict does not compile until someone decides here — and
/// in ferret's `FLEET_FAILING` — whether it alerts.
fn ferret_fails_on(health: AppHealth) -> (&'static str, bool) {
    match health {
        AppHealth::Down => ("down", true),
        AppHealth::Degraded => ("degraded", true),
        AppHealth::NotMeasured => ("not_measured", false),
        AppHealth::Quiet => ("quiet", false),
        AppHealth::Operational => ("operational", false),
    }
}

#[test]
fn every_verdict_is_spelled_as_ferret_reads_it() {
    for health in [
        AppHealth::Down,
        AppHealth::Degraded,
        AppHealth::NotMeasured,
        AppHealth::Quiet,
        AppHealth::Operational,
    ] {
        let (wire, _) = ferret_fails_on(health);
        assert_eq!(
            serde_json::to_value(health).unwrap(),
            wire,
            "fleet-health's {health:?} is the word ferret matches on; changing it needs a \
             matching change in ferret's FLEET_FAILING"
        );
    }
}

#[tokio::test]
async fn the_fleet_answers_the_page_ferret_parses() {
    let fx = staff_fixture().await;
    let app = published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    let org_slug = format!("acme-{}", fx.org_id.simple());

    for (shape, key) in staff_keys(&fx).await {
        let uri = format!(
            "/customer-apps/fleet-health?{}",
            query(&[("limit", "200"), ("offset", "0")])
        );
        let page = ferret_get(Some(&key), &uri).await;
        page.assert_json_200(&format!("fleet-health, {shape}"));
        assert_fields(&page.body, FLEET_PAGE, "the fleet-health page");
        assert_eq!(page.body["has_more"], false, "one app, one page");

        // ferret's lookup is by slugs, not by id.
        let rows = page.body["apps"].as_array().unwrap();
        let row = rows
            .iter()
            .find(|r| r["org_slug"] == org_slug.as_str() && r["app_slug"] == app.slug.as_str())
            .unwrap_or_else(|| panic!("no row for {org_slug}/{}: {}", app.slug, page.body));
        assert_fields(row, FLEET_ROW, "a fleet-health row");
        // No observability backend in the test process: unmeasured, and said so.
        assert_eq!(row["health"], "not_measured");
        assert_eq!(page.body["observability_configured"], false);
    }
}

#[tokio::test]
async fn has_more_is_true_until_the_last_page() {
    let fx = staff_fixture().await;
    published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    let (_, key) = staff_keys(&fx).await.remove(1);

    let page = |offset: &'static str| {
        let uri = format!(
            "/customer-apps/fleet-health?{}",
            query(&[("limit", "1"), ("offset", offset)])
        );
        let key = key.clone();
        async move { ferret_get(Some(&key), &uri).await }
    };
    let first = page("0").await;
    first.assert_json_200("the first fleet page");
    assert_eq!(first.body["apps"].as_array().unwrap().len(), 1);
    assert_eq!(first.body["has_more"], true, "a second app is left");
    let last = page("1").await;
    last.assert_json_200("the last fleet page");
    assert_eq!(last.body["apps"].as_array().unwrap().len(), 1);
    assert_eq!(last.body["has_more"], false, "ferret stops here");
}
