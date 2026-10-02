//! Upload endpoint for file-based source reports.
//!
//! One landing zone serves every uploadable source kind — see
//! [`UPLOADABLE_SOURCE_KINDS`], which is `ubereats` today. The kind is a path
//! segment read from the pipeline definition, so adding a kind is one entry
//! there rather than a second bucket, a second env var and a second route.
//!
//! Writes a report into the S3 landing zone that airway's `ubereats` source
//! reads.
//!
//! **The bucket is server configuration, never the pipeline's.** A pipeline's
//! `base_path` is customer-editable and the pod role reaches every
//! `<cluster>-*` bucket, so trusting it would let a customer name
//! `s3://oxy-dev-customer-apps/…` and have the server write their bytes there
//! with its own credentials — a confused deputy. The bucket and root prefix
//! come from [`ZONE_VAR`]; the pipeline only ever contributes a *segment*
//! below it.
//!
//! **A pipeline does not have to declare its zone at all**, and omitting it is
//! the recommended shape: the run derives the same value from
//! [`agentic_airway::upload_zone`] that this endpoint does, so there is
//! nothing for the two to disagree about. Transcribing a bucket, a workspace
//! uuid and a path slug by hand was only ever a way to get one of them wrong.
//!
//! A *declared* zone is still checked against the one this writes to, because
//! a value someone wrote can drift: a mismatch is refused loudly, naming both
//! values, instead of landing reports where nothing reads them — see
//! [`zone_for_pipeline`]. That check is what an omitted value makes
//! unnecessary rather than what it skips.
//!
//! The key layout below is what the source derives the report **period** and
//! the row identity from, so it cannot be changed freely — see [`object_key`].
//!
//! # Why the bytes come through oxy
//!
//! The obvious alternative is a presigned PUT, the way the custom-app asset
//! store works. It is rejected here on purpose: presigning means the server
//! never sees the content, so a report whose header variant renamed a
//! JE-critical column is accepted, sits in the zone, and fails at **load**
//! time — hours later, in a pipeline run, as a "missing JE-critical column"
//! error nobody is watching for. Monthly payment-details reports are small, so
//! the cost of streaming them is a rounding error against catching that at the
//! moment someone can still fix it.
//!
//! # Why validation runs the real source
//!
//! Everything needed to validate — the 49-column map, the JE-critical set,
//! header detection, period derivation — is `pub(crate)` inside airway. Copying
//! any of it here would put two copies of a contract in two repos on two
//! release cadences, which is the drift this codebase has been bitten by
//! repeatedly. `agentic_airway::report_validation` instead runs the real source
//! over the bytes: validation *by execution* of the shipped code path, so it
//! cannot disagree with what the loader will do.
//!
//! It lives in `agentic-airway` rather than here because that crate owns oxy's
//! dependency on the engine — this handler is transport, and linking `airway`
//! into the app crate would widen that coupling for no gain.

use agentic_airway::report_validation::{
    ReportValidationError, check_period, validate_ubereats_report,
};
use axum::extract::{Multipart, Path};
use axum::http::StatusCode;
use serde::Serialize;
use uuid::Uuid;

use oxy_app::surface::WorkspaceManagerReadOnly;
use oxy_app::surface::role_guards::WorkspaceEditor;

/// Body ceiling for an upload.
///
/// Enforced by a `DefaultBodyLimit` layer on the route's `Router`, NOT by the
/// check in the handler: axum applies its own 2 MiB default otherwise, so a
/// 3 MiB report fails inside `field.bytes()` as a `400 length limit exceeded`
/// and the handler's check is never reached. The in-handler check remains as a
/// backstop for a caller that streams past the layer, and it runs *after* the
/// part is buffered — it bounds what is stored, not what is read.
///
/// A monthly payment-details report for one store is well under a megabyte;
/// 64 MiB is far above any real one and still bounded, which matters because
/// the body is buffered to validate it.
pub const MAX_REPORT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Serialize)]
pub struct UploadedReport {
    /// Where it landed, as a URL — the same spelling the pipeline's `base_path`
    /// uses, so an operator can see at a glance that the two agree.
    pub location: String,
    /// The period the loader will stamp on every row, derived here rather than
    /// reported back from the loader, so a mismatch surfaces now.
    pub report_year: i64,
    pub report_month: u32,
    /// Rows the report yielded under validation. Zero is a successful upload of
    /// an empty report, which is worth seeing.
    pub rows: usize,
}

/// The landing zone, as a URL — `s3://bucket/prefix`.
///
/// **Server configuration, deliberately.** This is the one value a customer
/// cannot edit: the pod role reaches every `<cluster>-*` bucket, so a
/// customer-supplied bucket name would be a confused deputy — the server
/// writing customer bytes into a sibling service's bucket with its own
/// credentials.
pub const ZONE_VAR: &str = agentic_airway::upload_zone::ZONE_VAR;

/// The configured zone split into `(bucket, root_prefix)`.
///
/// Parsing lives in [`agentic_airway::upload_zone`] so the run path derives
/// the identical zone; only the status-code mapping is this layer's business.
/// Every variant reachable here is a server-configuration fault, hence 503.
fn zone() -> Result<(String, String), (StatusCode, String)> {
    agentic_airway::upload_zone::zone_from_env()
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e.to_string()))
}

/// Check the pipeline agrees with where we are about to write, and return the
/// segment it contributes.
///
/// The pipeline does NOT choose the bucket — see [`ZONE_VAR`]. It is read for
/// two things only: that it really is a `ubereats` pipeline, and that its
/// `base_path` names the same place this is about to write. The second check
/// is what replaces the old unenforced "keep these equal" instruction: a drift
/// now fails the upload naming both values, instead of landing reports
/// somewhere no loader looks.
async fn zone_for_pipeline(
    workspace: &oxy::adapters::workspace::manager::WorkspaceManager<impl oxy::config::DiskSlot>,
    workspace_id: Uuid,
    pipeline_ref: &str,
    bucket: &str,
    root: &str,
    pipeline_slug: &str,
) -> Result<(String, Option<std::collections::HashSet<String>>), (StatusCode, String)> {
    // Boundary first, FS fallback — the same shape `list_pipelines` uses, and
    // `FleetOk` for the same reason: the compiled revision is `None` for a
    // LOCAL workspace unconditionally, so treating that as fatal made this
    // endpoint unusable in local mode entirely. The FS path is reached only
    // there (one instance, working copy present) or briefly before a workspace
    // is promoted.
    //
    // Both arms come from `ConfigManager`, which owns the choice, rather than
    // from `compiled_reader` plus a hand-written fallback beside it. The caller
    // already holds a manager whose revision the middleware pinned, so reading
    // around it would decide compiled-vs-disk a second time, at a call site
    // that cannot see which revision this request is on.
    let definition = match workspace
        .config_manager
        .pipeline_definition(pipeline_ref)
        .await
        .map_err(|e| pipeline_read_error(pipeline_ref, e))?
    {
        Some(definition) => definition,
        // `Ok(None)` is already the answer from BOTH sources. `definition()`
        // falls through to the working copy on a compiled miss and only returns
        // `None` for a genuine not-found; when there is no disk at all it
        // returns `None` without reading one. Either way a re-read here found
        // the same nothing and phrased the same 503 — so it re-derived the
        // workspace root through `working_copy().root()` to learn nothing, and
        // that synonym is invisible to `workspace_path_escape_hatch`.
        //
        // Still 503 rather than 404: the caller cannot tell a typo from a
        // workspace mid-compile, and "no such pipeline" during a deploy sends
        // an operator looking for a file that is there.
        None => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                format!(
                    "`{pipeline_ref}` is not in the compiled workspace and could not be read \
                     from the working copy — it may not exist, or the workspace may not be \
                     compiled yet. Retry, and check the path if it persists."
                ),
            ));
        }
    };

    // The kind is read BEFORE the expected path is derived, because the kind
    // is one of its segments. That ordering is why this returns the kind
    // rather than taking a path: deriving it outside would need the kind, and
    // the kind only exists once the definition has been read.
    let kind = uploadable_kind(&definition, pipeline_ref)?.to_string();
    let expected = pipeline_base_path(bucket, root, workspace_id, &kind, pipeline_slug);
    check_pipeline_definition(&definition, pipeline_ref, &expected)?;
    // A pipeline the loader would refuse is refused here too, naming the
    // pipeline — not carried forward as a scope that only this side honours.
    let allowed_stores = allowed_stores_of(&definition)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("`{pipeline_ref}`: {e}")))?;
    Ok((kind, allowed_stores))
}

