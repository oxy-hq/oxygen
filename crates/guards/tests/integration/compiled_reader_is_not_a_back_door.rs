//! `ConfigManager` decides which source an artifact read uses. This keeps that
//! true.
//!
//! `compiled_reader` used to be the way every handler asked "is there a compiled
//! row for this?" and then decided what to do about the answer — nine call sites
//! spelling the same three-arm match, and two of them getting it wrong in a way
//! that reported a platform fault as the customer's configuration. The reads
//! moved onto `ConfigManager`, which matches on `Origin` once and returns a
//! typed error instead of an empty list.
//!
//! Nothing stops the next handler reaching past it. This does.
//!
//! Same shape as `workspace_path_escape_hatch.rs`: a source scan with an
//! allowlist where each entry carries its reason, and where a STALE entry fails
//! too — otherwise the list only grows and stops meaning anything.

use std::path::Path;

/// Files that may name `compiled_reader`, and why.
///
/// Two kinds only. Anything else belongs on `ConfigManager`.
///
///   1. **Resolving the request's `Origin`** — runs before a manager exists, and
///      needs the process role plus a `git` call for the default branch, neither
///      of which belongs in `crates/core`.
///   2. **No manager to have** — the compile worker checking a revision it just
///      wrote, and two decisions taken before there is anything to read from.
const ALLOWED: &[(&str, &str)] = &[
    (
        // `oxy-workspace-runtime`'s since 2026-11-22 (W7), was
        // `src/server/api/middlewares/workspace_context.rs`.
        "../workspace-runtime/src/request/workspace_context.rs",
        "Resolves the revision the whole request is pinned to, once, and stamps \
         it into `Origin`. This is the source of the answer every other read \
         uses.",
    ),
    (
        "../workspace-runtime/src/driver_ctx.rs",
        "Builds a manager outside the HTTP path, so it resolves the revision the \
         same way the middleware does. (Was `src/server/router/recovery.rs` \
         until the driver's context builders went down into \
         `oxy-workspace-runtime`, 2026-11-23.)",
    ),
    (
        "../workspace-runtime/src/preagg_workspace.rs",
        "Same: the pre-aggregation cycle builds its manager on the worker fleet, \
         off the HTTP path. Without the resolved revision the builder reads the \
         worker's absent working copy and self-heal compiles every heartbeat. \
         (Was `src/server/preagg_workspace.rs` until it went down into \
         `oxy-workspace-runtime`, 2026-11-22.)",
    ),
    (
        "../workspace-runtime/src/custom_apps_gates.rs",
        "Same: builds the per-request project context for the public custom-app \
         router, which has no workspace middleware. (Was \
         `src/server/api/custom_apps_gates.rs` until it went down into \
         `oxy-workspace-runtime`, 2026-11-22.)",
    ),
    (
        "../app-core/src/staging_pin_scope.rs",
        "`with_staging_pin` scopes the revision a STAGING custom-app request \
         resolves at (the draft build's pin), before the gates build the \
         manager, and a workspace preview pins the same way. Lowered with \
         `compiled_reader` so `preagg_context` can read the pin. It picks the \
         revision; it reads no artifact.",
    ),
    (
        "../api-world-model/src/toast_webhook.rs",
        "Same, on the public webhook router (`oxy-api-world-model`'s since \
         2026-11-15, was `src/server/api/webhooks/toast.rs`).",
    ),
    (
        "../api-slack/src/workspace.rs",
        "Same, off the HTTP path entirely.",
    ),
    (
        "../workspace-runtime/src/scan_source.rs",
        "Doc comment only — explains where branch semantics come from. If this \
         ever becomes a call, it is a bug: the caller has a manager. (Was \
         `src/server/api/semantic.rs` until the scan resolution moved down, \
         2026-11-16, then `src/agentic_wiring/scan_source.rs` until it went \
         into `oxy-workspace-runtime`, 2026-11-22.)",
    ),
    (
        "../workspace-runtime/src/compile_dispatcher.rs",
        "The compile worker's `CompiledConfigReader` adapter. The worker \
         reconciles the health and pre-aggregation schedule rows from the \
         compiled config right after its own promote, and `oxy-server`'s \
         periodic driver does the same off the HTTP path; neither has a \
         manager. (The worker, `../compile-runtime/src/worker.rs`, named \
         `compiled_reader` itself until it took these reads through \
         `oxy_compile_runtime::ports`, depth stage C, 2026-11-27.)",
    ),
    (
        "../app-core/src/pods/servability.rs",
        "Decides whether a replica can serve a workspace's custom-app function \
         locally or must relay it to the Factory. A placement decision, taken \
         before there is a manager.",
    ),
    (
        "../workspace-health/src/smoke/config.rs",
        "Reads the compiled config BEFORE building a workspace context, \
         deliberately — a promoted revision with no `smoke_test` block is \
         authoritative, and building a context to re-learn that on every eval \
         pass is the cost this avoids.",
    ),
    (
        "../workspace-runtime/src/preview_ctx.rs",
        "Builds the workspace-preview platform's manager: reads the staging \
         revision's compiled config and hands it to the builder as a provided \
         config, because the builder's own load falls back to the working copy \
         (and to an EMPTY config) on a miss. Resolving the `Origin` — there is no \
         manager yet. (Was `src/agentic_wiring/preview_ctx.rs` until it went \
         down into `oxy-workspace-runtime`, 2026-11-22, W5.)",
    ),
    (
        "../api-custom-apps/src/custom_apps_functions/preflight.rs",
        "Runs inside `oxy migrate`, before any server or manager exists, and takes \
         the `databases` key from the compiled config of every workspace that has \
         a live custom app — the promoted revision the serve fleet reads. Building \
         a workspace context per workspace in a deploy hook to learn a list of \
         names is the cost this avoids.",
    ),
    (
        "../api-custom-apps/src/custom_apps_nonproduction/mod.rs",
        "Publish checks a build's `nonProduction.destinations` against the \
         `databases` key of the promoted revision's compiled config; publish \
         has no manager, and building one would read the node's working copy. \
         The host re-checks every staging write against its manager's config.",
    ),
];

