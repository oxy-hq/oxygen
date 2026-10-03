//! The routers `oxy-app` mounts without declaring a role must not reach disk.
//!
//! A merge at a tree's ROOT has no prefix to hang a declaration on, so
//! `RoleRouter::merge_undeclared` mounts those routers and their routes fall to
//! `classify`'s FleetOk default — the one hole the type-level gate cannot close.
//!
//! That is safe today for a reason nothing was checking: neither crate reaches
//! the workspace working copy, `.git`, or the local state dir at all. Measured
//! when this file was written — `oxy-cameras` mounts 75 routes and `airhouse` 6,
//! and the count of workspace-FS accesses across both crates is zero.
//!
//! So the guard is not "classify these routes correctly", it is "keep them
//! diskless". A camera handler that started reading the working copy would be
//! served on a replica that has none, with nothing to catch it — the exact
//! failure this branch exists to make unrepresentable.

use std::path::Path;

/// Every way a handler reaches node-local state, as the route-classification
/// skill defines it.
const WORKSPACE_FS: &[&str] = &[
    "workspace_path",
    "effective_workspace_path",
    "resolve_state_dir",
    "GitClient",
    "WorkspaceManagerWorkingCopy",
    "WorkspaceRootWorkingCopy",
    "ConfigManager",
    // Creating a workspace scaffolds its working copy.
    "workspace_provisioning",
    "create_blank_workspace",
    "create_default_workspace",
];

/// Accesses a scanned crate makes behind a route it DOES declare, so the
/// FleetOk default never applies to them. `(file suffix, needle)` — per needle,
/// so the exemption cannot quietly cover a second kind of access in that file.
const DECLARED_ACCESSES: &[(&str, &str)] = &[
    // `POST /partners/{id}/orgs` creates the client org's Default workspace;
    // `oxy_api_tenancy::partner_console::route_roles()` declares it IdeOnly, and that
    // crate's own test asserts the declaration classifies.
    (
        "api-tenancy/src/partner_console/orgs.rs",
        "workspace_provisioning",
    ),
    (
        "api-tenancy/src/partner_console/orgs.rs",
        "create_default_workspace",
    ),
    // `POST /admin/orgs` creates the new org's Default workspace; the tenancy
    // crate's admin section declares it IdeOnly (`CREATE_ORG_ROLE`), and
    // `oxy-api-tenancy`'s `tests/integration/admin_route_roles.rs` asserts
    // the declaration classifies.
    ("api-tenancy/src/admin/orgs.rs", "workspace_provisioning"),
    ("api-tenancy/src/admin/orgs.rs", "create_default_workspace"),
    // `POST /{workspace_id}/source-uploads/reports` reads the pipeline
    // definition through `ConfigManager`, which owns the compiled-vs-disk
    // choice and pins the request's revision; `oxy_api_source_upload::
    // route_roles()` declares the route FleetOk. Today the call is spelled as a
    // field (`workspace.config_manager`), so nothing matches — this reserves the
    // slot so naming the type later does not read as a new disk access. The
    // working-copy needles still apply to the file.
    ("api-source-upload/src/source_upload.rs", "ConfigManager"),
];

/// Every `.rs` file under `dir` — or `dir` itself, when it names one file.
fn rust_sources(dir: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if dir.is_file() {
        if let Ok(body) = std::fs::read_to_string(dir) {
            out.push((dir.display().to_string(), body));
        }
        return out;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(rust_sources(&path));
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs")
            && let Ok(body) = std::fs::read_to_string(&path)
        {
            out.push((path.display().to_string(), body));
        }
    }
    out
}

#[test]
fn the_crates_mounted_without_a_declaration_never_touch_the_working_copy() {
    let mut offenders = Vec::new();
    let mut files_scanned = 0;

    for crate_dir in [
        "../cameras/src",
        "../airhouse/src",
        // Declared FleetOk wholesale by the `/admin` wildcard in global.rs, so a
        // route added under one inherits FleetOk without anyone deciding. Measured
        // diskless when those declarations were written; the reconcile config
        // read under `/admin/workspace-health` runs on the WORKER, not on the
        // request path, which is why it is not an exception here.
        // Extracted from `src/server/api/partner_console` into a sibling crate,
        // now the `partner_console` module of `oxy-api-tenancy`. ONLY that
        // module: the crate's `onboarding` module clones and scaffolds on purpose
        // and declares every route IdeOnly, so scanning it here would be wrong.
        // The guard asserts its own sources are non-empty precisely so a move
        // like that fails loudly instead of silently covering nothing. Merged
        // at the root with no prefix; the one route that needs the ide is in
        // its `route_roles()`, and exempted by `DECLARED_ACCESSES`.
        "../api-tenancy/src/partner_console",
        // The rest of tenancy, moved out of `oxy-app`'s `build_global_routes`.
        // There the routes were `route_fleet`, whose state type refused a
        // working-copy extractor at compile time; as plain axum routes merged
        // through the seam they lost that gate, so this scan is what is left.
        // All FleetOk except `POST /admin/orgs`, exempted above.
        "../api-tenancy/src/organizations",
        "../api-tenancy/src/org_teams",
        "../api-tenancy/src/admin",
        // One file, not a directory: `PUT/DELETE /orgs/{org_id}/logo`. Logo
        // bytes live in Postgres; this is the only guard left on that.
        "../api-tenancy/src/org_logo.rs",
        // Extracted from `src/server/api/documents`, merged at the root the same
        // way. Postgres + presigned S3 everywhere; `POST /documents/ask` resolves
        // an agent config out of the working copy (through the project context,
        // not a needle below) and is declared IdeOnly in its `route_roles()`.
        "../api-documents/src",
        // Extracted from `src/server/api/frontline*.rs`. Its routes ARE declared
        // (`route_roles` / `public_route_roles`), but every declaration is a
        // hand-written FleetOk: leaving `RoleRouter::route_fleet` lost the type
        // gate that stopped one of these handlers taking a working copy, so
        // "stays diskless" is the check that is left.
        "../api-frontline/src",
        // Extracted from `src/server/api/source_upload.rs`, merged inside the
        // `/{workspace_id}` nest. Declared FleetOk in its `route_roles()`: it
        // writes to S3 and reads the pipeline definition through
        // `ConfigManager`, which owns the compiled-vs-disk choice — the handler
        // itself never reaches for a working copy.
        "../api-source-upload/src",
        "src/server/api/billing",
    ] {
        let sources = rust_sources(Path::new(crate_dir));
        assert!(
            !sources.is_empty(),
            "no sources under {crate_dir} — the crate moved and this guard \
             stopped covering it",
        );
        files_scanned += sources.len();

        for (path, body) in sources {
            for needle in WORKSPACE_FS {
                if DECLARED_ACCESSES
                    .iter()
                    .any(|(file, declared)| path.ends_with(file) && declared == needle)
                {
                    continue;
                }
                for (index, line) in body.lines().enumerate() {
                    // A mention in prose is not an access.
                    let code = line.split("//").next().unwrap_or(line);
                    if code.contains(needle) {
                        offenders.push(format!("{path}:{} — {needle}", index + 1));
                    }
                }
            }
        }
    }

    assert!(
        files_scanned > 20,
        "scanned only {files_scanned} files — the walk broke, and a guard that \
         reads nothing passes for the wrong reason",
    );

    assert!(
        offenders.is_empty(),
        "these crates are mounted with `merge_undeclared`, so their routes are \
         served on any pod — including replicas with no working copy. One of \
         them now reaches for one:\n  {}\n\nEither drop the access, or mount \
         the router through a door that declares a role.",
        offenders.join("\n  "),
    );
}
