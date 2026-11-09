//! G1 of the classic-agent retirement memo: an agent is looked up by the key
//! the compiler wrote, not by its file stem.
//!
//! `oxy-compile` keys a nameless agent by its path minus the extension
//! (`agents/sales/foo`), so a lookup that strips a ref to `foo` misses the
//! row on every pod, and a fleet pod has no file to fall back to.

use agentic_automation::WorkspaceReadError;
use agentic_automation::preview_names::scoped;
use agentic_pipeline::platform::ProjectContext;
use serde_json::json;

use super::super::PreviewPlatformContext;
use super::fixture::{RUN, exec, row, seed_revision, seed_workspace};
use crate::server::test_support::{SKIP_MSG, test_db};

/// Seeds the row exactly as `oxy-compile` writes a nameless agent in a
/// subdirectory: `name` is the path minus `.agentic.yml`.
async fn seed_nameless_agent(db: &sea_orm::DatabaseConnection, rev: uuid::Uuid) {
    let name = oxy_compile::compile::derive_name_from_path("agents/sales/foo.agentic.yml");
    assert_eq!(name, "agents/sales/foo", "the compiler's key for this file");
    exec(
        db,
        "INSERT INTO agent_definitions (revision_id, name, file_path, definition) \
         VALUES ($1, $2, 'agents/sales/foo.agentic.yml', $3)",
        vec![
            rev.into(),
            name.into(),
            json!({ "description": "sales-foo" }).into(),
        ],
    )
    .await;
}

#[tokio::test]
async fn a_nameless_agent_in_a_subdirectory_resolves_by_its_path() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let ws = seed_workspace(&db).await;
    let staging = seed_revision(&db, ws, "staging", json!([])).await;
    seed_nameless_agent(&db, staging).await;
    let ctx = PreviewPlatformContext::new(&db, &row(ws, staging))
        .await
        .expect("platform");

    // The path, the path without its extension, and the bare file stem the
    // compiler's `agent_ref` check also accepts (one `foo` in this revision).
    for agent_ref in ["agents/sales/foo.agentic.yml", "agents/sales/foo", "foo"] {
        let yaml = ctx
            .resolve_agent(&scoped(RUN, agent_ref))
            .await
            .unwrap_or_else(|e| panic!("`{agent_ref}` missed its compiled row: {e}"))
            .expect("this host always answers");
        assert!(yaml.contains("sales-foo"), "{agent_ref}: {yaml}");
    }

    // A ref naming nothing is a genuine miss, answered from the revision and
    // not the disk: `Missing`, which a run turns into a 404.
    for agent_ref in ["agents/other/foo.agentic.yml", "missing"] {
        let miss = ctx
            .resolve_agent(&scoped(RUN, agent_ref))
            .await
            .expect_err("no such agent");
        assert!(
            matches!(miss, WorkspaceReadError::Missing(_)),
            "{agent_ref}: {miss:?}"
        );
    }
}
