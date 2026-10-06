//! Markdown context documents across the compile boundary.
//!
//! An analytics agent's `context:` globs name the `.md` files injected into its
//! prompt. They used to be read off the context root, which on a pod with no
//! working copy is a materialised copy of the compiled revision — and markdown
//! was never in it. The run started without its documents and said nothing.
//!
//! These drive the real path end to end, as `airway_compile_boundary` does for
//! pipelines: `oxy_compile::compile_workspace` writes the rows, then the port
//! (`ProjectContext::resolve_context_documents`) is asked through the host
//! adapter (`OxyProjectContext`), where `ConfigManager::context_documents`
//! lets `Origin` decide. The workspace directory is **absent from disk**
//! wherever the claim is about a pod that has none.
//!
//! No process role is set anywhere here, deliberately: the read consults the
//! manager's `Origin` and whether the root is on disk, never `OXY_ROLE`. That
//! is what "every role reads the revision" means in code.
//!
//! Database-backed through [`common::fresh_db`] — own database per test, so
//! this module belongs to `db-per-test` in `.config/nextest.toml`.

use std::sync::Arc;

use agentic_pipeline::platform::ProjectContext;
use entity::{context_document_definitions, revisions};
use sea_orm::EntityTrait;

use crate::common::Schema;

mod fixture;
use fixture::*;

/// A compile carries exactly the markdown some agent's `context:` reaches.
#[tokio::test]
async fn a_compile_stores_the_documents_agents_reach_and_no_others() {
    let db = setup_db(Schema::Central).await;
    let files = working_copy();
    let ws_id = seed_workspace_at(&db, files.path()).await;
    let revision_id = compile(&db, ws_id, files.path()).await;

    assert_eq!(
        compiled_document_paths(&db, revision_id).await,
        [
            "docs/glossary.md",
            "docs/metrics.md",
            "finance/close.md",
            "semantics/notes.md",
        ],
        "the README no agent references is not compiled"
    );

    let glossary = context_document_definitions::Entity::find_by_id((
        revision_id,
        "docs/glossary.md".to_string(),
    ))
    .one(&db)
    .await
    .expect("read the row")
    .expect("the glossary compiled");
    assert_eq!(glossary.content, GLOSSARY, "the body is stored verbatim");

    let revision = revisions::Entity::find_by_id(revision_id)
        .one(&db)
        .await
        .expect("read the revision")
        .expect("the revision exists");
    assert!(
        revision.schema_version >= oxy_compile::context_documents::SINCE_SCHEMA_VERSION,
        "the revision says it carries documents, which is how a reader tells \
         `no documents` from `never compiled` (got {})",
        revision.schema_version
    );
}

/// THE claim: an analytics run on a pod with no working copy gets its
/// documents. Before this they were globbed from a context root that never
/// held markdown, and the answer was an empty list with no error.
#[tokio::test]
async fn a_run_with_no_working_copy_reads_its_documents_from_the_revision() {
    let db = setup_db(Schema::Central).await;
    let files = working_copy();
    let ws_id = seed_workspace_at(&db, files.path()).await;
    let revision_id = compile(&db, ws_id, files.path()).await;

    // A different node: the files this was compiled from are not here.
    let pod = context(ws_id, &nowhere(), Some(revision_id)).await;

    assert_eq!(
        pod.resolve_context_documents(&analyst_patterns())
            .await
            .expect("the boundary answers"),
        Some(analyst_documents()),
        "the analyst's documents, in the order its patterns list them"
    );
    assert_eq!(
        pod.resolve_context_documents(&strings(&["./finance/**/*.md"]))
            .await
            .expect("the boundary answers"),
        Some(vec![CLOSE.to_string()]),
        "a revision carries every agent's documents; each agent gets its own"
    );
}

/// The point of the change, stated as one equality: the pod with no working
/// copy, the node that owns the files and serves the revision, and the node
/// reading a draft branch off disk all hand the same agent the same documents.
#[tokio::test]
async fn every_node_hands_the_same_agent_the_same_documents() {
    let db = setup_db(Schema::Central).await;
    let files = working_copy();
    let ws_id = seed_workspace_at(&db, files.path()).await;
    let revision_id = compile(&db, ws_id, files.path()).await;

    let diskless = context(ws_id, &nowhere(), Some(revision_id)).await;
    let owns_files = context(ws_id, files.path(), Some(revision_id)).await;
    let draft_branch = context(ws_id, files.path(), None).await;

    for (node, ctx) in [
        ("no working copy, compiled", &diskless),
        ("working copy, compiled", &owns_files),
        ("working copy, reading the disk", &draft_branch),
    ] {
        assert_eq!(
            ctx.resolve_context_documents(&analyst_patterns())
                .await
                .unwrap_or_else(|e| panic!("{node}: {e}")),
            Some(analyst_documents()),
            "{node}"
        );
    }
}

