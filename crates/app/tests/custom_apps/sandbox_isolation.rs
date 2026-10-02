//! Two sandboxes of one app are isolated from each other and from production
//! (`internal-docs/custom-app-sandboxes.md` → "What is isolated per sandbox"):
//! each runs its own build, works in its own storage silo, and reads its own
//! secrets (then staging's, then production's `shared` ones).
//!
//! Every call goes through the real serve route on a `dev-<handle>--` host,
//! after a real publish to the sandbox. Airhouse is
//! `sandbox_isolation_airhouse`'s.

use entity::apps;
use oxy::service::secret_manager::SecretManagerService;
use oxy_app::server::api::custom_apps_publish::{OrgRef, PublishInput, PublishTarget, publish_to};
use oxy_app::server::api::custom_apps_sandboxes::ops;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use sea_orm::EntityTrait;
use serde_json::{Value, json};

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{FunctionSpec, Tenant, seeded_tenant};
use crate::custom_apps_publish_function_artifacts::tar_gz;
use crate::staging_function_homes::{call, publish_with_env};
use crate::staging_functions::{data, make_guest_staff, production_host, staging_host};

const INDEX_HTML: &[u8] =
    b"<!doctype html><html><head><title>isolation</title></head><body></body></html>";

pub(crate) fn sandbox(handle: &str) -> AppEnvironment {
    AppEnvironment::Dev {
        handle: handle.into(),
    }
}

fn sandbox_host(t: &Tenant, app: &str, handle: &str) -> String {
    format!(
        "dev-{handle}--{}--{app}.customer-apps.oxygen-hq.com",
        t.org_slug
    )
}

/// Create the sandbox `handle` of `app` and publish `functions` (with `env`
/// in the manifest) to it as build `build_id` — through the real publish.
async fn sandbox_serving(
    t: &Tenant,
    app: &apps::Model,
    handle: &str,
    build_id: &str,
    functions: &[FunctionSpec],
    env: Value,
) {
    ops::create(&t.db, app, &sandbox(handle), t.guest_id)
        .await
        .expect("create the sandbox");
    let declared: serde_json::Map<String, Value> = functions
        .iter()
        .map(|f| (f.name.to_string(), f.manifest.clone()))
        .collect();
    let manifest = json!({
        "schemaVersion": 2, "slug": app.slug, "functions": declared, "env": env,
    })
    .to_string();
    let artifacts: Vec<(String, &[u8])> = functions
        .iter()
        .map(|f| (format!("functions/{}.js", f.name), f.js.as_bytes()))
        .collect();
    let mut files: Vec<(&str, &[u8])> = vec![
        ("index.html", INDEX_HTML),
        ("oxy-app.json", manifest.as_bytes()),
    ];
    files.extend(artifacts.iter().map(|(path, js)| (path.as_str(), *js)));
    let publish = PublishInput {
        org_ref: Some(OrgRef::Id(t.org_id)),
        app_slug: app.slug.clone(),
        project_id: demo_workspace_id(),
        branch: None,
        build_id: build_id.to_string(),
        name: None,
        promote: false,
        tarball: tar_gz(&files),
        manifest: None,
        source_repo: None,
        commit_sha: None,
        published_by: Some(t.guest_id),
        published_by_email: Some(LOCAL_GUEST_EMAIL.to_string()),
        machine_app_id: None,
        published_via: None,
        semantic_revision_id: None,
    };
    publish_to(publish, PublishTarget::Sandbox(sandbox(handle)))
        .await
        .expect("publish to the sandbox");
}

async fn app_row(t: &Tenant, id: uuid::Uuid) -> apps::Model {
    apps::Entity::find_by_id(id)
        .one(&t.db)
        .await
        .expect("read the app")
        .expect("the app")
}

// ── Builds and storage ───────────────────────────────────────────────────────

/// One function, three builds: each says which build it is, then does what
/// the request asks of its own silo and reports what it sees there.
macro_rules! homes_js {
    ($mark:literal) => {
        concat!(
            "const MARK = \"",
            $mark,
            "\";\n",
            r#"
export default async (req, ctx) => {
  const { op, key } = JSON.parse(req.body || "{}");
  const out = { build: MARK, channel: ctx.channel };
  if (op === "put") out.put = (await ctx.storage.put(key, MARK, { allowOverwrite: true })).key;
  const refused = (e) => "refused: " + String(e && e.message ? e.message : e);
  if (op === "delete") {
    try { out.deleted = (await ctx.storage.delete(key)).deleted; } catch (e) { out.deleted = refused(e); }
  }
  if (key) {
    try { const o = await ctx.storage.get(key); out.body = o ? o.body : null; } catch (e) { out.body = refused(e); }
  }
  out.list = (await ctx.storage.list()).objects.map((o) => o.key).sort();
  return Response.json(out);
};
"#
        )
    };
}

fn homes(js: &'static str) -> [FunctionSpec; 1] {
    [FunctionSpec {
        name: "homes",
        manifest: json!({ "route": true, "storage": { "read": true, "write": true } }),
        js,
    }]
}