/// Every `.rs` under `oxy-app-core` (where `compiled_reader`
/// itself now lives), `oxy-custom-apps`, `oxy-api-custom-apps` (the custom-app
/// vertical lifted out of `src/server/api`) and `oxy-api-documents` and
/// `oxy-api-airway`, `oxy-api-notifications`, `oxy-api-slack`,
/// `oxy-api-operating-graph`, `oxy-api-metric-monitoring`,
/// `oxy-api-observability`, `oxy-api-automations`,
/// `oxy-api-workspace-config` and
/// `oxy-api-world-model` (with `oxy-world-model`, where the graph's cores
/// went down to), and workspace health's
/// two crates, extracted from oxy-app's
/// `src/server/api` and still served
/// by this binary, and the rest walked below — as
/// (`../<crate>/src/…` path, contents).
fn source_files() -> Vec<(String, String)> {
    fn walk(dir: &Path, root: &Path, label: &str, out: &mut Vec<(String, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, root, label, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs")
                && let Ok(body) = std::fs::read_to_string(&path)
            {
                let rel = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((format!("{label}{rel}"), body));
            }
        }
    }
    // oxy-app's own `src` was walked first, labelled `src/…`, until decompose
    // DEL deleted the crate (2026-11-25); every tree below is a sibling crate.
    let documents = crate::guards_root().join("../api-documents/src");
    let mut out = Vec::new();
    walk(&documents, &documents, "../api-documents/src/", &mut out);
    let app_core = crate::guards_root().join("../app-core/src");
    walk(&app_core, &app_core, "../app-core/src/", &mut out);
    let custom_apps = crate::guards_root().join("../custom-apps/src");
    walk(&custom_apps, &custom_apps, "../custom-apps/src/", &mut out);
    let api_custom_apps = crate::guards_root().join("../api-custom-apps/src");
    walk(
        &api_custom_apps,
        &api_custom_apps,
        "../api-custom-apps/src/",
        &mut out,
    );
    let source_upload = crate::guards_root().join("../api-airway/src");
    walk(
        &source_upload,
        &source_upload,
        "../api-airway/src/",
        &mut out,
    );
    let notifications = crate::guards_root().join("../api-notifications/src");
    walk(
        &notifications,
        &notifications,
        "../api-notifications/src/",
        &mut out,
    );
    let slack = crate::guards_root().join("../api-slack/src");
    walk(&slack, &slack, "../api-slack/src/", &mut out);
    let operating_graph = crate::guards_root().join("../api-operating-graph/src");
    walk(
        &operating_graph,
        &operating_graph,
        "../api-operating-graph/src/",
        &mut out,
    );
    // `oxy-api-previews` (2026-11-14): the previews' upper half.
    let previews = crate::guards_root().join("../api-previews/src");
    walk(&previews, &previews, "../api-previews/src/", &mut out);
    // `oxy-api-metric-monitoring` (2026-11-15): the metric tree and the
    // anomaly inbox, whose scan reads `.monitor.yml` through the compile boundary.
    let metric_monitoring = crate::guards_root().join("../api-metric-monitoring/src");
    walk(
        &metric_monitoring,
        &metric_monitoring,
        "../api-metric-monitoring/src/",
        &mut out,
    );
    // `oxy-api-observability` (2026-11-15): the observability console, which
    // reads the ClickHouse product store.
    let observability = crate::guards_root().join("../api-observability/src");
    walk(
        &observability,
        &observability,
        "../api-observability/src/",
        &mut out,
    );
    // `oxy-api-automations` (2026-11-15): the compile-boundary automation
    // listing and the schedule CRUD.
    let automations = crate::guards_root().join("../api-automations/src");
    walk(
        &automations,
        &automations,
        "../api-automations/src/",
        &mut out,
    );
    // `oxy-api-workspace-config` (2026-11-15): the workspace's databases and
    // secrets, and (2026-11-21) the OAuth connects out of `src/integrations`,
    // walked above (under `src`) before the move.
    let workspace_config = crate::guards_root().join("../api-workspace-config/src");
    walk(
        &workspace_config,
        &workspace_config,
        "../api-workspace-config/src/",
        &mut out,
    );
    // `oxy-api-tenancy`: the org subdomain's status, the logo and the consent
    // switch moved there from `src/server/api` on 2026-11-16. The whole
    // crate, since none of it names `compiled_reader`.
    let tenancy = crate::guards_root().join("../api-tenancy/src");
    walk(&tenancy, &tenancy, "../api-tenancy/src/", &mut out);
    // `oxy-server`, the whole crate: the health probes and route discovery
    // (`core_routes`) moved UP there from `src/server/api` on 2026-11-16, and
    // the run drivers and the periodic tick (`drivers`) from `src/server/router`
    // on 2026-11-23 (decompose BL1). None of it names `compiled_reader`.
    let server = crate::guards_root().join("../server/src");
    walk(&server, &server, "../server/src/", &mut out);
    // `oxy-api-threads`: the conversation surface moved there from
    // `src/server/api` on 2026-11-16. The whole crate.
    let threads = crate::guards_root().join("../api-threads/src");
    walk(&threads, &threads, "../api-threads/src/", &mut out);
    // `oxy-api-workspaces`: the workspaces surface moved there from
    // `src/server/api` on 2026-11-16. The whole crate.
    let workspaces = crate::guards_root().join("../api-workspaces/src");
    walk(&workspaces, &workspaces, "../api-workspaces/src/", &mut out);
    // `oxy-api-semantic`: the semantic surface moved there from
    // `src/server/api` on 2026-11-16. The whole crate.
    let semantic = crate::guards_root().join("../api-semantic/src");
    walk(&semantic, &semantic, "../api-semantic/src/", &mut out);
    let world_model = crate::guards_root().join("../api-world-model/src");
    walk(
        &world_model,
        &world_model,
        "../api-world-model/src/",
        &mut out,
    );
    let world_model_domain = crate::guards_root().join("../world-model/src");
    walk(
        &world_model_domain,
        &world_model_domain,
        "../world-model/src/",
        &mut out,
    );
    let health = crate::guards_root().join("../workspace-health/src");
    walk(&health, &health, "../workspace-health/src/", &mut out);
    // `oxy-usage-report` (2026-11-20): the usage report's domain half, out of
    // `src/server/api/admin/usage_report`, and the usage trio out of
    // `../app-core/src`, both walked above before the move.
    let usage_report = crate::guards_root().join("../usage-report/src");
    walk(
        &usage_report,
        &usage_report,
        "../usage-report/src/",
        &mut out,
    );
    // `oxy-api-billing` (admin split PR 8b): the usage report's surface, out
    // of `src/server/api/admin/usage_report`, walked above before the move.
    let billing = crate::guards_root().join("../api-billing/src");
    walk(&billing, &billing, "../api-billing/src/", &mut out);
    // `oxy-preagg` (2026-11-20): the pre-aggregation runtime's domain half,
    // out of `src/server/preagg_{freshness,generation,rebuild,retract}`, and
    // the post-promote nudge (`src/server/preagg_promote.rs`) the same day,
    // walked above before the moves.
    let preagg = crate::guards_root().join("../preagg/src");
    walk(&preagg, &preagg, "../preagg/src/", &mut out);
    // `oxy-workspace-runtime` (2026-11-21): the agentic wiring's leaves, out
    // of `src/agentic_wiring`, and `src/server/{runtime_artifact,
    // api/typed_stream}.rs`, all walked above before the move; then (W1b,
    // 2026-11-22) `project_ctx`'s connector half, `sql_exec`, `sql_http` and
    // `src/server/preagg_workspace.rs`.
    let runtime = crate::guards_root().join("../workspace-runtime/src");
    walk(&runtime, &runtime, "../workspace-runtime/src/", &mut out);
    // `oxy-compile-runtime` (2026-11-21): the compile boundary's runtime
    // leaves, out of `src/server/compile_{git,oltp,config_gate,maintenance}`,
    // walked above before the move.
    let compile_runtime = crate::guards_root().join("../compile-runtime/src");
    walk(
        &compile_runtime,
        &compile_runtime,
        "../compile-runtime/src/",
        &mut out,
    );
    // `oxy-previews` (2026-11-22): workspace previews' rows, out of
    // `src/server/previews` and `src/server/api/projects/automation_run/
    // preview_settle.rs`, walked above before the move.
    let previews_rows = crate::guards_root().join("../previews/src");
    walk(&previews_rows, &previews_rows, "../previews/src/", &mut out);
    // `oxy-feature-flags` (2026-11-20): the feature-flag runtime's domain
    // half, out of `src/server/feature_flags`, walked above before the move.
    let flags = crate::guards_root().join("../feature-flags/src");
    walk(&flags, &flags, "../feature-flags/src/", &mut out);
    // `oxy-metric-monitoring` (2026-11-21): insights delivery's enqueue, out
    // of `src/server/anomaly_notify`, walked above before the move. (Its
    // executor went to `../api-metric-monitoring/src`, walked above too.)
    let monitoring = crate::guards_root().join("../metric-monitoring/src");
    walk(
        &monitoring,
        &monitoring,
        "../metric-monitoring/src/",
        &mut out,
    );
    // `oxy-api-eval` (2026-11-20): `oxy test`, its event handler, the
    // reporters and the engine, out of `src/cli/commands/test.rs`,
    // `src/server/service/eval.rs` and `src/integrations/eval/`, all walked
    // above before the move.
    let eval = crate::guards_root().join("../api-eval/src");
    walk(&eval, &eval, "../api-eval/src/", &mut out);
    // `oxy-api-admin` (2026-11-21): the feature-flag routes, out of
    // `src/server/feature_flags/routes.rs`, walked above before the move,
    // with the rest of the staff console crate.
    let admin = crate::guards_root().join("../api-admin/src");
    walk(&admin, &admin, "../api-admin/src/", &mut out);
    let api_health = crate::guards_root().join("../api-workspace-health/src");
    walk(
        &api_health,
        &api_health,
        "../api-workspace-health/src/",
        &mut out,
    );
    out
}

