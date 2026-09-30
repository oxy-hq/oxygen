//! `ctx.semantic` answers from the compiled revision, not the working copy.
//!
//! It used to load views from `semantics_scan_path()` — the raw working copy —
//! so an ide node answered from whatever was on disk and a diskless replica
//! from a directory that is not there. It now reads through
//! `semantic_scan::scan_dir`, as the `/semantic-query` route does.
//!
//! The workspace is compiled and promoted with a view whose one row says
//! `from-compiled`; then the file on disk is rewritten to say `from-disk`
//! without recompiling. A function must still read `from-compiled`.

use serde_json::json;

use crate::custom_app_functions_fixture::{
    FunctionSpec, call_function_in, publish_app, seeded_tenant, throwaway_org,
};
use crate::custom_app_functions_shape_zoo::{compile, write_config};

const LABELS_JS: &str = r#"
export default async (req, ctx) => {
  const { rows } = await ctx.semantic.query({ topic: "orders", dimensions: ["orders.label"], measures: ["orders.n"] });
  return Response.json({ rows });
};
"#;

fn write_view(root: &std::path::Path, label: &str) {
    let dir = root.join("semantics");
    std::fs::create_dir_all(&dir).expect("semantics dir");
    std::fs::write(
        dir.join("orders.view.yml"),
        format!(
            "name: orders\ndatasource: duck\nsql: |\n  SELECT '{label}' AS label\n\
             dimensions:\n- name: label\n  type: string\n  expr: label\n\
             measures:\n- name: n\n  type: count\n"
        ),
    )
    .expect("write view");
    std::fs::write(
        dir.join("orders.topic.yml"),
        "name: orders\nviews:\n- orders\n",
    )
    .expect("write topic");
}

#[tokio::test]
async fn ctx_semantic_reads_the_compiled_revision_not_the_working_copy() {
    let t = throwaway_org(&seeded_tenant().await).await;
    let root = write_config("  - name: duck\n    type: duckdb\n    path: semantic.duckdb\n");
    write_view(root.path(), "from-compiled");
    let workspace = compile(&t, root.path()).await;
    // An edit nobody compiled: what the working copy says, and live must not.
    write_view(root.path(), "from-disk");

    let slug = "semantic-revision";
    let functions = vec![FunctionSpec {
        name: "labels",
        manifest: json!({ "route": true, "timeoutSeconds": 60 }),
        js: LABELS_JS,
    }];
    publish_app(&t, slug, workspace, &functions).await;

    let call = call_function_in(&t.org_slug, slug, "labels", json!({})).await;
    let got = call
        .frame("data")
        .unwrap_or_else(|| panic!("no data frame; stream: {}", call.raw))
        .to_string();
    assert!(
        got.contains("from-compiled"),
        "ctx.semantic reads the promoted revision: {got}"
    );
    assert!(
        !got.contains("from-disk"),
        "not the uncompiled working copy: {got}"
    );
}