/// Each sandbox runs the build published to it, never the other's or
/// production's; an object one puts is in its own silo and absent from the
/// other's and from production's; and a sandbox deleting production's key
/// removes nothing of production's.
#[tokio::test]
async fn two_sandboxes_run_their_own_builds_in_their_own_silos() {
    let t = seeded_tenant().await;
    let slug = "sbx-iso";
    let published = publish_with_env(&t, slug, &homes(homes_js!("production")), json!({}))
        .await
        .expect("publish production");
    make_guest_staff();
    let app = app_row(&t, published.app_id).await;
    sandbox_serving(&t, &app, "a1", "build-a", &homes(homes_js!("a")), json!({})).await;
    sandbox_serving(&t, &app, "b2", "build-b", &homes(homes_js!("b")), json!({})).await;
    let on = |handle: &str| sandbox_host(&t, slug, handle);
    let prod = production_host(&t, slug);
    let silo = |suffix: &str| format!("customer-app-storage/{}{suffix}/", app.id);

    // Production writes a key; each sandbox reads it through the fallback.
    let written = call(
        &t,
        slug,
        "homes",
        &prod,
        json!({ "op": "put", "key": "notes/shared.txt" }),
    )
    .await;
    let prod_key = data(&written)["put"].as_str().expect("a key").to_string();
    assert_eq!(prod_key, format!("{}notes/shared.txt", silo("")));

    let a = call(
        &t,
        slug,
        "homes",
        &on("a1"),
        json!({ "op": "put", "key": "notes/mine.txt" }),
    )
    .await;
    let a = data(&a);
    assert_eq!(
        (&a["build"], &a["channel"]),
        (&json!("a"), &json!("dev-a1"))
    );
    let a_key = a["put"].as_str().expect("a key").to_string();
    assert_eq!(a_key, format!("{}notes/mine.txt", silo("~dev-a1")));
    assert_eq!(
        a["list"],
        json!([a_key]),
        "dev-a1's listing is its own silo"
    );

    let b = call(&t, slug, "homes", &on("b2"), json!({ "key": a_key })).await;
    let b = data(&b);
    assert_eq!(
        (&b["build"], &b["channel"]),
        (&json!("b"), &json!("dev-b2"))
    );
    assert_ne!(b["body"], "a", "dev-a1's object is not dev-b2's to read");
    assert!(
        b["body"].is_null()
            || b["body"]
                .as_str()
                .is_some_and(|s| s.starts_with("refused: ")),
        "{}",
        b["body"]
    );
    assert_eq!(b["list"], json!([]), "dev-b2 holds nothing");

    // dev-b2 reads production's key, and its delete of it never reaches
    // production: the key is re-rooted into dev-b2's own silo, which holds no
    // such object, and production's is still there to read.
    let b = call(
        &t,
        slug,
        "homes",
        &on("b2"),
        json!({ "op": "delete", "key": prod_key }),
    )
    .await;
    assert_eq!(
        data(&b)["body"],
        "production",
        "production's object survives a sandbox's delete of its key"
    );

    let p = call(&t, slug, "homes", &prod, json!({ "key": prod_key })).await;
    let p = data(&p);
    assert_eq!(
        (&p["build"], &p["channel"]),
        (&json!("production"), &json!("production"))
    );
    assert_eq!(p["body"], "production");
    assert_eq!(
        p["list"],
        json!([prod_key]),
        "production's silo is what production wrote"
    );

    // dev-a1 still holds its own, untouched by dev-b2's delete.
    let a = call(&t, slug, "homes", &on("a1"), json!({ "key": a_key })).await;
    assert_eq!(data(&a)["body"], "a");
}

// ── Secrets ──────────────────────────────────────────────────────────────────

const ENV_JS: &str = r#"
export default async (req, ctx) => {
  const { set } = JSON.parse(req.body || "{}");
  const out = { channel: ctx.channel };
  for (const key of set || []) {
    try { await ctx.secrets.set(key, ctx.channel + "-set"); out[key] = "written"; }
    catch (e) { out[key] = String(e && e.message ? e.message : e); }
  }
  out.env = ctx.env;
  return Response.json(out);
};
"#;

fn use_a_fixed_encryption_key() {
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::set_var(
            "OXY_ENCRYPTION_KEY",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        );
    }
}