fn mentions_compiled_reader(body: &str) -> bool {
    body.contains("compiled_reader::")
}

#[test]
fn nothing_new_reaches_past_config_manager() {
    let offenders: Vec<String> = source_files()
        .into_iter()
        .filter(|(rel, body)| {
            rel != "../app-core/src/compiled_reader.rs"
                && mentions_compiled_reader(body)
                && !ALLOWED.iter().any(|(f, _)| f == rel)
        })
        .map(|(rel, _)| rel)
        .collect();

    assert!(
        offenders.is_empty(),
        "these files name `compiled_reader` and are not on the list:\n  {}\n\n\
         Reading an artifact through `compiled_reader` means deciding \
         compiled-vs-disk at the call site, which is what `ConfigManager` is \
         for. Before adding an entry:\n\
         \x20 1. Does `ConfigManager` already answer this? `list_apps`, \
         `resolve_app`, `list_automations`, `automation_definition`, \
         `monitor_config`, `semantics_scan` and friends cover every artifact \
         kind, and each owns BOTH arms — there is no `compiled_*` left to \
         call.\n\
         \x20 2. Do you have a manager? If the caller has a `WorkspaceManager`, \
         use it — the middleware already pinned its revision.\n\
         \x20 3. If you genuinely have no manager (you are resolving the \
         `Origin`, or checking a revision you just wrote), add the file WITH the \
         reason.",
        offenders.join("\n  ")
    );
}

