//! Every write a function can ask the host for, attempted from staging, one
//! step per write `HostOp`: each op the policy holds or refuses is listed in
//! the invocation's single `app.staging.held` row, and each op with an
//! isolated staging home runs there and is not.
//!
//! What staging holds is decided by the policy for what each step names: the
//! probe's app maps no `nonProduction.destinations`, so every warehouse write
//! and `ctx.tx` stays held; its Airhouse writes have a home — the app schema's
//! sibling (`staging_homes_differential` proves where they land) — so they are
//! **not** held, and must not be listed. With no Airhouse in this test they
//! fail to connect, which reaches no store either. Storage, secrets and email
//! (P5a) have homes decided by the op alone, probed in `ISOLATED_STEPS`.
//!
//! The steps are two tables, and both are checked against the policy: a
//! `HostOp` that staging does not simply allow must have a step in the table
//! its decision names, so a write op added later — or a row moved from `Hold`
//! to `Isolate` — fails this test until it is probed. The held assertion is on
//! the held row, not on what the function saw, so removing any one host-side
//! guard fails it: an op that runs for real is not in the row.
//!
//! The OLTP store is provisioned for real (`staging_functions_oltp::OltpApp`),
//! so the `ctx.oltp` and `ctx.oltp.tx` steps reach a real `READ ONLY` session
//! and the row count is checked afterwards. `tx.commit` is its own entry: a
//! read-only transaction's commit is held (rolled back) and logged, which is
//! what the held commit is checked by — `READ ONLY` cannot mask it.

use std::collections::BTreeSet;

use oxy_app::server::api::custom_apps_functions::env_policy::{
    Decision, EnvPolicy, HostOp, Target,
};
use oxy_app::server::api::custom_apps_functions::host::WriterCapability;
use oxy_app_core::custom_app_environment::AppEnvironment;
use serde_json::json;

use crate::custom_app_functions_fixture::FunctionSpec;
use crate::staging_functions::{data, held_ops, held_rows};
use crate::staging_functions_oltp::{OltpApp, run_then_cleanup};

