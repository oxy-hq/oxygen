use super::*;
use axum::http::StatusCode;
use axum::response::IntoResponse;

/// A stale label is a conflict with the workspace's state; a value that is not
/// a label at all is still a bad request. (Moved from
/// `api::workspaces::detached_tests` with the function, 2026-11-16.)
#[test]
fn only_a_well_formed_label_is_a_conflict() {
    assert_eq!(
        unresolved_branch_status(Some("HEAD@abc1234")),
        StatusCode::CONFLICT
    );
    for value in [Some("HEAD@../../etc"), Some("has@symbol"), Some(""), None] {
        assert_eq!(
            unresolved_branch_status(value),
            StatusCode::BAD_REQUEST,
            "{value:?}"
        );
    }
}

/// The boundary-miss 503 has to speak the protocol the frontend listens on.
///
/// `shouldRetryWorkspaceQuery` keys the 24-retry spinner off
/// `x-oxy-unavailable: workspace-materializing`. This response carried only
/// `x-oxy-needs-recompile`, which nothing in the frontend reads, so it took
/// the three-retry path and surfaced as a generic load failure — for a
/// workspace whose compile had already been enqueued.
#[test]
fn the_boundary_miss_503_asks_the_frontend_to_wait() {
    let workspace_id = Uuid::new_v4();
    let response = WorkspaceManagerMissing {
        needs_recompile: Some(workspace_id),
    }
    .into_response();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response.headers().get("x-oxy-unavailable").unwrap(),
        "workspace-materializing"
    );
    assert_eq!(response.headers().get("retry-after").unwrap(), "5");
    assert_eq!(
        response.headers().get("x-oxy-needs-recompile").unwrap(),
        &workspace_id.to_string()
    );
}

/// The other flavour is not transient — the workspace path or config.yml is
/// unreachable and retrying changes nothing. Asking for a spinner there
/// would hide a real fault behind two minutes of polling.
#[test]
fn the_unreachable_workspace_503_does_not_ask_the_frontend_to_wait() {
    let response = WorkspaceManagerMissing {
        needs_recompile: None,
    }
    .into_response();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(response.headers().get("x-oxy-unavailable").is_none());
    assert!(response.headers().get("retry-after").is_none());
}

/// EVERY middleware that publishes a workspace manager must publish BOTH
/// extensions. `WorkspaceManagerReadOnly` reads its own — deliberately, so
/// a pod can stop publishing the disk one — which means a middleware that
/// inserts only `WorkingCopy` leaves every read-only handler with nothing
/// to find, and its extractor answers a missing extension with 503.
///
/// `local_context` did exactly that, and nothing in the type system or the
/// suite noticed: `--local` is the mode the agentic browser flows boot, so
/// the first sign was four of them failing in CI with 503s from the moment
/// the server came up. A source scan is crude, but it is the only thing
/// here that can see "this file publishes one and not the other".
#[test]
fn every_context_middleware_publishes_both_managers() {
    for (name, src) in [
        ("workspace_context.rs", include_str!("workspace_context.rs")),
        (
            "local_context.rs",
            include_str!("../../../app/src/server/api/middlewares/local_context.rs"),
        ),
    ] {
        let body = &src[..src.find("#[cfg(test)]").unwrap_or(src.len())];
        assert!(
            body.contains(".insert(workspace_manager)") || body.contains(".insert(manager)"),
            "{name} should publish the WorkingCopy manager"
        );
        assert!(
            body.contains("into_read_only()"),
            "{name} publishes a `WorkspaceManager<WorkingCopy>` but no \
             `ReadOnly` one. The read-only extractor reads its OWN \
             extension rather than downgrading the disk one, so every \
             handler taking `WorkspaceManagerReadOnly` gets a 503 on this \
             surface — from the first request, with nothing in the type \
             system to say so."
        );
    }
}