/// The pipeline's `allowed_stores`, so validation scopes the way the load will.
///
/// `None` when the key is absent — every store loads.
///
/// `Err` for the two shapes the loader refuses, rather than passing them
/// through as a set that scopes differently here than there. That distinction
/// is the whole claim of this design — validation cannot disagree with the
/// loader, because it runs the same source — and it is cheap to lose:
///
/// - `allowed_stores: []` is refused by `source_factory` with a specific
///   message, because a pipeline that matches nothing can never load a row.
///   Passed through as `Some(∅)` it made the upload answer `rows: 0` and look
///   like a scoping result, for a pipeline that cannot run at all.
/// - a non-string entry was silently dropped by `filter_map(as_str)`, where
///   the loader's `Vec<String>` fails to deserialize. A typo'd entry would
///   have narrowed the count here and broken the run there.
fn allowed_stores_of(
    definition: &serde_json::Value,
) -> Result<Option<std::collections::HashSet<String>>, String> {
    let Some(list) = definition
        .get("source")
        .and_then(|s| s.get("config"))
        .and_then(|c| c.get("allowed_stores"))
    else {
        return Ok(None);
    };
    let Some(list) = list.as_array() else {
        return Err("`allowed_stores` must be a list of store names".to_string());
    };
    if list.is_empty() {
        return Err(
            "`allowed_stores` is an empty list, which matches no store and would load \
             nothing — remove the key to load every store"
                .to_string(),
        );
    }
    let mut out = std::collections::HashSet::with_capacity(list.len());
    for entry in list {
        let Some(name) = entry.as_str() else {
            return Err(format!(
                "`allowed_stores` entries must be store names, found `{entry}`"
            ));
        };
        out.insert(name.to_string());
    }
    Ok(Some(out))
}

/// Reject a `pipeline_ref` that could address anything but a file inside the
/// workspace: empty, absolute, or carrying a `..` segment.
///
/// **Syntactic, and deliberately not a path check.** The containment test this
/// replaces joined first and then asked `path.starts_with(root)` — which is a
/// component-wise prefix test over an unnormalized path, so
/// `root.join("../../other-ws/x.yml")` has components `/`, `ws`, `abc`, `..`,
/// `..`, … and *does* start with `/ws/abc`. It passed, and `read_to_string`
/// then resolved the `..` at the syscall layer. Only the absolute case was
/// caught, because `join` replaces rather than appends there.
///
/// That was reachable in cloud, not merely locally: `resolve_pipeline` answers
/// `Ok(None)` when no compiled row matches, and a `..`-bearing ref can never
/// match one — so it always fell through to the disk read, on a node that may
/// hold several workspaces' working copies.
///
/// Mirrors `agentic_pipeline`'s `validate_pipeline_ref`, which guards the same
/// input one crate over.
fn validate_pipeline_ref(pipeline_ref: &str) -> Result<(), String> {
    // Emptiness is checked here as well as at the call site. Nothing slips
    // through today, but this is written as the reusable mirror of
    // `agentic_pipeline::validate_pipeline_ref`, and a guard whose doc claims
    // a rejection it does not make is worse than one that never claimed it.
    if pipeline_ref.trim().is_empty() {
        return Err("`pipeline_ref` must not be empty".to_string());
    }
    let candidate = std::path::Path::new(pipeline_ref);
    if candidate.is_absolute() {
        return Err(format!(
            "`{pipeline_ref}` must be relative to the workspace"
        ));
    }
    if candidate
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(format!("`{pipeline_ref}` must not contain `..` segments"));
    }
    Ok(())
}

/// How a failed pipeline read reaches the caller.
///
/// `pipeline_definition` owns the DISK read as well as the boundary one, so its
/// errors carry both kinds and they must not answer the same way. The rule is
/// `manager.rs`'s own, stated there in as many words: a root that is not on this
/// node is transient, while a file that fails to parse is a real fault, and
/// collapsing them turns a 503 into a 500.
///
/// This existed before as two separate shapes and was lost when the read moved
/// onto the manager: a malformed `.airway.yml` used to answer 400 naming the
/// parse error, and it briefly answered 500 with the error dropped. Sibling
/// callers draw the same line — `list_monitors` answers 400, `get_automation_file`
/// 422.
fn pipeline_read_error(
    pipeline_ref: &str,
    err: oxy::config::ArtifactError,
) -> (StatusCode, String) {
    // `retryable()` tests the WRAPPER, and `ArtifactError::Config` is
    // `#[from] OxyError` — so it carries more than a parse failure.
    // `definition_from_disk` reaches `fs_link` → `validate_path_within_project`,
    // which answers `OxyError::IOError("Failed to canonicalize project path")`
    // on an EACCES, or on the root vanishing between `disk()`'s check and the
    // read. Keying the 400 on the variant alone reported that node-local IO
    // fault as the customer's YAML, non-retryable, naming a parse that never
    // ran. Match the INNER error instead: only a configuration error is theirs.
    let their_yaml = matches!(
        &err,
        oxy::config::ArtifactError::Config(oxy_shared::errors::OxyError::ConfigurationError(_))
    );
    if !their_yaml {
        // A root this node does not hold, an unreadable file, a boundary that
        // is not there yet — all the same state to the caller, and all worth
        // retrying. Same phrasing `read_pipeline_from_disk` produces, because
        // the caller cannot tell which of the two produced it.
        tracing::warn!(error = %err, pipeline_ref, "pipeline not readable here");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            format!(
                "`{pipeline_ref}` is not in the compiled workspace and could not be read from \
                 the working copy — it may not exist, or the workspace may not be compiled \
                 yet. Retry, and check the path if it persists."
            ),
        );
    }
    // The customer's YAML. Carry the parse error: it names the line.
    (
        StatusCode::BAD_REQUEST,
        format!("`{pipeline_ref}` could not be parsed: {err}"),
    )
}

