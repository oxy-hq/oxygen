//! Phase 5a of the previews plan, secrets (environments design §4.2): staging
//! `ctx.env` reads `apps/<id>/staging/<KEY>`, falling back to production's
//! value only for a key **both** the staging build and the build production
//! serves mark `shared`; `ctx.secrets.set` writes the staging path and refuses
//! a key read through the fallback; production's `ctx.env` never lists a
//! staging key; a publish refuses `shared` on a key a function of that build —
//! or of production's — writes or verifies webhooks with.
//!
//! Every call goes through the real serve route on a `staging--` or production
//! host (`staging_functions`).

use oxy::service::secret_manager::SecretManagerService;
use oxy_app::server::api::custom_apps_publish::PublishError;
use serde_json::json;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{FunctionSpec, seeded_tenant};
use crate::staging_function_homes::{call, publish_env_build, publish_with_env};
use crate::staging_functions::{data, make_guest_staff, production_host, staging_host};

const ENV_JS: &str = r#"
export default async (req, ctx) => {
  const out = { env: ctx.env };
  // Computed, so the publish-time scan cannot see it: the runtime refuses it.
  const shared = ["SHARED", "KEY"].join("_");
  try { await ctx.secrets.set(shared, "rotated"); out.sharedSet = "written"; }
  catch (e) { out.sharedSet = String(e && e.message ? e.message : e); }
  await ctx.secrets.set("WRITTEN_KEY", ctx.channel + "-written");
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

#[tokio::test]
async fn staging_env_overlays_production_only_for_shared_keys() {
    use_a_fixed_encryption_key();
    let t = seeded_tenant().await;
    let app = "stg-env";
    let env_fn = FunctionSpec {
        name: "env",
        manifest: json!({ "route": true, "secrets": { "write": true } }),
        js: ENV_JS,
    };
    let manifest_env = json!({ "SHARED_KEY": { "shared": true }, "PRIVATE_KEY": {} });
    let published = publish_with_env(&t, app, std::slice::from_ref(&env_fn), manifest_env)
        .await
        .unwrap();
    let id = published.app_id;
    let secrets = SecretManagerService::new(demo_workspace_id());
    for (env, key, value) in [
        (None, "SHARED_KEY", "prod-shared"),
        (None, "PRIVATE_KEY", "prod-private"),
        (Some("staging"), "STAGING_ONLY", "stg-only"),
    ] {
        secrets
            .set_app_secret_in(&t.db, id, env, key, value, t.guest_id)
            .await
            .expect("seed secret");
    }
    make_guest_staff();

    let staged = call(&t, app, "env", &staging_host(&t, app), json!({})).await;
    let got = data(&staged);
    assert_eq!(
        got["env"],
        json!({ "SHARED_KEY": "prod-shared", "STAGING_ONLY": "stg-only" }),
        "staging reads its own keys and production's shared ones; PRIVATE_KEY is unset"
    );
    let refused = got["sharedSet"].as_str().unwrap();
    assert!(refused.contains("EnvironmentRefused:"), "{refused}");
    assert!(
        refused.contains("shared"),
        "the refusal says why: {refused}"
    );
    let read = |name: String| async move { secrets_value(&name).await };
    assert_eq!(
        read(format!("apps/{id}/SHARED_KEY")).await.as_deref(),
        Some("prod-shared")
    );
    assert_eq!(
        read(format!("apps/{id}/staging/WRITTEN_KEY"))
            .await
            .as_deref(),
        Some("staging-written"),
        "ctx.secrets.set wrote staging's path"
    );
    assert_eq!(read(format!("apps/{id}/WRITTEN_KEY")).await, None);

    let prod = call(&t, app, "env", &production_host(&t, app), json!({})).await;
    let got = data(&prod);
    assert_eq!(
        got["env"],
        json!({ "SHARED_KEY": "prod-shared", "PRIVATE_KEY": "prod-private" }),
        "production lists its top-level keys only — never staging/STAGING_ONLY"
    );
    assert_eq!(
        got["sharedSet"], "written",
        "production rotates its own key"
    );
}

async fn secrets_value(name: &str) -> Option<String> {
    SecretManagerService::new(demo_workspace_id())
        .get_secret(name)
        .await
}

#[tokio::test]
async fn publish_refuses_shared_on_a_written_key_or_a_webhook_secret() {
    let t = seeded_tenant().await;
    let writer = FunctionSpec {
        name: "refresh",
        manifest: json!({ "route": true, "secrets": { "write": true } }),
        js: r#"export default async (req, ctx) => { await ctx.secrets.set("QB_REFRESH_TOKEN", "t"); return Response.json({}); };"#,
    };
    let hook = FunctionSpec {
        name: "hook",
        manifest: json!({ "route": true, "webhook": { "secretVar": "SIGNING_KEY" } }),
        js: "export default async () => Response.json({});",
    };
    let err = publish_with_env(
        &t,
        "stg-shared",
        &[writer, hook],
        json!({
            "QB_REFRESH_TOKEN": { "shared": true },
            "SIGNING_KEY": { "shared": true },
            "READ_ONLY_KEY": { "shared": true },
        }),
    )
    .await
    .expect_err("shared on a written key is refused");
    let PublishError::SharedEnvConflict { conflicts } = &err else {
        panic!("expected SharedEnvConflict, got {err:?}");
    };
    assert_eq!(conflicts.len(), 2, "{conflicts:?}");
    assert!(err.to_string().contains("QB_REFRESH_TOKEN"), "{err}");
    assert!(err.to_string().contains("SIGNING_KEY"), "{err}");
    assert!(!err.to_string().contains("READ_ONLY_KEY"), "{err}");
}

/// Reads `ctx.env` and writes nothing: the publish-time scan has no key to
/// object to, so only the both-builds rule stands between staging and
/// production's value.
const READ_ENV_JS: &str = "export default async (req, ctx) => Response.json({ env: ctx.env });";

fn reader() -> FunctionSpec {
    FunctionSpec {
        name: "env",
        manifest: json!({ "route": true }),
        js: READ_ENV_JS,
    }
}

/// The review's escape: a staging build marks production's
/// `QB_REFRESH_TOKEN` shared. Production's build does not, so staging reads
/// nothing of it — and a key both builds share still falls back.
#[tokio::test]
async fn a_staging_build_alone_cannot_share_a_production_key() {
    use_a_fixed_encryption_key();
    let t = seeded_tenant().await;
    let app = "stg-escape";
    let production_env = json!({ "QB_REFRESH_TOKEN": {}, "READ_ONLY": { "shared": true } });
    let published = publish_env_build(&t, app, &[reader()], production_env, "esc-prod", true)
        .await
        .unwrap();
    let id = published.app_id;
    let secrets = SecretManagerService::new(demo_workspace_id());
    for key in ["QB_REFRESH_TOKEN", "READ_ONLY"] {
        secrets
            .set_app_secret_in(&t.db, id, None, key, "prod-value", t.guest_id)
            .await
            .expect("seed secret");
    }
    let staging_env = json!({
        "QB_REFRESH_TOKEN": { "shared": true },
        "READ_ONLY": { "shared": true },
    });
    publish_env_build(&t, app, &[reader()], staging_env, "esc-stg", false)
        .await
        .expect("a staging build may mark keys shared");
    make_guest_staff();

    let staged = call(&t, app, "env", &staging_host(&t, app), json!({})).await;
    assert_eq!(
        data(&staged)["env"],
        json!({ "READ_ONLY": "prod-value" }),
        "only a key production's build also shares falls back"
    );
    let prod = call(&t, app, "env", &production_host(&t, app), json!({})).await;
    assert_eq!(
        data(&prod)["env"],
        json!({ "QB_REFRESH_TOKEN": "prod-value", "READ_ONLY": "prod-value" })
    );
}

/// An app never promoted has no production build to agree with: it shares
/// nothing, whatever its staging build marks.
#[tokio::test]
async fn an_app_never_promoted_shares_nothing() {
    use_a_fixed_encryption_key();
    let t = seeded_tenant().await;
    let app = "stg-unpromoted";
    let published = publish_env_build(
        &t,
        app,
        &[reader()],
        json!({ "READ_ONLY": { "shared": true } }),
        "unpromoted-1",
        false,
    )
    .await
    .unwrap();
    SecretManagerService::new(demo_workspace_id())
        .set_app_secret_in(
            &t.db,
            published.app_id,
            None,
            "READ_ONLY",
            "prod",
            t.guest_id,
        )
        .await
        .expect("seed secret");
    make_guest_staff();
    let staged = call(&t, app, "env", &staging_host(&t, app), json!({})).await;
    assert_eq!(data(&staged)["env"], json!({}));
}

/// A staging publish is checked against the build production serves: a key
/// production's function writes, or verifies its webhook with, cannot be
/// marked shared by a staging bundle that does neither.
#[tokio::test]
async fn publish_refuses_shared_on_a_key_productions_build_writes() {
    let t = seeded_tenant().await;
    let app = "stg-prod-writes";
    let writer = FunctionSpec {
        name: "refresh",
        manifest: json!({ "route": true, "secrets": { "write": true } }),
        js: r#"export default async (req, ctx) => { const { secrets: s } = ctx; await s.set("QB_REFRESH_TOKEN", "t"); return Response.json({}); };"#,
    };
    let hook = FunctionSpec {
        name: "hook",
        manifest: json!({ "route": true, "webhook": { "secretVar": "SIGNING_KEY" } }),
        js: "export default async () => Response.json({});",
    };
    publish_env_build(&t, app, &[writer, hook], json!({}), "pw-prod", true)
        .await
        .unwrap();
    let err = publish_env_build(
        &t,
        app,
        &[reader()],
        json!({
            "QB_REFRESH_TOKEN": { "shared": true },
            "SIGNING_KEY": { "shared": true },
            "READ_ONLY": { "shared": true },
        }),
        "pw-stg",
        false,
    )
    .await
    .expect_err("production's written key and webhook key are refused");
    let PublishError::SharedEnvConflict { conflicts } = &err else {
        panic!("expected SharedEnvConflict, got {err:?}");
    };
    assert_eq!(conflicts.len(), 2, "{conflicts:?}");
    assert!(
        conflicts
            .iter()
            .all(|c| c.contains("production's build `pw-prod`")),
        "{conflicts:?}"
    );
    assert!(!err.to_string().contains("READ_ONLY"), "{err}");
}