/// The publish gate, which is the one change in this area that no compiler
/// checks: `workspace_middleware` inserts the `WorkingCopy` extension only
/// on a process that owns the files.
///
/// Asserted on the extension map rather than through the extractor, because
/// the map IS the mechanism — `WorkspaceManagerWorkingCopy` ignores the
/// state it is handed and reads this one type. That the extractor rejects a
/// missing extension is covered by
/// `read_only_resolves_without_the_disk_manager` beside it.
#[tokio::test]
async fn a_diskless_process_publishes_no_working_copy_extension() {
    let dir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(dir.path().join("config.yml"), "models: []\ndatabases: []\n")
        .await
        .expect("write config");
    let manager = WorkspaceBuilder::new(Uuid::new_v4())
        .with_working_copy(dir.path(), None, oxy::config::OnMissing::Empty)
        .await
        .expect("builder")
        .build()
        .await
        .expect("manager");

    // Exactly what the middleware does, both ways round.
    let publish = |owns_files: bool| {
        let mut ext = axum::http::Extensions::new();
        ext.insert(manager.clone().into_read_only());
        if owns_files {
            ext.insert(manager.clone());
        }
        ext
    };

    let ide = publish(true);
    assert!(
        ide.get::<WorkspaceManager<WorkingCopy>>().is_some(),
        "the ide owns its files, so a handler that requires them gets them"
    );

    let replica = publish(false);
    assert!(
        replica.get::<WorkspaceManager<ReadOnly>>().is_some(),
        "a read-only handler keeps working — that is the whole point of the split"
    );
    assert!(
        replica.get::<WorkspaceManager<WorkingCopy>>().is_none(),
        "a handler that REQUIRES the files must be refused at the door, not \
         handed a manager rooted at a directory that is not here"
    );
}

/// The two extractors read two extensions, so a pod can publish the
/// capability-free manager without publishing the disk one. Until that
/// split existed, `WorkspaceManagerReadOnly` downgraded from `WorkingCopy` and would
/// have died with it — taking every handler that never needed a disk.
#[tokio::test]
async fn read_only_resolves_without_the_disk_manager() {
    let dir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(dir.path().join("config.yml"), "models: []\ndatabases: []\n")
        .await
        .expect("write config");
    let manager = WorkspaceBuilder::new(Uuid::new_v4())
        .with_working_copy(dir.path(), None, oxy::config::OnMissing::Empty)
        .await
        .expect("builder")
        .build()
        .await
        .expect("manager");

    let mut parts = axum::http::Request::builder()
        .body(())
        .expect("request")
        .into_parts()
        .0;
    parts.extensions.insert(manager.into_read_only());

    assert!(
        WorkspaceManagerReadOnly::from_request_parts(&mut parts, &())
            .await
            .is_ok(),
        "the capability-free manager is all a read-only handler needs"
    );
    assert!(
        WorkspaceManagerWorkingCopy::from_request_parts(
            &mut parts,
            &oxy_app_core::pods::IdeState(oxy_app_core::app_state::bare_app_state()),
        )
        .await
        .is_err(),
        "a handler asking for a disk must be told there isn't one, not handed an empty one"
    );
}

/// An ordinary status denial stays opaque — same bytes as before this
/// type existed, so nothing that mapped a bare code changes shape.
#[test]
fn access_error_status_passes_through_unchanged() {
    let response = WorkspaceAccessError::Status(StatusCode::NOT_FOUND).into_response();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(
        response.headers().get("x-oxy-assume-required").is_none(),
        "a plain status denial must not advertise assume-role"
    );
}

/// The staff denial explains itself. This is the contract the FE keys off
/// to swap a dead 403 for the assume-role dialog — the header carries the
/// org that must be assumed, so the dialog opens pre-scoped.
#[test]
fn access_error_assume_required_advertises_org_to_assume() {
    let workspace_id = Uuid::new_v4();
    let org_id = Uuid::new_v4();
    let response = WorkspaceAccessError::AssumeRequired {
        workspace_id,
        org_id,
        org_name: Some("Pokehouse".to_string()),
    }
    .into_response();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let header = response
        .headers()
        .get("x-oxy-assume-required")
        .expect("assume-required denial must carry the org header")
        .to_str()
        .expect("header is ascii");
    assert_eq!(
        header,
        org_id.to_string(),
        "header must name the org to assume, not the workspace"
    );
}