/// Source kinds that accept an upload.
///
/// One list so widening is a single edit — and so the UI's tab predicate and
/// the server's refusal cannot disagree about which pipelines are uploadable.
/// A kind here also becomes a path segment, which is why it is a fixed set
/// rather than "whatever the definition says".
pub use agentic_airway::upload_zone::UPLOADABLE_SOURCE_KINDS;

/// The two things a pipeline definition has to agree about.
///
/// Split from the lookup so both refusals are testable without a database —
/// the DB half is one `resolve_pipeline` call, and these are the decisions.
fn uploadable_kind<'a>(
    definition: &'a serde_json::Value,
    pipeline_ref: &str,
) -> Result<&'a str, (StatusCode, String)> {
    let kind = definition
        .get("source")
        .and_then(|src| src.get("kind"))
        .and_then(|k| k.as_str())
        .unwrap_or_default();
    if !agentic_airway::upload_zone::is_uploadable(kind) {
        // A payment-details report in another vendor's zone lands rows no
        // reader expects, and the loader would NOT refuse it — nothing
        // downstream knows the file came through the wrong door.
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "`{pipeline_ref}` is a `{kind}` pipeline — reports can only be uploaded to one \
                 of {UPLOADABLE_SOURCE_KINDS:?}"
            ),
        ));
    }
    Ok(kind)
}

fn check_pipeline_definition(
    definition: &serde_json::Value,
    pipeline_ref: &str,
    expected_base_path: &str,
) -> Result<(), (StatusCode, String)> {
    uploadable_kind(definition, pipeline_ref)?;

    // The config block itself has to be a map before anything is read out of
    // it. `Value::get` answers `None` on a string or a list exactly as it does
    // for a missing key, so without this a `config: "oops"` would read as
    // "no base_path, fine", the upload would be ACCEPTED and the bytes
    // written — for a pipeline whose run then refuses the same shape
    // (`source config for … is not a map`). Refusing here keeps the two halves
    // agreeing, which is the entire claim of this design.
    let config = match definition.get("source").and_then(|src| src.get("config")) {
        None | Some(serde_json::Value::Null) => None,
        Some(cfg) if cfg.is_object() => Some(cfg),
        Some(_) => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("`{pipeline_ref}`: `source.config` must be a map"),
            ));
        }
    };

    // ABSENT IS FINE, and is now the recommended shape. The run derives its
    // zone from `agentic_airway::upload_zone` — the same helper that produced
    // `expected_base_path` — so an omitted value cannot disagree with this
    // one. Only a value someone WROTE can drift, which is why the check below
    // still exists for a declared one.
    let declared = match config.and_then(|cfg| cfg.get("base_path")) {
        // Absent, null, or blank-after-normalization all mean the run derives
        // its zone from the same helper that produced `expected`.
        None | Some(serde_json::Value::Null) => return Ok(()),
        // A wrong TYPE gets its own refusal rather than falling through as an
        // empty string. `base_path: 42` rendered through the mismatch arm
        // reads "reads from ``", which is indistinguishable from
        // `base_path: ""` and sends the operator looking for the wrong thing.
        Some(v) if !v.is_string() => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "`{pipeline_ref}`: `base_path` must be a string, got {}",
                    match v {
                        serde_json::Value::Number(_) => "a number",
                        serde_json::Value::Bool(_) => "a boolean",
                        serde_json::Value::Array(_) => "a list",
                        _ => "a map",
                    }
                ),
            ));
        }
        // Normalized through the SHARED helper, so this side and the run side
        // cannot disagree about what a declared zone reduces to. A value that
        // normalizes to nothing (`""`, `"  "`, `"/"`) reads as ABSENT and is
        // derived, matching `fill_upload_zone`. Refusing it instead produced a
        // message saying the pipeline "reads from ``" — false, since at run
        // time it reads the derived zone.
        Some(v) => match v
            .as_str()
            .and_then(agentic_airway::upload_zone::normalize_base_path)
        {
            Some(normalized) => normalized,
            None => return Ok(()),
        },
    };

    if declared != expected_base_path {
        // The old shape left these two to be kept equal by instruction, and a
        // drift landed reports where nothing read them — silently, since both
        // halves look fine on their own. Naming BOTH values is the point: the
        // fix is to edit one of them, and the operator has to see which.
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "`{pipeline_ref}` reads from `{declared}`, but uploads for it land in \
                 `{expected_base_path}` — the report would never be loaded. Set the pipeline's \
                 `base_path` to the upload location."
            ),
        ));
    }
    Ok(())
}

/// Where a report lands inside the zone.
///
/// **This layout is read by the loader, not just written by us.**
///
/// - `YYYY.MM` is a path component because that is what the source scans for
///   when deriving the period. It scans components right-to-left for a
///   `YYYY[.-_]MM` token, so the period must appear as its own segment — a
///   Hive-style `report_year=2026` would NOT be recognized.
/// - The path **within the zone** is what the source folds into `_row_uid`. So
///   two uploads that should merge must land on the same key, and two that must
///   coexist (two chunks of one period) must land on different ones. Re-posting
///   the same `workflow_id` for the same period is therefore idempotent by
///   construction: same key, same rows, same ids, and the loader's `Merge`
///   collapses them.
/// - The extension stays `.csv` because the source reads `.csv` only and
///   refuses a base naming anything else.
/// - **Workspace, source kind AND pipeline are segments**, because one bucket
///   now serves every file-upload source kind, workspace and pipeline on a
///   deployment. Without them, two pipelines posting the same `workflow_id`
///   for the same period write the same object — and since the key IS the
///   merge identity, the second silently overwrites the first and its rows
///   merge into the other's table at identical `_row_uid`s. Both sit BEFORE
///   the period so the source's right-to-left scan still finds `YYYY.MM` as
///   its own segment.
///
/// So the layout is
/// `<root>/<workspace_id>/<source_kind>/<pipeline>/YYYY.MM/<workflow_id>.csv`,
/// and a pipeline's `base_path` is
/// `<zone>/<workspace_id>/<source_kind>/<pipeline>` — which
/// [`pipeline_base_path`] derives and [`check_pipeline_definition`] enforces,
/// rather than leaving an operator to keep two values equal by hand.
#[allow(clippy::too_many_arguments)]
fn object_key(
    root: &str,
    workspace_id: Uuid,
    source_kind: &str,
    pipeline_slug: &str,
    year: i64,
    month: u32,
    workflow_id: &str,
) -> String {
    let period = format!("{year:04}.{month:02}");
    let tail = format!("{workspace_id}/{source_kind}/{pipeline_slug}/{period}/{workflow_id}.csv");
    if root.is_empty() {
        tail
    } else {
        format!("{root}/{tail}")
    }
}

