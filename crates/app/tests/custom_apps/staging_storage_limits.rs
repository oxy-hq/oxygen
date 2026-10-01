//! Storage limits outside production (`custom_apps_storage::environment_limits`):
//! a staging silo is refused past its own cap
//! (`OXY_CUSTOMER_APPS_STORAGE_ENVIRONMENT_MAX_BYTES`), and its bytes — metered in the app's
//! usage row under `~staging/` — never count toward the org quota that gates
//! production's writes, so a staging loop cannot pause production.
//!
//! Every call goes through the real serve route (`staging_functions`); storage
//! is the filesystem store `test_db` points `OXY_STATE_DIR` at. nextest runs
//! each test in its own process, so the limits are set per test.

use chrono::Utc;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::custom_app_functions_fixture::{FunctionSpec, Tenant, seeded_tenant};
use crate::staging_function_homes::{call, publish_with_env};
use crate::staging_functions::{data, make_guest_staff, production_host, staging_host};

/// Writes `{ size }` bytes at `{ path }`, answering the key or the error.
const PUT_JS: &str = r#"
export default async (req, ctx) => {
  const { path, size } = JSON.parse(req.body);
  try {
    const out = await ctx.storage.put(path, "x".repeat(size), { allowOverwrite: true });
    return Response.json({ key: out.key });
  } catch (e) {
    return Response.json({ error: String(e && e.message ? e.message : e) });
  }
};
"#;

fn put_fn() -> FunctionSpec {
    FunctionSpec {
        name: "put",
        manifest: json!({ "route": true, "storage": { "read": true, "write": true } }),
        js: PUT_JS,
    }
}

fn set_env(key: &str, value: &str) {
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::set_var(key, value) }
}

async fn put(t: &Tenant, app: &str, host: &str, path: &str, size: usize) -> Value {
    let call = call(t, app, "put", host, json!({ "path": path, "size": size })).await;
    data(&call).clone()
}

#[tokio::test]
async fn a_staging_silo_is_refused_past_its_own_cap() {
    set_env("OXY_CUSTOMER_APPS_STORAGE_ENVIRONMENT_MAX_BYTES", "16");
    let t = seeded_tenant().await;
    let app = "stg-cap";
    publish_with_env(&t, app, &[put_fn()], json!({}))
        .await
        .unwrap();
    make_guest_staff();
    let (stg, prod) = (staging_host(&t, app), production_host(&t, app));

    let first = put(&t, app, &stg, "a.txt", 10).await;
    assert!(first["key"].as_str().is_some(), "under the cap: {first}");
    let second = put(&t, app, &stg, "b.txt", 10).await;
    let refused = second["error"].as_str().unwrap_or_default();
    assert!(
        refused.contains("environment's storage is full"),
        "20 bytes past a 16-byte cap: {second}"
    );
    // Overwriting what is there is measured against the silo as it stands.
    let rewrite = put(&t, app, &stg, "a.txt", 6).await;
    assert!(rewrite["key"].as_str().is_some(), "10 + 6 = 16: {rewrite}");

    for path in ["a.txt", "b.txt", "c.txt"] {
        let written = put(&t, app, &prod, path, 10).await;
        assert!(
            written["key"].as_str().is_some(),
            "production has no staging cap: {written}"
        );
    }
}

/// The org's only usage row: `app_id`'s, whose production bytes are
/// `production` and whose staging silo holds `staging`.
async fn usage_row(t: &Tenant, app_id: Uuid, production: i64, staging: i64) {
    let breakdown = json!({
        "uploads/": { "bytes": production, "objects": 1 },
        "~staging/uploads/": { "bytes": staging, "objects": 1 },
    });
    let row = entity::app_storage_usage::ActiveModel {
        app_id: ActiveValue::Set(app_id),
        org_id: ActiveValue::Set(t.org_id),
        bytes: ActiveValue::Set(production + staging),
        object_count: ActiveValue::Set(2),
        untagged_bytes: ActiveValue::Set(0),
        untagged_object_count: ActiveValue::Set(0),
        prefix_breakdown: ActiveValue::Set(Some(breakdown)),
        measured_at: ActiveValue::Set(Utc::now().into()),
        measure_status: ActiveValue::Set("ok".into()),
        measure_detail: ActiveValue::Set(None),
    };
    entity::app_storage_usage::Entity::delete_many()
        .filter(entity::app_storage_usage::Column::OrgId.eq(t.org_id))
        .exec(&t.db)
        .await
        .expect("clear the org's usage rows");
    row.insert(&t.db).await.expect("insert usage row");
}

#[tokio::test]
async fn staging_bytes_never_count_toward_the_org_quota() {
    set_env("OXY_CUSTOMER_APPS_STORAGE_SOFT_LIMIT_BYTES", "1000");
    set_env("OXY_CUSTOMER_APPS_STORAGE_HARD_LIMIT_BYTES", "2000");
    let t = seeded_tenant().await;
    let app = "stg-quota";
    let app_id = publish_with_env(&t, app, &[put_fn()], json!({}))
        .await
        .unwrap()
        .app_id;
    let prod = production_host(&t, app);

    // Far past the hard limit, all of it staging's: production still writes.
    usage_row(&t, app_id, 10, 50_000).await;
    let written = put(&t, app, &prod, "a.txt", 4).await;
    assert!(
        written["key"].as_str().is_some(),
        "staging's bytes do not pause production: {written}"
    );

    // The control: the same total as production's own bytes is refused.
    usage_row(&t, app_id, 50_000, 10).await;
    let refused = put(&t, app, &prod, "b.txt", 4).await;
    assert!(
        refused["error"]
            .as_str()
            .unwrap_or_default()
            .contains("storage quota exceeded"),
        "production past its hard limit is refused: {refused}"
    );
}