/// One step per op staging holds or refuses: its `HOST_OPS` name and the call
/// that makes it. The `ctx.oltp.tx` read serves two ops — it opens a
/// transaction and commits it.
const WRITE_STEPS: &[(&str, &str)] = &[
    (
        "fetch",
        r#"ctx.fetch("https://api.example.com/journal-entries", { method: "POST", body: "{}" })"#,
    ),
    ("airway.run", r#"ctx.airway.run("orders_sync")"#),
    (
        "warehouse.insert",
        r#"ctx.warehouse.insert("duck", "orders", [{ id: 1 }])"#,
    ),
    (
        "warehouse.exec",
        r#"ctx.warehouse.exec("duck", "insert into orders values (1)")"#,
    ),
    (
        "warehouse.upsert",
        r#"ctx.warehouse.upsert("duck", "orders", [{ id: 1 }], ["id"])"#,
    ),
    (
        "tx.begin",
        r#"ctx.tx("duck", (tx) => tx.query("select 1"))"#,
    ),
    (
        "tx.begin_oltp",
        r#"ctx.oltp.tx((tx) => tx.query("select 1 as one"))"#,
    ),
    (
        "tx.commit",
        r#"ctx.oltp.tx((tx) => tx.query("select 1 as one"))"#,
    ),
    (
        "tx.query",
        r#"ctx.oltp.tx((tx) => tx.query("insert into orders (id) values (5) returning id"))"#,
    ),
    (
        "tx.exec",
        r#"ctx.oltp.tx((tx) => tx.exec("insert into orders (id) values (6)"))"#,
    ),
    (
        "oltp.query",
        r#"ctx.oltp.query("with x as (insert into orders (id) values (3) returning id) select id from x")"#,
    ),
    (
        "oltp.exec",
        r#"ctx.oltp.exec("insert into orders (id) values (2)")"#,
    ),
    (
        "airhouse.exec",
        r#"ctx.airhouse.exec(`insert into ${ctx.airhouse.schema}.events values (1)`)"#,
    ),
    (
        "airhouse.append",
        r#"ctx.airhouse.append("events", [{ id: 1 }])"#,
    ),
];

/// The key `storage.put` wrote, for the steps after it.
const PUT_KEY: &str = r#"out["storage.put"].ok.key"#;

/// One step per op staging runs in its isolated home (P5a), in the order the
/// calls depend on each other. `None`: not run here — `email.send` would reach
/// real SES; `staging_function_homes` sends it to a mock.
const ISOLATED_STEPS: &[(&str, Option<&str>)] = &[
    (
        "storage.put",
        Some(r#"ctx.storage.put("probe.txt", "x", { allowOverwrite: true })"#),
    ),
    ("storage.get", Some("ctx.storage.get(PUT_KEY)")),
    ("storage.head", Some("ctx.storage.head(PUT_KEY)")),
    ("storage.list", Some("ctx.storage.list()")),
    (
        "storage.copy",
        Some(r#"ctx.storage.copy(PUT_KEY, "copy.txt", { allowOverwrite: true })"#),
    ),
    ("storage.delete", Some("ctx.storage.delete(PUT_KEY)")),
    // Presigning needs a bucket, which this test has none of: the step shows
    // the policy let it through to the store, not held.
    (
        "storage.getUploadUrl",
        Some(r#"ctx.storage.getUploadUrl({ pathname: "upload.txt", contentLength: 1 })"#),
    ),
    (
        "storage.getDownloadUrl",
        Some("ctx.storage.getDownloadUrl(PUT_KEY)"),
    ),
    (
        "secrets.set",
        Some(r#"ctx.secrets.set("STAGING_PROBE", "v")"#),
    ),
    ("email.send", None),
];

/// The probe function: every step, each result or error kept under its op.
fn probe_js() -> &'static str {
    let isolated = ISOLATED_STEPS
        .iter()
        .filter_map(|(op, call)| Some((*op, (*call)?)));
    let steps: String = WRITE_STEPS
        .iter()
        .copied()
        .chain(isolated)
        .map(|(op, call)| {
            let call = call.replace("PUT_KEY", PUT_KEY);
            format!("  await attempt({op:?}, async () => {call});\n")
        })
        .collect();
    let js = format!(
        "export default async (req, ctx) => {{\n  const out = {{}};\n  \
         const attempt = async (key, f) => {{\n    \
         try {{ out[key] = {{ ok: await f() }}; }} \
         catch (e) {{ out[key] = {{ error: String(e && e.message ? e.message : e) }}; }}\n  }};\n\
         {steps}  return Response.json(out);\n}};\n"
    );
    Box::leak(js.into_boxed_str())
}

fn probe_manifest() -> serde_json::Value {
    json!({
        "route": true,
        "timeoutSeconds": 60,
        "oltp": { "enabled": true },
        "airhouse": { "enabled": true },
        "email": { "send": true },
        "secrets": { "write": true },
        "storage": { "read": true, "write": true },
        "destinations": ["duck"],
        "customerWarehouseWrites": { "duck": "the staging write probe writes it" },
    })
}

/// What the probe's staging policy decides for `op`, given what its step
/// names: the unmapped database `duck`, or the app's own Airhouse schema.
fn probe_decision(op: HostOp, app_slug: &str) -> Decision {
    let staging = EnvPolicy::for_environment(AppEnvironment::Staging);
    match op {
        HostOp::WarehouseInsert
        | HostOp::WarehouseExec
        | HostOp::WarehouseUpsert
        | HostOp::TxBegin => staging.decide_on_database(op, "duck"),
        HostOp::AirhouseExec | HostOp::AirhouseAppend => {
            let schema = WriterCapability::resolve(true, app_slug)
                .schema()
                .expect("the probe's slug backs a schema");
            staging.decide_in_schema(op, &schema)
        }
        _ => staging.decide(op),
    }
}

/// The steps the probe's policy holds or refuses — each must be listed.
fn held_steps(app_slug: &str) -> Vec<&'static str> {
    WRITE_STEPS
        .iter()
        .map(|(op, _)| *op)
        .filter(|op| {
            let host_op = HostOp::from_name(op).expect("a HOST_OPS name");
            matches!(
                probe_decision(host_op, app_slug),
                Decision::Hold | Decision::Refuse { .. }
            )
        })
        .collect()
}

/// The tables cover every op staging does not simply allow, each in the table
/// its decision names: a new write op, or a row moved between `Hold` and
/// `Isolate`, fails here until its step is where the policy says.
#[test]
fn every_write_op_has_a_probe_step() {
    let staging = EnvPolicy::for_environment(AppEnvironment::Staging);
    let held: BTreeSet<&str> = WRITE_STEPS.iter().map(|(op, _)| *op).collect();
    let isolated: BTreeSet<&str> = ISOLATED_STEPS.iter().map(|(op, _)| *op).collect();
    let misplaced: Vec<(&str, Decision)> = HostOp::ALL
        .iter()
        .map(|op| (op.name(), staging.decide(*op)))
        .filter(|(name, decided)| match decided {
            Decision::Allow => held.contains(name) || isolated.contains(name),
            // A home picked by what the call names (P5b): probed with the
            // writes, where the probe's names decide (`probe_decision`).
            Decision::Isolate(Target::MappedDestination | Target::SiblingSchema) => {
                !held.contains(name) || isolated.contains(name)
            }
            Decision::Isolate(_) => !isolated.contains(name) || held.contains(name),
            Decision::Hold | Decision::Refuse { .. } => {
                !held.contains(name) || isolated.contains(name)
            }
        })
        .collect();
    assert!(
        misplaced.is_empty(),
        "ops whose step is missing or in the wrong table for the policy: {misplaced:?}"
    );
}

#[tokio::test]
async fn every_write_op_is_held_in_staging_and_listed_in_one_held_row() {
    let probe = vec![FunctionSpec {
        name: "probe",
        manifest: probe_manifest(),
        js: probe_js(),
    }];
    let app = OltpApp::provision(&probe).await;
    run_then_cleanup(&app, async {
        let staged = app.staged("probe").await;
        let got = data(&staged);
        let held = held_steps(&app.slug);
        assert!(
            held.contains(&"warehouse.insert") && held.contains(&"tx.begin"),
            "an unmapped warehouse write is held: {held:?}"
        );
        let not_held: Vec<String> = held
            .iter()
            .filter(|op| !step_was_held(op, &got[**op]))
            .map(|op| format!("{op}: {}", got[*op]))
            .collect();
        let rows = held_rows(&app.t).await;
        assert_eq!(rows.len(), 1, "one held row per invocation: {not_held:#?}");
        assert_eq!(rows[0].environment, "staging");
        let listed: BTreeSet<String> = held_ops(&rows[0]).into_iter().collect();
        let missing: Vec<&str> = held
            .iter()
            .copied()
            .filter(|op| !listed.contains(*op))
            .collect();
        assert!(
            missing.is_empty() && not_held.is_empty(),
            "ops that ran without being held: {missing:?} (held row lists {listed:?}); \
             what they answered: {not_held:#?}"
        );
        // An op with an isolated home is performed there, never held — and
        // never listed as if it had been.
        for (op, _) in WRITE_STEPS.iter().filter(|(op, _)| !held.contains(op)) {
            assert!(
                !listed.contains(*op),
                "{op} is isolated, yet listed as held"
            );
            assert!(!step_was_held(op, &got[*op]), "{op}: {}", got[*op]);
        }
        assert_eq!(
            app.order_count().await,
            1,
            "no staging write reached the table"
        );
        assert_isolated_steps_ran_at_home(got, &listed);
    })
    .await;
}

/// Each isolated step passed the policy — it is neither held nor refused, and
/// not in the held row — and storage wrote staging's silo.
fn assert_isolated_steps_ran_at_home(got: &serde_json::Value, listed: &BTreeSet<String>) {
    for (op, call) in ISOLATED_STEPS {
        if call.is_none() {
            continue;
        }
        assert!(!step_was_held(op, &got[*op]), "{op} was held: {}", got[*op]);
        assert!(!listed.contains(*op), "{op} is in the held row: {listed:?}");
    }
    let key = got["storage.put"]["ok"]["key"].as_str().unwrap_or_default();
    assert!(
        key.contains("~staging/"),
        "put lands in staging's silo: {key}"
    );
    let copy = got["storage.copy"]["ok"]["key"]
        .as_str()
        .unwrap_or_default();
    assert!(
        copy.contains("~staging/"),
        "copy lands in staging's silo: {copy}"
    );
    assert_eq!(got["secrets.set"]["ok"], json!({ "ok": true }));
}

/// What a held step looks like to the function: an error carrying the
/// policy's label, a held result, or a 409. The transaction that only reads
/// answers its rows; its commit is held all the same.
fn step_was_held(op: &str, step: &serde_json::Value) -> bool {
    let error = step["error"].as_str().unwrap_or_default();
    error.contains("HeldInStaging:")
        || error.contains("EnvironmentRefused:")
        || step["ok"]["held"] == true
        || step["ok"]["status"] == 409
        || matches!(op, "tx.begin_oltp" | "tx.commit")
}