/// The zone a pipeline's reports live under — everything above the period.
///
/// This is what the pipeline's `base_path` must equal, and what
/// [`check_pipeline_definition`] compares against. Deriving it in one place
/// means the value we CHECK and the value we WRITE cannot drift.
fn pipeline_base_path(
    bucket: &str,
    root: &str,
    workspace_id: Uuid,
    source_kind: &str,
    pipeline_slug: &str,
) -> String {
    agentic_airway::upload_zone::pipeline_base_path(
        bucket,
        root,
        workspace_id,
        source_kind,
        pipeline_slug,
    )
}

/// A pipeline ref reduced to one safe path segment.
///
/// `pipelines/ubereats.airway.yml` → `pipelines__ubereats`: every component
/// participates, joined with `__`. The directory is in there because
/// `east/ue.airway.yml` and `west/ue.airway.yml` otherwise reduce to one
/// segment, one base path and one key prefix — and `<kind>` does not separate
/// them either, since both are `ubereats`.
///
/// Rejected rather than sanitized when nothing usable remains: the segment is
/// part of the key, and the key is the merge identity.
fn pipeline_slug(pipeline_ref: &str) -> Option<String> {
    agentic_airway::upload_zone::pipeline_slug(pipeline_ref)
}

/// A `workflow_id` has to be safe to put in an object key.
///
/// Rejected rather than sanitized: a silently-rewritten id would produce a key
/// the caller does not expect, and since the key IS the merge identity, a
/// caller who thinks they re-posted the same report would instead land a second
/// copy under a different name.
fn valid_workflow_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

/// Read a numeric multipart field, or refuse naming the field and the value.
async fn parse_field<T: std::str::FromStr>(
    field: axum::extract::multipart::Field<'_>,
    name: &str,
) -> Result<T, (StatusCode, String)> {
    let raw = field
        .text()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("reading `{name}`: {e}")))?;
    raw.trim().parse::<T>().map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            format!("`{name}` is not a number: `{}`", raw.trim()),
        )
    })
}

/// `POST /api/{workspace_id}/source-uploads/reports`
///
/// Multipart fields: `file` (required), `workflow_id` (required),
/// `report_year` + `report_month` (optional, both-or-neither).
pub async fn upload_report(
    // Writing the input to a journal-entry pipeline is a contributor action:
    // `workspace_middleware` proves the caller may *access* the workspace, not
    // that they may change it, so a Viewer would otherwise pass.
    _editor: WorkspaceEditor,
    WorkspaceManagerReadOnly(workspace): WorkspaceManagerReadOnly,
    Path(workspace_id): Path<Uuid>,
    mut multipart: Multipart,
) -> Result<axum::Json<UploadedReport>, (StatusCode, String)> {
    let mut bytes: Option<Vec<u8>> = None;
    let mut pipeline_ref: Option<String> = None;
    let mut filename: Option<String> = None;
    let mut workflow_id: Option<String> = None;
    let mut year: Option<i64> = None;
    let mut month: Option<u32> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("multipart error: {e}")))?
    {
        match field.name().unwrap_or_default() {
            "file" => {
                filename = field.file_name().map(str::to_string);
                let data = field
                    .bytes()
                    .await
                    .map_err(|e| (StatusCode::BAD_REQUEST, format!("file read: {e}")))?;
                if data.len() > MAX_REPORT_BYTES {
                    return Err((
                        StatusCode::PAYLOAD_TOO_LARGE,
                        format!(
                            "report is {} bytes, over the {MAX_REPORT_BYTES}-byte ceiling",
                            data.len()
                        ),
                    ));
                }
                bytes = Some(data.to_vec());
            }
            "pipeline_ref" => {
                pipeline_ref = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| {
                            (
                                StatusCode::BAD_REQUEST,
                                format!("reading `pipeline_ref`: {e}"),
                            )
                        })?
                        .trim()
                        .to_string(),
                );
            }
            "workflow_id" => {
                // Not `.ok()`: a read failure here would leave `None` and tell
                // the caller the field is *missing* — the same misdirection
                // `parse_field` was added to remove one field over.
                workflow_id = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| {
                            (
                                StatusCode::BAD_REQUEST,
                                format!("reading `workflow_id`: {e}"),
                            )
                        })?
                        .trim()
                        .to_string(),
                );
            }
            // Parsed strictly. Silently dropping an unparseable value told a
            // caller who sent BOTH halves that they must "be given together",
            // which is the same misdirection this file refuses to commit by
            // sanitizing a `workflow_id`.
            "report_year" => year = Some(parse_field::<i64>(field, "report_year").await?),
            "report_month" => month = Some(parse_field::<u32>(field, "report_month").await?),
            _ => {}
        }
    }

    let bytes = bytes.ok_or((StatusCode::BAD_REQUEST, "missing `file` field".to_string()))?;
    let pipeline_ref = pipeline_ref.filter(|p| !p.is_empty()).ok_or((
        StatusCode::BAD_REQUEST,
        "missing `pipeline_ref` field — the landing zone is the pipeline's own \
         `base_path`, so the upload has to name which pipeline it is for"
            .to_string(),
    ))?;
    // Checked HERE, not where the path is built, so it also covers the
    // compiled-row lookup — a ref that could escape the workspace on disk must
    // not be allowed to address a row either.
    validate_pipeline_ref(&pipeline_ref).map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    let workflow_id = workflow_id.ok_or((
        StatusCode::BAD_REQUEST,
        "missing `workflow_id` field — it names the object, and re-posting the same \
         id for the same period is what makes an upload idempotent"
            .to_string(),
    ))?;
    if !valid_workflow_id(&workflow_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            "`workflow_id` must be 1–128 characters of [A-Za-z0-9_-] — it becomes an \
             object key, and sanitizing it silently would land the report somewhere \
             the caller did not name"
                .to_string(),
        ));
    }
    // Both or neither, matching the pipeline config's rule for the same pair:
    // half a period is not one, and guessing the other half stamps a month
    // nobody named.
    let period = match (year, month) {
        (Some(y), Some(m)) => {
            // The same bounds the pipeline config enforces, from one
            // definition — here the period is also interpolated into the
            // object key, so `2026.13` would produce a key the source's period
            // scan cannot read and strand the report in the zone.
            check_period(y, m).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
            Some((y, m))
        }
        (None, None) => None,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "`report_year` and `report_month` must be given together — half a \
                 period is not one"
                    .to_string(),
            ));
        }
    };
    // A name is required, because it is what the period is derived from when
    // the caller did not supply one — and `.csv`, because the source reads
    // nothing else and would refuse this file at load time.
    let filename = filename.filter(|f| !f.trim().is_empty()).ok_or((
        StatusCode::BAD_REQUEST,
        "the `file` part needs a filename — the report period is read from it when \
         `report_year`/`report_month` are not given"
            .to_string(),
    ))?;
    if !filename.to_ascii_lowercase().ends_with(".csv") {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "`{filename}` is not a `.csv` — this source reads CSV only, and `.xlsx` \
                 is out of scope"
            ),
        ));
    }

    let (bucket, root) = zone()?;
    let slug = pipeline_slug(&pipeline_ref).ok_or((
        StatusCode::BAD_REQUEST,
        format!(
            "`{pipeline_ref}` does not reduce to a usable path segment — a pipeline file name \
             must be [A-Za-z0-9_-]"
        ),
    ))?;

    // The pipeline is read BEFORE the report is validated, because validation
    // has to use the pipeline's own scoping. Validating with a bare source
    // reported what a bare source would load, not what THIS pipeline will —
    // a report spanning six stores with two in scope answered six and then
    // loaded two, which is precisely the disagreement "validate by running the
    // real source" exists to rule out.
    //
    // It also fails earlier on a misconfigured pipeline, before spending time
    // parsing a file that had nowhere to go.
    let (kind, allowed_stores) = zone_for_pipeline(
        &workspace,
        workspace_id,
        &pipeline_ref,
        &bucket,
        &root,
        &slug,
    )
    .await?;

    let validated = validate_ubereats_report(&bytes, &filename, period, allowed_stores.as_ref())
        .await
        .map_err(|e| match e {
            // Airway's message verbatim: it names the file and the missing
            // column, and rewording it would make the upload-time and
            // load-time diagnoses differ for one cause.
            ReportValidationError::Rejected(msg) => (StatusCode::BAD_REQUEST, msg),
            ReportValidationError::Unavailable(msg) => {
                tracing::error!(error = %msg, "report validation unavailable");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "could not validate the report".to_string(),
                )
            }
        })?;
    let (year, month, rows) = (
        validated.report_year,
        validated.report_month,
        validated.rows,
    );

    // Bounds again, because a period derived from the FILENAME never passed
    // through the check above and lands in the key just the same. Named for
    // where it came from: telling a caller who sent no form fields that
    // "`report_year` must be 2000–2100" points at something they never sent.
    //
    // Only the filename branch can fail here. A caller-supplied period was
    // already checked above, and `validate_ubereats_report` pins it with
    // `with_period`, so the value read back off a row equals the input — the
    // `Some(_)` arm below is defensive, not a second live path.
    check_period(year, month).map_err(|e| {
        let msg = match period {
            Some(_) => e,
            None => format!("the period read from `{filename}` is out of range: {e}"),
        };
        (StatusCode::BAD_REQUEST, msg)
    })?;
    let key = object_key(&root, workspace_id, &kind, &slug, year, month, &workflow_id);

    put_object(&bucket, &key, bytes).await?;

    Ok(axum::Json(UploadedReport {
        location: format!("s3://{bucket}/{key}"),
        report_year: year,
        report_month: month,
        rows,
    }))
}