/// A sandbox reads its own value, then staging's, then production's for a
/// `shared` key. A value set in one sandbox is unset in the other; staging's
/// shows through in both until a sandbox overrides it; and `ctx.secrets.set`
/// is refused for a key the run read from staging — rotating it would void
/// staging's own.
#[tokio::test]
async fn a_sandbox_reads_its_own_secrets_then_stagings_and_never_another_sandboxs() {
    use_a_fixed_encryption_key();
    let t = seeded_tenant().await;
    let slug = "sbx-env";
    let env_fn = [FunctionSpec {
        name: "env",
        manifest: json!({ "route": true, "secrets": { "write": true } }),
        js: ENV_JS,
    }];
    let declared = json!({ "SHARED_KEY": { "shared": true }, "PRIVATE_KEY": {} });
    let published = publish_with_env(&t, slug, &env_fn, declared.clone())
        .await
        .expect("publish production and staging");
    make_guest_staff();
    let app = app_row(&t, published.app_id).await;
    sandbox_serving(&t, &app, "a1", "env-a", &env_fn, declared.clone()).await;
    sandbox_serving(&t, &app, "b2", "env-b", &env_fn, declared).await;

    let secrets = SecretManagerService::new(demo_workspace_id());
    for (environment, key, value) in [
        (None, "SHARED_KEY", "prod-shared"),
        (None, "PRIVATE_KEY", "prod-private"),
        (Some("staging"), "QB_TOKEN", "stg-token"),
        (Some("dev-a1"), "MINE", "a1-own"),
    ] {
        secrets
            .set_app_secret_in(&t.db, app.id, environment, key, value, t.guest_id)
            .await
            .expect("seed a secret");
    }
    let on = |handle: &str| sandbox_host(&t, slug, handle);

    // dev-a1: its own key, staging's, production's shared one — and no
    // private production key.
    let a = call(&t, slug, "env", &on("a1"), json!({ "set": ["QB_TOKEN"] })).await;
    let a = data(&a);
    assert_eq!(a["channel"], "dev-a1");
    assert_eq!(
        a["env"],
        json!({ "MINE": "a1-own", "QB_TOKEN": "stg-token", "SHARED_KEY": "prod-shared" })
    );
    let refused = a["QB_TOKEN"].as_str().expect("the set's answer");
    assert!(refused.contains("EnvironmentRefused:"), "{refused}");
    assert!(
        refused.contains("another environment"),
        "the refusal says whose it is: {refused}"
    );
    assert_eq!(
        secrets_value(&format!("apps/{}/staging/QB_TOKEN", app.id))
            .await
            .as_deref(),
        Some("stg-token"),
        "staging's value is untouched"
    );
    assert_eq!(
        secrets_value(&format!("apps/{}/dev-a1/QB_TOKEN", app.id)).await,
        None
    );

    // dev-b2 holds nothing of dev-a1's: staging's and the shared key only.
    // (`ctx.env` is read when the run starts, so a key the run sets shows in
    // the next one.)
    let b = call(&t, slug, "env", &on("b2"), json!({ "set": ["ROTATED"] })).await;
    let b = data(&b);
    assert_eq!(b["ROTATED"], "written", "a sandbox sets a key of its own");
    assert_eq!(
        b["env"],
        json!({ "QB_TOKEN": "stg-token", "SHARED_KEY": "prod-shared" }),
        "MINE is dev-a1's alone"
    );
    assert_eq!(
        secrets_value(&format!("apps/{}/dev-b2/ROTATED", app.id))
            .await
            .as_deref(),
        Some("dev-b2-set")
    );

    // dev-a1 overrides staging's value with its own: staging's no longer
    // shows through there, the key is now dev-a1's to rotate, and dev-b2
    // still reads staging's — and its own ROTATED, which dev-a1 never sees.
    secrets
        .set_app_secret_in(
            &t.db,
            app.id,
            Some("dev-a1"),
            "QB_TOKEN",
            "a1-token",
            t.guest_id,
        )
        .await
        .expect("override in dev-a1");
    let a = call(&t, slug, "env", &on("a1"), json!({ "set": ["QB_TOKEN"] })).await;
    let a = data(&a);
    assert_eq!(
        a["env"],
        json!({ "MINE": "a1-own", "QB_TOKEN": "a1-token", "SHARED_KEY": "prod-shared" })
    );
    assert_eq!(
        a["QB_TOKEN"], "written",
        "its own value is its own to rotate"
    );
    assert_eq!(
        secrets_value(&format!("apps/{}/dev-a1/QB_TOKEN", app.id))
            .await
            .as_deref(),
        Some("dev-a1-set")
    );
    let b = call(&t, slug, "env", &on("b2"), json!({})).await;
    assert_eq!(
        data(&b)["env"],
        json!({ "QB_TOKEN": "stg-token", "ROTATED": "dev-b2-set", "SHARED_KEY": "prod-shared" })
    );

    // Staging and production see neither sandbox's keys.
    let stg = call(&t, slug, "env", &staging_host(&t, slug), json!({})).await;
    assert_eq!(
        data(&stg)["env"],
        json!({ "QB_TOKEN": "stg-token", "SHARED_KEY": "prod-shared" })
    );
    let prod = call(&t, slug, "env", &production_host(&t, slug), json!({})).await;
    assert_eq!(
        data(&prod)["env"],
        json!({ "SHARED_KEY": "prod-shared", "PRIVATE_KEY": "prod-private" })
    );
}

async fn secrets_value(name: &str) -> Option<String> {
    let secrets = SecretManagerService::new(demo_workspace_id());
    secrets.clear_cache().await;
    secrets.get_secret(name).await
}