/// Without this the allowlist only ever grows. A stale entry is worse than a
/// missing one: it reads as "this file needs the escape hatch" long after it
/// stopped needing it, and the next person copies it.
#[test]
fn the_list_has_no_stale_entries() {
    let files = source_files();
    let stale: Vec<&str> = ALLOWED
        .iter()
        .map(|(f, _)| *f)
        .filter(|f| {
            !files
                .iter()
                .any(|(rel, body)| rel == f && mentions_compiled_reader(body))
        })
        .collect();

    assert!(
        stale.is_empty(),
        "these are on the list but no longer name `compiled_reader` — remove \
         them:\n  {}",
        stale.join("\n  ")
    );
}

/// The counter-guard: if someone "fixes" the test above by deleting the module,
/// this fails. `compiled_reader` still owns the one question `ConfigManager`
/// cannot answer — which revision this request reads — and that has to keep
/// living somewhere the process role and `git` are reachable.
#[test]
fn the_origin_resolver_still_exists() {
    let path = crate::guards_root().join("../app-core/src/compiled_reader.rs");
    let body =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    for expected in [
        "pub async fn resolve_request_revision",
        "pub async fn with_pinned_revision",
    ] {
        assert!(
            body.contains(expected),
            "`{expected}` should still live in `compiled_reader` — resolving the \
             request's revision needs the process role and a `git` call for the \
             default branch, so it cannot move into `crates/core` with the \
             queries."
        );
    }
}