/// Write the report, with its own S3 client.
///
/// Honors `AWS_ENDPOINT_URL` with path-style addressing so LocalStack/MinIO
/// work, matching the custom-app stores — that flag being forgotten in a second
/// client is the failure this comment exists to prevent.
async fn put_object(bucket: &str, key: &str, bytes: Vec<u8>) -> Result<(), (StatusCode, String)> {
    let shared = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let mut builder = aws_sdk_s3::config::Builder::from(&shared);
    if std::env::var("AWS_ENDPOINT_URL")
        .ok()
        .is_some_and(|v| !v.trim().is_empty())
    {
        builder = builder.force_path_style(true);
    }
    aws_sdk_s3::Client::from_conf(builder.build())
        .put_object()
        .bucket(bucket)
        .key(key)
        .content_type("text/csv")
        .body(aws_sdk_s3::primitives::ByteStream::from(bytes))
        .send()
        .await
        // Category, not the error: an S3 SDK error's Display can carry the
        // built request URL, which for a presigned or credentialed request is
        // the credential.
        .map_err(|e| {
            tracing::error!(error = ?e, bucket, key, "UberEats report upload failed");
            (
                StatusCode::BAD_GATEWAY,
                "writing the report to the landing zone failed".to_string(),
            )
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The route must keep an authorization extractor.
    ///
    /// `authz_boundaries.rs` cannot catch this: it bans hand-rolled authority
    /// *shapes* (`matches!(role, Owner | Admin)` and friends), so a handler
    /// with NO guard at all is invisible to it — which is exactly how this
    /// route shipped letting a Viewer write into the journal-entry landing
    /// zone. Scanning the signature is the cheap check that would have caught
    /// it, so it lives next to the thing it protects.
    #[test]
    fn the_upload_handler_takes_a_role_guard() {
        let src = include_str!("source_upload.rs");
        let at = src
            .find("pub async fn upload_report(")
            .expect("the handler must still be named this");
        let end = src[at..]
            .find(") -> Result")
            .expect("the handler must still return a Result");
        // Comments stripped: the window spans the explanatory comment above
        // `_editor`, so a future reword that merely MENTIONS the type in prose
        // would keep this green with the extractor gone. The test would then
        // assert the wording of a comment, not the presence of a guard.
        let signature: String = src[at..at + end]
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        // ANY of the six guards, in a binding position — not this one exactly.
        //
        // Round 5 named `WorkspaceAdmin` as the defensible alternative for
        // financial ingest. Pinning `WorkspaceEditor` would make that
        // *tightening* fail the test, reporting "lost its role guard" — the
        // opposite of what happened. The property worth protecting is that a
        // guard is present, not which one; a test that resists being
        // strengthened teaches people to delete it.
        //
        // Hand-copied, and therefore a duplicate of the real set: adding a
        // seventh guard to `role_guards.rs` and using it here fails this test
        // with "has no role guard". It fails CLOSED and names the list, so it
        // is cheap to diagnose.
        //
        // **This whole scan is the wrong artifact and should be deleted.** The
        // durable check is "every mutating route names a guard from
        // `role_guards`", in `crates/app/tests/authz/authz_boundaries.rs` — it
        // already walks every handler source with a `BANNED`/`ALLOWED` pair,
        // and it would retire this test, this copied list, and the three
        // rounds of syntax-chasing above it (exact type, qualified path,
        // trailing comment). This scan exists because that check does not yet,
        // and it is NOT currently tracked by an issue — saying "tracked
        // separately" without naming one is how a deferral becomes permanent.
        const GUARDS: [&str; 6] = [
            "OrgOwner",
            "OrgAdmin",
            "OrgAdminStrict",
            "OrgMemberStrict",
            "WorkspaceAdmin",
            "WorkspaceEditor",
        ];
        // Each parameter's type reduced to its LAST `::` segment, so a
        // fully-qualified binding — `_editor:
        // oxy_server_authz::role_guards::WorkspaceAdmin`, the form round 5's
        // own snippet used — still counts. Binding position is kept, so a bare
        // mention in prose cannot satisfy it.
        let guarded = signature.lines().any(|line| {
            // Truncated at `//`, which covers a TRAILING comment as well as a
            // whole-line one. Without it, `_editor: WorkspaceEditor, // …`
            // leaves the comment glued to the type, `trim_end_matches(',')`
            // no-ops because the line ends in prose, and the test reports "no
            // role guard" on a guarded route.
            // `split` with a non-empty pattern always yields one item.
            let line = line.split("//").next().expect("split yields one item");
            let Some((_, ty)) = line.split_once(':') else {
                return false;
            };
            let ty = ty.trim().trim_end_matches(',').trim();
            // `rsplit` always yields at least one item, so this is total.
            let last = ty.rsplit("::").next().expect("rsplit yields one item");
            GUARDS.contains(&last)
        });

        assert!(
            guarded,
            "the upload handler has no role guard in its signature — \
             `workspace_middleware` proves workspace ACCESS, not that the caller \
             may write. If you deliberately CHANGED the guard, it must still be \
             one of {GUARDS:?}. Signature was:\n{signature}"
        );
    }

    fn definition(kind: &str, base_path: &str) -> serde_json::Value {
        serde_json::json!({
            "name": "ue",
            "source": { "kind": kind, "config": { "base_path": base_path } },
        })
    }

    /// The bucket is server configuration and the pipeline never chooses it —
    /// the pod role reaches every `<cluster>-*` bucket, so a customer-supplied
    /// bucket would have the server write customer bytes into a sibling
    /// service's bucket with its own credentials.
    ///
    /// What the pipeline DOES have to do is agree about where its reports
    /// land, and a disagreement is refused naming both values.
    #[test]
    fn a_pipeline_reading_somewhere_else_is_refused() {
        let expected = "s3://oxy-dev-ue/ws-1/ubereats";

        check_pipeline_definition(&definition("ubereats", expected), "p.airway.yml", expected)
            .expect("agreement passes");

        // Agreement is judged on the NORMALIZED value, so padding is not a
        // disagreement. Without this, dropping the `normalize_base_path` call
        // and going back to `as_str().unwrap_or_default()` breaks no test —
        // every other fixture here is already clean.
        for padded in [
            "s3://oxy-dev-ue/ws-1/ubereats/",
            "s3://oxy-dev-ue/ws-1/ubereats ",
            "  s3://oxy-dev-ue/ws-1/ubereats/ ",
        ] {
            check_pipeline_definition(&definition("ubereats", padded), "p.airway.yml", expected)
                .unwrap_or_else(|e| panic!("`{padded}` normalizes to agreement: {e:?}"));
        }

        let (status, msg) = check_pipeline_definition(
            &definition("ubereats", "s3://somewhere-else/x"),
            "p.airway.yml",
            expected,
        )
        .expect_err("a pipeline reading elsewhere would never see the report");
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(msg.contains("somewhere-else"), "names what it reads: {msg}");
        assert!(msg.contains(expected), "names where uploads land: {msg}");
    }

    /// THE claim of the derived-zone change: a pipeline that declares no
    /// `base_path` agrees by construction, because the run derives its zone
    /// from the same helper that produced `expected`.
    ///
    /// Worth pinning rather than trusting: the previous shape read an absent
    /// value as `""` and refused it, so "absent means refuse" is a one-line
    /// regression away, and it would surface as uploads failing for exactly
    /// the config this change tells people to write.
    #[test]
    fn an_omitted_base_path_is_agreement() {
        let expected = "s3://oxy-dev-ue/ws-1/ubereats";

        check_pipeline_definition(
            &serde_json::json!({ "source": { "kind": "ubereats", "config": {} } }),
            "p.airway.yml",
            expected,
        )
        .expect("an omitted base_path is derived at run time, so it cannot disagree");

        // No `config` block at all — `#[serde(default)]` makes this Null, not
        // an empty map, and it has to read the same way.
        check_pipeline_definition(
            &serde_json::json!({ "source": { "kind": "ubereats" } }),
            "p.airway.yml",
            expected,
        )
        .expect("an absent config block is an absent base_path");

        // An explicit `base_path:` with nothing after it is YAML null, which
        // means "no value" rather than a value of the wrong type.
        check_pipeline_definition(
            &serde_json::json!({
                "source": { "kind": "ubereats", "config": { "base_path": null } }
            }),
            "p.airway.yml",
            expected,
        )
        .expect("an explicit null is absent, not a wrong type");

        // A DECLARED value that normalizes to nothing. This arm is a genuine
        // behaviour flip — it used to be refused — and the cases above cannot
        // reach it: absent / `{}` / null all return before the normalize arm.
        for blank in ["", "   ", "/", "///"] {
            check_pipeline_definition(
                &serde_json::json!({
                    "source": { "kind": "ubereats", "config": { "base_path": blank } }
                }),
                "p.airway.yml",
                expected,
            )
            .unwrap_or_else(|e| panic!("`{blank}` names nowhere, so it derives: {e:?}"));
        }
    }

    /// `Value::get` answers `None` on a string exactly as it does for a
    /// missing key, so without an explicit shape check a `config: "oops"`
    /// reads as "no base_path, fine" — the upload is ACCEPTED and the bytes
    /// written, for a pipeline whose run then refuses the same shape. The two
    /// halves agreeing is the whole claim, so the asymmetry is the bug.
    #[test]
    fn a_non_map_source_config_is_refused() {
        for cfg in [
            serde_json::json!("oops"),
            serde_json::json!([1, 2]),
            serde_json::json!(7),
        ] {
            let (status, msg) = check_pipeline_definition(
                &serde_json::json!({ "source": { "kind": "ubereats", "config": cfg } }),
                "p.airway.yml",
                "s3://z",
            )
            .expect_err("a non-map config cannot carry a base_path");
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert!(msg.contains("source.config"), "names the field: {msg}");
        }
    }

    /// A wrong TYPE gets its own refusal rather than falling through as an
    /// empty string. Rendered through the mismatch arm, `base_path: 42` reads
    /// "reads from ``" — indistinguishable from `base_path: ""`, which sends
    /// the operator looking for a value they did not write.
    #[test]
    fn a_wrong_typed_base_path_is_refused_by_type() {
        for (bad, want) in [
            (serde_json::json!(42), "a number"),
            (serde_json::json!(true), "a boolean"),
            (serde_json::json!(["s3://z"]), "a list"),
            (serde_json::json!({"bucket": "z"}), "a map"),
        ] {
            let (status, msg) = check_pipeline_definition(
                &serde_json::json!({
                    "source": { "kind": "ubereats", "config": { "base_path": bad } }
                }),
                "p.airway.yml",
                "s3://z",
            )
            .expect_err("a base_path that is not a string is not a base_path");
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert!(
                msg.contains("must be a string"),
                "says what is wrong: {msg}"
            );
            assert!(msg.contains(want), "names the type it got: {msg}");
        }
    }

    /// A payment-details report in another vendor's zone lands rows no reader
    /// expects — and the loader would NOT refuse it, because nothing
    /// downstream knows the file came through the wrong door.
    #[test]
    fn uploading_into_a_non_ubereats_pipeline_is_refused() {
        for kind in ["toast", "quickbooks", "filesystem", ""] {
            let (status, msg) = check_pipeline_definition(
                &definition(kind, "s3://z"),
                "toast.airway.yml",
                "s3://z",
            )
            .expect_err("only a ubereats pipeline may take these reports");
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert!(
                msg.contains("toast.airway.yml"),
                "names the pipeline: {msg}"
            );
        }
    }

    /// A definition with no `source` at all is refused for the missing KIND,
    /// not for the missing `base_path` — an absent `base_path` is agreement
    /// now, and the recommended shape. The distinction matters because this
    /// fixture would still pass if the base_path arm regressed, so it is not
    /// the test that guards that; `an_omitted_base_path_is_agreement` is.
    #[test]
    fn a_definition_missing_its_source_is_refused() {
        let (status, _) =
            check_pipeline_definition(&serde_json::json!({ "name": "ue" }), "p", "s3://z")
                .expect_err("no source means no agreement");
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// Scoping must be refused here exactly when the loader refuses it.
    ///
    /// The design's strongest claim is that validation cannot disagree with
    /// the load, because it runs the same source. That holds for the parsing
    /// and held for the store set only by accident: an empty list came
    /// through as `Some(∅)`, so the upload answered `rows: 0` — a scoping
    /// result, for a pipeline `source_factory` refuses outright — and a
    /// non-string entry was dropped by `filter_map`, narrowing the count here
    /// and failing the run there.
    #[test]
    fn scoping_is_refused_exactly_where_the_loader_refuses_it() {
        let of = |cfg: serde_json::Value| {
            allowed_stores_of(&serde_json::json!({ "source": { "config": cfg } }))
        };

        // Absent: every store loads, which is a scope, not an error.
        assert_eq!(of(serde_json::json!({})), Ok(None));

        let one = of(serde_json::json!({ "allowed_stores": ["Poke House SF"] }))
            .expect("a list of names is valid");
        assert_eq!(
            one,
            Some(std::collections::HashSet::from([
                "Poke House SF".to_string()
            ]))
        );

        // The two the loader refuses.
        assert!(
            of(serde_json::json!({ "allowed_stores": [] })).is_err(),
            "an empty list matches nothing and must not read as a scope"
        );
        assert!(
            of(serde_json::json!({ "allowed_stores": ["ok", 7] })).is_err(),
            "a non-string entry must fail here, as it does in the loader"
        );
        assert!(
            of(serde_json::json!({ "allowed_stores": "Poke House SF" })).is_err(),
            "a bare string is not a list"
        );
    }

    /// A `pipeline_ref` must not be able to address a file outside the
    /// workspace.
    ///
    /// Regression test for a containment check that did not contain. It joined
    /// first and then asked `path.starts_with(root)` — a component-wise prefix
    /// test over an unnormalized path — so `root.join("../../other/x.yml")`
    /// starts with `root` and passed, and `read_to_string` resolved the `..`
    /// at the syscall layer. Only the absolute case was caught, because `join`
    /// replaces rather than appends there.
    ///
    /// The `..` cases are the ones that regressed; the absolute one is here so
    /// a future rewrite cannot fix these by dropping that.
    #[test]
    fn a_pipeline_ref_cannot_escape_the_workspace() {
        for bad in [
            "../secrets.yml",
            "../../other-ws/pipelines/ue.airway.yml",
            "pipelines/../../../etc/passwd",
            "/etc/passwd",
        ] {
            assert!(
                validate_pipeline_ref(bad).is_err(),
                "`{bad}` must be refused"
            );
        }

        // The lexical test the guard replaces, asserted directly: it is why a
        // path-shaped check cannot be the guard here.
        let root = std::path::Path::new("/ws/abc");
        assert!(
            root.join("../../other-ws/x.yml").starts_with(root),
            "if this ever fails, `starts_with` gained normalization and this \
             test's premise is stale — not that the guard is unnecessary"
        );

        for good in [
            "pipelines/ubereats.airway.yml",
            "ue.airway.yml",
            "a/b/c.airway.yml",
        ] {
            assert!(
                validate_pipeline_ref(good).is_ok(),
                "`{good}` must be accepted"
            );
        }
    }

    #[test]
    fn a_pipeline_ref_reduces_to_one_safe_segment() {
        // The conventional location, and the directory is part of the slug:
        // it is what keeps two same-named pipelines apart.
        assert_eq!(
            pipeline_slug("pipelines/ubereats.airway.yml").as_deref(),
            Some("pipelines__ubereats")
        );
        assert_eq!(
            pipeline_slug("a/b/my-pipe.airway.yml").as_deref(),
            Some("a__b__my-pipe")
        );

        // The collision this separator exists to prevent: same file name,
        // different directories, and nothing else in the key distinguishes
        // them — `<kind>` is `ubereats` for both.
        assert_ne!(
            pipeline_slug("east/ue.airway.yml"),
            pipeline_slug("west/ue.airway.yml")
        );
        // `__` rather than `_`, so a `_` legal inside a component cannot
        // reproduce the collision one level down.
        assert_ne!(
            pipeline_slug("a_b/c.airway.yml"),
            pipeline_slug("a/b_c.airway.yml")
        );
        assert_eq!(
            pipeline_slug("ue_2026.airway.yml").as_deref(),
            Some("ue_2026")
        );

        // A `..` is now REFUSED rather than dropped, and this assertion read
        // `Some("x")` until the helper moved into `agentic-airway`.
        //
        // Dropping was defensible while this lived beside the
        // `validate_pipeline_ref` that refuses traversal at handler entry.
        // It is not defensible in a crate whose premise is a third caller:
        // dropping collapses `../x.airway.yml` and `x.airway.yml` onto one
        // slug, one key prefix and — the object name being a content hash —
        // one merge identity. That is the collision `__` exists to prevent,
        // reintroduced by the sanitizer meant to be harmless.
        assert_eq!(pipeline_slug("../../x.airway.yml"), None);

        // A dot the suffix-stripping does NOT consume stays in the stem, and a
        // dot in a key segment is not safe.
        assert_eq!(pipeline_slug("ue.2026.airway.yml"), None);

        for bad in ["", "sp ace.airway.yml", "sl/ash!"] {
            assert_eq!(pipeline_slug(bad), None, "`{bad}` must be refused");
        }

        // NOT refused here, deliberately: a trailing slash is ignored by
        // `file_name()`, so `pipelines/` reduces to a perfectly safe segment.
        // This function only asks "is this usable in a key" — whether a
        // pipeline exists at that ref is `resolve_pipeline`'s answer, and it
        // refuses with the retryable not-compiled message.
        assert_eq!(pipeline_slug("pipelines/").as_deref(), Some("pipelines"));
    }

    /// What we CHECK and what we WRITE are derived from one function, so they
    /// cannot drift — the failure the old "keep these two equal" instruction
    /// allowed.
    #[test]
    fn the_declared_base_path_is_where_uploads_actually_land() {
        let w = ws(1);
        let base = pipeline_base_path("bkt", "root", w, "ubereats", "ue");
        let key = object_key("root", w, "ubereats", "ue", 2026, 8, "wf-1");

        assert_eq!(base, format!("s3://bkt/root/{w}/ubereats/ue"));
        assert_eq!(
            format!("s3://bkt/{key}"),
            format!("{base}/2026.08/wf-1.csv"),
            "the object must sit directly under the base_path the pipeline declares"
        );
    }

    fn ws(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    /// The period must be its own `YYYY.MM` path segment, because that is what
    /// the source scans for. A Hive-style `report_year=2026` is NOT recognized
    /// by `period_from_filename`, so writing one would strand every report.
    #[test]
    fn the_key_puts_the_period_in_its_own_segment() {
        let a = ws(1);
        assert_eq!(
            object_key("ubereats", a, "ubereats", "ue", 2026, 8, "wf-1"),
            format!("ubereats/{a}/ubereats/ue/2026.08/wf-1.csv")
        );
        // Zero-padded, matching the token the source parses.
        assert_eq!(
            object_key("z", a, "ubereats", "ue", 2026, 1, "w"),
            format!("z/{a}/ubereats/ue/2026.01/w.csv")
        );
        // A zone at the bucket root has no leading slash to double up.
        assert_eq!(
            object_key("", a, "ubereats", "ue", 2026, 8, "wf-1"),
            format!("{a}/ubereats/ue/2026.08/wf-1.csv")
        );
    }

    /// The zone is one process-wide setting shared by every workspace, and the
    /// key IS the merge identity — so without a workspace segment one tenant
    /// overwrites another's object, and its rows merge into the other's table
    /// at identical `_row_uid`s.
    #[test]
    fn two_workspaces_cannot_collide_on_one_key() {
        assert_ne!(
            object_key("z", ws(1), "ubereats", "ue", 2026, 8, "wf-1"),
            object_key("z", ws(2), "ubereats", "ue", 2026, 8, "wf-1"),
            "same workflow and period in two workspaces must not share a key"
        );
    }

    /// The workspace segment must sit BEFORE the period, or the source's
    /// right-to-left scan stops finding `YYYY.MM` as its own component.
    #[test]
    fn the_period_is_still_the_last_directory_segment() {
        let key = object_key("z", ws(7), "ubereats", "ue", 2026, 8, "wf-1");
        let mut parts: Vec<&str> = key.split('/').collect();
        parts.pop(); // the file
        assert_eq!(
            parts.last().copied(),
            Some("2026.08"),
            "the period must be the directory the report sits in: {key}"
        );
    }

    /// The key IS the merge identity: same workflow + period must produce the
    /// same key so a re-post merges, and a different workflow must not.
    #[test]
    fn the_key_is_the_idempotency_boundary() {
        let w = ws(1);
        let a = object_key("z", w, "ubereats", "ue", 2026, 8, "wf-1");
        assert_eq!(
            a,
            object_key("z", w, "ubereats", "ue", 2026, 8, "wf-1"),
            "a re-post must collide"
        );
        assert_ne!(
            a,
            object_key("z", w, "ubereats", "ue", 2026, 8, "wf-2"),
            "chunks must not"
        );
        assert_ne!(
            a,
            object_key("z", w, "ubereats", "ue", 2026, 9, "wf-1"),
            "periods must not"
        );
    }

    /// Rejected, not sanitized — a rewritten id lands the report under a name
    /// the caller did not choose, and the key is the merge identity.
    #[test]
    fn a_workflow_id_that_would_reshape_the_key_is_refused() {
        assert!(valid_workflow_id("wf-1"));
        assert!(valid_workflow_id("chunk_2026_08_a"));

        for bad in [
            "",
            "../escape",
            "a/b",
            "wf 1",
            "wf.1",
            "wf%2F",
            &"x".repeat(129),
        ] {
            assert!(!valid_workflow_id(bad), "`{bad}` must be refused");
        }
    }
}

#[cfg(test)]
mod pipeline_read_error_tests {
    use super::*;
    use oxy::config::ArtifactError;
    use oxy_shared::errors::OxyError;

    /// A customer's malformed `.airway.yml` is not a server fault, and the
    /// parse error is the only thing that tells them which line.
    ///
    /// This is the case that regressed. The read moved onto
    /// `ConfigManager::pipeline_definition`, which owns the disk arm too, and a
    /// blanket `map_err` sent every variant to `500 could not read the
    /// pipeline` — dropping the parse error and reporting the customer's typo
    /// as a platform failure. Nothing caught it: every arm compiled, and no
    /// test covered the 400.
    #[test]
    fn a_malformed_pipeline_is_the_customers_four_hundred() {
        let (status, body) = pipeline_read_error(
            "p.airway.yml",
            ArtifactError::Config(OxyError::ConfigurationError(
                "p.airway.yml: did not find expected key at line 4".to_string(),
            )),
        );
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            body.contains("line 4"),
            "the parse error names the line; dropping it is what made this a \
             server error to the customer: {body}"
        );
    }

    /// Everything else is transient and must stay retryable — a root this node
    /// does not hold, a half-mounted volume, a boundary that has not landed.
    /// `ArtifactError::retryable()` is the split, and `manager.rs` states the
    /// rule this pins: collapsing the two turns a 503 into a 500.
    #[test]
    fn an_unreadable_pipeline_stays_a_retryable_five_oh_three() {
        for err in [
            ArtifactError::WorkspaceUnavailable(
                "`p.airway.yml` exists but could not be read: EIO".to_string(),
            ),
            ArtifactError::WorkspaceUnavailable("no working copy on this node".to_string()),
        ] {
            let (status, body) = pipeline_read_error("p.airway.yml", err);
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert!(
                body.contains("Retry"),
                "must tell the caller to retry: {body}"
            );
        }
    }

    /// `ArtifactError::Config` is `#[from] OxyError`, so it carries an IO
    /// failure as readily as a parse one — `definition_from_disk` reaches
    /// `fs_link` → `validate_path_within_project`, which answers
    /// `OxyError::IOError("Failed to canonicalize project path")` on an EACCES
    /// or on the root vanishing mid-deploy.
    ///
    /// Keying the 400 on the wrapper variant made that a customer error: a
    /// node-local IO fault, answered non-retryable, with a message naming a
    /// parse that never ran. That is this branch's own subject pointed the
    /// wrong way, so it is pinned rather than left to the variant's shape.
    #[test]
    fn an_io_failure_wearing_config_is_still_ours_to_retry() {
        let (status, body) = pipeline_read_error(
            "p.airway.yml",
            ArtifactError::Config(OxyError::IOError(
                "Failed to canonicalize project path: Permission denied".to_string(),
            )),
        );
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "an EACCES on the root is not the customer's YAML: {body}"
        );
        assert!(
            !body.contains("could not be parsed"),
            "must not name a parse that never ran: {body}"
        );
    }

    /// The two answers must not be the same, which is the whole point.
    #[test]
    fn the_two_kinds_do_not_share_a_response() {
        let bad_yaml = pipeline_read_error(
            "p.airway.yml",
            ArtifactError::Config(OxyError::ConfigurationError("boom".to_string())),
        );
        let unreadable = pipeline_read_error(
            "p.airway.yml",
            ArtifactError::WorkspaceUnavailable("io".to_string()),
        );
        assert_ne!(bad_yaml.0, unreadable.0);
    }
}