/// A revision written before documents were a compiled kind says nothing about
/// them. On a pod with no files the run goes ahead with none: that is exactly
/// what such a pod did before documents were compiled, so nothing starts
/// failing on the day this ships. What is new is that the compile which WILL
/// carry them is queued, so the next run has them.
///
/// The port answers `Some([])`, not an error. That is the only thing `start`
/// and `resume` branch on, so no run is refused and none is closed.
#[tokio::test]
async fn an_older_revision_on_a_pod_with_no_files_runs_without_documents_and_queues_a_compile() {
    let db = setup_db(Schema::All).await;
    let files = working_copy();
    let ws_id = seed_workspace_at(&db, files.path()).await;
    let revision_id = compile(&db, ws_id, files.path()).await;
    make_revision_predate_documents(&db, revision_id).await;

    let pod = context(ws_id, &nowhere(), Some(revision_id))
        .await
        .with_db(Arc::new(db.clone()));

    assert_eq!(
        pod.resolve_context_documents(&analyst_patterns())
            .await
            .expect("an older revision is never a failed run"),
        Some(vec![]),
        "the run proceeds with no documents, as it did before they were compiled"
    );
    assert_eq!(
        queued_compiles(&db).await,
        1,
        "and the self-heal compile that will carry them was queued"
    );

    // Every later run on the same revision asks again; the queue dedupes.
    pod.resolve_context_documents(&analyst_patterns())
        .await
        .expect("still not a failure");
    assert_eq!(
        queued_compiles(&db).await,
        1,
        "one compile, not one per run"
    );
}

/// A workspace preview or a custom app's staging build is pinned to a staging
/// revision, which is never recompiled in place. An older one runs without
/// documents exactly as it did before, and no compile is queued for it:
/// compiling `main` would not change what that request reads.
#[tokio::test]
async fn an_older_pinned_staging_revision_runs_without_documents_and_queues_nothing() {
    use oxy_app::server::api::custom_apps_staging_pin::with_staging_pin;

    let db = setup_db(Schema::All).await;
    let files = working_copy();
    let ws_id = seed_workspace_at(&db, files.path()).await;
    let revision_id = compile(&db, ws_id, files.path()).await;
    make_revision_predate_documents(&db, revision_id).await;

    let pod = context(ws_id, &nowhere(), Some(revision_id))
        .await
        .with_db(Arc::new(db.clone()));
    let patterns = analyst_patterns();
    let answer =
        with_staging_pin(Some(revision_id), pod.resolve_context_documents(&patterns)).await;

    assert_eq!(answer.expect("not a failure"), Some(vec![]));
    assert_eq!(queued_compiles(&db).await, 0);
}

/// The same older revision on the node that holds the files reads the files,
/// which is exactly what that node did before documents were compiled. No
/// regression there while a workspace waits for its next compile.
#[tokio::test]
async fn an_older_revision_on_a_node_with_the_files_reads_the_files() {
    let db = setup_db(Schema::Central).await;
    let files = working_copy();
    let ws_id = seed_workspace_at(&db, files.path()).await;
    let revision_id = compile(&db, ws_id, files.path()).await;
    make_revision_predate_documents(&db, revision_id).await;

    let owns_files = context(ws_id, files.path(), Some(revision_id)).await;
    assert_eq!(
        owns_files
            .resolve_context_documents(&analyst_patterns())
            .await
            .expect("the working copy answers"),
        Some(analyst_documents())
    );
}

/// Nothing compiled at all, on a pod with no files: retryable, never an agent
/// that simply has no documents.
#[tokio::test]
async fn nothing_compiled_is_retryable_not_an_agent_without_documents() {
    let db = setup_db(Schema::Central).await;
    let files = working_copy();
    let ws_id = seed_workspace_at(&db, files.path()).await;

    let pod = context(ws_id, &nowhere(), None).await;
    let err = pod
        .resolve_context_documents(&analyst_patterns())
        .await
        .expect_err("an absent workspace must not answer `Some([])`");
    assert!(err.is_unavailable(), "{err:?}");
}

/// The other side of the older-revision rule, so it cannot be satisfied by
/// always reading the disk: a revision that DID compile documents and found
/// none is authoritative, even on a node whose working copy has since gained
/// one. The agent's definition comes from that same revision.
#[tokio::test]
async fn a_revision_that_compiled_no_documents_is_empty_not_a_reason_to_read_the_disk() {
    let db = setup_db(Schema::Central).await;
    let files = working_copy();
    std::fs::remove_dir_all(files.path().join("docs")).unwrap();
    std::fs::remove_file(files.path().join("semantics/notes.md")).unwrap();
    let ws_id = seed_workspace_at(&db, files.path()).await;
    let revision_id = compile(&db, ws_id, files.path()).await;

    // Written after the compile: on disk, not in the revision.
    write(files.path(), "docs/glossary.md", GLOSSARY);

    let owns_files = context(ws_id, files.path(), Some(revision_id)).await;
    assert_eq!(
        owns_files
            .resolve_context_documents(&analyst_patterns())
            .await
            .expect("the boundary answers"),
        Some(vec![]),
        "the promoted revision is what this request reads"
    );
}