/// Generic 503 — no header, no workspace_id in the body — so legacy
/// behavior is preserved when the workspace genuinely has no path.
#[test]
fn rejection_not_available_has_no_recompile_header() {
    let response = WorkspaceManagerMissing {
        needs_recompile: None,
    }
    .into_response();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        response.headers().get("x-oxy-needs-recompile").is_none(),
        "legacy NotAvailable must not carry the recompile header"
    );
}

/// Serve-mode short-circuit carries the `X-Oxy-Needs-Recompile`
/// header keyed by workspace_id so the FE can route the toast and
/// reload-after-compile UX accordingly.
#[test]
fn rejection_needs_recompile_carries_workspace_header() {
    let workspace_id = Uuid::new_v4();
    let response = WorkspaceManagerMissing {
        needs_recompile: Some(workspace_id),
    }
    .into_response();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let header = response
        .headers()
        .get("x-oxy-needs-recompile")
        .expect("NeedsRecompile rejection must set the header");
    assert_eq!(header.to_str().unwrap(), workspace_id.to_string());
}

/// Collapse whitespace, and close the gap after an opening paren, so an
/// assertion about a call site reads the same however rustfmt broke it.
fn one_line(src: &str) -> String {
    src.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("( ", "(")
}

/// Each handler family must keep naming the source that matches how it
/// resolves its scan root.
///
/// `get_or_load`'s first argument is one autocomplete away from wrong at
/// every call site, and wrong there re-opens the collision the key closed —
/// *silently*, because the layer still loads; it is just the other root's.
/// So assert at the call site: a file-wide check could not tell "keyed
/// correctly" from "nobody mentioned it".
#[test]
fn layer_sources_match_the_handler_family() {
    // The boundary readers key on the revision their resolver reports.
    // A literal at one of their call sites cannot track the fallback arms
    // (`materialise_semantic_entity` returning None for an unpromoted file,
    // a `scan_dir` error), each of which yields a working-copy path while
    // the manager is still pinned to a revision.
    //
    // The world-model family is one of them now: it scanned
    // `semantics_scan_path()` unconditionally and so passed `None`, until
    // its routes became `FleetOk` and its scan moved to
    // `graph::source` (now `oxy-api-world-model`'s).
    for (name, src) in [
        // The semantic explorer and the pre-aggregation status and
        // rebuild, lifted into `oxy-api-semantic` (2026-11-16).
        (
            "oxy-api-semantic semantic.rs",
            include_str!("../../../api-semantic/src/semantic.rs"),
        ),
        (
            "oxy-api-semantic preagg.rs",
            include_str!("../../../api-semantic/src/preagg.rs"),
        ),
        // The IDE handlers, lifted into `oxy-api-world-model`.
        (
            "oxy-api-world-model graph/handlers.rs",
            include_str!("../../../api-world-model/src/graph/handlers.rs"),
        ),
        // The graph cores, lowered into `oxy-world-model`: the IDE
        // handlers above hand them the engine they keyed.
        (
            "oxy-world-model graph/query.rs",
            include_str!("../../../world-model/src/graph/query.rs"),
        ),
        (
            "oxy-world-model graph/instances.rs",
            include_str!("../../../world-model/src/graph/instances.rs"),
        ),
        (
            "oxy-world-model graph/breakdown.rs",
            include_str!("../../../world-model/src/graph/breakdown.rs"),
        ),
        (
            "oxy-api-world-model graph/source.rs",
            include_str!("../../../api-world-model/src/graph/source.rs"),
        ),
    ] {
        let flat = one_line(src);
        assert!(
            !flat.contains("get_or_load(None,"),
            "{name} resolves its scan root through the compile boundary, so it must key \
             on what its resolver reports (scan_source_revision / \
             QueryScanSource::source_revision), not a literal"
        );
    }
}
