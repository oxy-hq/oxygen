//! `GET /api/customer-apps` — ferret's app discovery.
//!
//! **ferret depends on this; changing it needs a matching change in
//! oxy-hq/infrastructure `services/ferret`** (`appRow` / `appListPage` in
//! `src/uptime/staff-client.ts`, `checkedApps` in `src/uptime/discovery.ts`).
//!
//! Discovery walks the list hourly with `limit=200&offset=N` until
//! `next_offset` is null, keeps the apps whose `published_at` is set, and
//! builds every per-app check from `org_slug` + `slug`. A list that stops
//! parsing keeps the previous checks; a `published_at` that stops arriving
//! drops every app from every check.

use entity::apps;
use sea_orm::{ActiveModelTrait, ActiveValue, IntoActiveModel};
use serde_json::Value;

use super::super::stack::published_app;
use super::{Field, assert_fields, ferret_get, query, staff_fixture, staff_keys};

const APP_ROW: &[(&str, Field)] = &[
    ("id", Field::Str),
    ("slug", Field::Str),
    ("org_slug", Field::Str),
    ("url_subdomain", Field::OptStr),
    ("published_at", Field::OptStr),
];

/// The row for `app` in a list page.
fn row_of<'a>(page: &'a Value, app: &apps::Model) -> &'a Value {
    page["items"]
        .as_array()
        .expect("items is an array")
        .iter()
        .find(|item| item["id"] == app.id.to_string())
        .unwrap_or_else(|| panic!("app {} is not in the list: {page}", app.id))
}

#[tokio::test]
async fn the_list_answers_the_page_ferret_parses() {
    let fx = staff_fixture().await;
    let live = published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    let draft = published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    let mut unpublish = draft.clone().into_active_model();
    unpublish.published_at = ActiveValue::Set(None);
    let draft = unpublish.update(&fx.db).await.expect("unpublish the draft");
    let org_slug = format!("acme-{}", fx.org_id.simple());

    for (shape, key) in staff_keys(&fx).await {
        let page = ferret_get(
            Some(&key),
            &format!(
                "/customer-apps?{}",
                query(&[("limit", "200"), ("offset", "0")])
            ),
        )
        .await;
        page.assert_json_200(&format!("the app list, {shape}"));
        assert_fields(
            &page.body,
            &[("items", Field::Array), ("next_offset", Field::OptInt)],
            "the app list page",
        );
        for app in [&live, &draft] {
            let row = row_of(&page.body, app);
            assert_fields(row, APP_ROW, "an app list row");
            assert_eq!(row["slug"], app.slug.as_str());
            assert_eq!(
                row["org_slug"],
                org_slug.as_str(),
                "ferret builds every check from it"
            );
        }
        // The one value discovery decides on: a published app has a
        // `published_at` string, a draft has null, and ferret checks only the first.
        assert!(row_of(&page.body, &live)["published_at"].is_string());
        assert!(row_of(&page.body, &draft)["published_at"].is_null());
    }
}

#[tokio::test]
async fn next_offset_advances_until_the_last_page_says_null() {
    let fx = staff_fixture().await;
    published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    let (_, key) = staff_keys(&fx).await.remove(1);

    // ferret's walk: stop on null, fail on a `next_offset` that does not advance.
    let mut offset = 0;
    let mut seen = 0;
    for _ in 0..4 {
        let uri = format!(
            "/customer-apps?{}",
            query(&[("limit", "1"), ("offset", &offset.to_string())])
        );
        let page = ferret_get(Some(&key), &uri).await;
        page.assert_json_200("one page of the app list");
        seen += page.body["items"].as_array().unwrap().len();
        match page.body["next_offset"].as_u64() {
            None => {
                assert!(page.body["next_offset"].is_null(), "{}", page.body);
                assert_eq!(seen, 2, "the walk ends having read both apps");
                return;
            }
            Some(next) => {
                assert!(
                    next > offset,
                    "next_offset {next} does not advance past {offset}"
                );
                offset = next;
            }
        }
    }
    panic!("the list never answered next_offset: null");
}