/// The servability check's validity cache is one instance, in
/// `oxy::config::servability`, and the resolver here reads it through
/// `revision_config_loads`. A second memo in `compiled_reader` would let the
/// promoted-revision pin and the preview pin disagree about one revision.
#[test]
fn the_resolver_shares_the_one_validity_cache() {
    let path = crate::guards_root().join("../app-core/src/compiled_reader.rs");
    let body =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    assert!(
        body.contains("oxy::config::servability::{load_config_value, revision_config_loads}"),
        "`compiled_reader` should take `revision_config_loads` from \
         `oxy::config::servability`, the one home of the validity cache"
    );
    assert!(
        !body.contains("fn config_validity_cache")
            && !body.contains("OnceLock<Mutex<HashMap<Uuid, bool>>>"),
        "`compiled_reader` must not keep its own validity cache: the one instance \
         lives in `oxy::config::servability`"
    );
    let home = crate::guards_root().join("../core/src/config/servability.rs");
    let home =
        std::fs::read_to_string(&home).unwrap_or_else(|e| panic!("read {}: {e}", home.display()));
    assert_eq!(
        home.matches("fn config_validity_cache").count(),
        1,
        "exactly one validity cache, in `oxy::config::servability`"
    );
    // The pin rule left the walk above with it (`crates/core` is not walked:
    // it is `ConfigManager`'s home), so the moved file is checked here. `oxy`
    // sits below `oxy-app-core` and cannot call `compiled_reader`; this keeps
    // a re-export or a doc from naming it without a reason either.
    assert!(
        !mentions_compiled_reader(&home),
        "`oxy::config::servability` should not name `compiled_reader`: it was \
         `../app-core/src/revision_pin.rs`, whose allowlist entry went with the move"
    );
}
