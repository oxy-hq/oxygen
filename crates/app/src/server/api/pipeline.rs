//! Compile-boundary-backed Airway pipeline (`.airway.yml`) listing.
//!
//! Airway pipelines are already compiled (`walker` → `CompiledRow::Pipeline` →
//! `airway_pipelines`). A legacy `/agentic-airway/files` lister in the
//! `agentic-http` crate (which can't reach `compiled_reader`) walked the
//! filesystem and 502d on a stateless serve node when the ide was down; it is
//! gone. This FleetOk endpoint serves the list from the boundary, falling
//! through to the filesystem in local / not-yet-promoted mode — mirroring
//! `automation.rs`.

use axum::extract::{self, Query};
use axum::http::StatusCode;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};

use crate::server::api::middlewares::workspace_context::WorkspaceManagerReadOnly;

/// Decode a `path_b64` route param tolerantly — accept both the
/// `URL_SAFE_NO_PAD` form our listers emit and the standard-padded form the FE
/// `btoa` path historically produced. Shared by the automation + pipeline
/// single-file fetches. Returns the decoded workspace-relative path.
pub(crate) fn decode_path_b64(path_b64: &str) -> Option<String> {
    let bytes = URL_SAFE_NO_PAD.decode(path_b64).ok().or_else(|| {
        let pad = (4 - path_b64.len() % 4) % 4;
        STANDARD
            .decode(format!("{path_b64}{}", "=".repeat(pad)))
            .ok()
    })?;
    String::from_utf8(bytes).ok()
}

#[derive(Debug, Deserialize)]
pub struct ListPipelinesQuery {
    /// Active branch hint; the boundary only serves the workspace default branch.
    pub branch: Option<String>,
}

/// One pipeline file, in the shape the FE
/// consumes (the removed `/agentic-airway/files` lister used it too): a
/// workspace-relative `path` and its `URL_SAFE_NO_PAD` base64 (`path_b64`).
#[derive(Debug, Serialize)]
pub struct PipelineFile {
    pub path: String,
    pub path_b64: String,
    /// `source.kind` — what the UI keys source-specific surfaces on (the
    /// report-upload tab is only meaningful for a `ubereats` pipeline).
    ///
    /// Read from the compiled definition when there is one, and parsed
    /// straight out of the YAML on the filesystem fallback below, so the two
    /// backends answer the same kind for the same pipeline —
    /// `the_filesystem_fallback_reads_the_same_kind_as_the_boundary` pins
    /// that. This doc used to say the fallback returned `None`, which was
    /// true before the fallback learned to read it and is exactly the sort of
    /// claim a gating decision gets reasoned from.
    ///
    /// `None` now means only that the file could not be read or did not
    /// parse. A surface gated on this stays hidden in that case rather than
    /// appearing and then failing — and the server refuses an upload for a
    /// non-`ubereats` pipeline regardless, so a missing kind costs a hidden
    /// tab, never a wrong write.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_kind: Option<String>,
}

/// Test-only: the listing itself always goes through `to_file_with_kind`,
/// with the kind coming from `PipelineEntry` (both origins fill it in core).
#[cfg(test)]
fn to_file(relative_path: String) -> PipelineFile {
    to_file_with_kind(relative_path, None)
}

fn to_file_with_kind(relative_path: String, source_kind: Option<String>) -> PipelineFile {
    let path_b64 = URL_SAFE_NO_PAD.encode(relative_path.as_bytes());
    PipelineFile {
        path: relative_path,
        path_b64,
        source_kind,
    }
}

/// `source.kind` out of a pipeline definition — now shared with both listing
/// origins in core (`oxy::config::pipeline_source_kind`), so the compiled row
/// and the working-copy read cannot drift. The handler no longer extracts it
/// (entries arrive with the kind filled in); the pin tests still exercise the
/// extraction under its old name.
#[cfg(test)]
use oxy::config::pipeline_source_kind as source_kind_of;

/// `GET /{workspace_id}/airway-pipelines` — FleetOk. Boundary first, FS fallback.
pub async fn list_pipelines(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Query(_query): Query<ListPipelinesQuery>,
) -> Result<extract::Json<Vec<PipelineFile>>, StatusCode> {
    let pipelines = workspace_manager
        .config_manager
        .list_pipelines()
        .await
        .map_err(|e| {
            tracing::warn!(
                workspace_id = %workspace_manager.workspace_id,
                error = %e,
                "list_pipelines failed"
            );
            if e.retryable() {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        })?;
    Ok(extract::Json(
        pipelines
            .into_iter()
            .map(|p| to_file_with_kind(p.file_path, p.source_kind))
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_kind_is_read_from_the_definition() {
        let def = serde_json::json!({
            "name": "ue",
            "source": { "kind": "ubereats", "config": { "base_path": "s3://z" } },
        });
        assert_eq!(source_kind_of(&def).as_deref(), Some("ubereats"));
    }

    /// Absent rather than empty-string: the UI gates a surface on this, and
    /// `Some("")` would read as "there is a kind, and it is nothing".
    #[test]
    fn a_definition_without_a_kind_yields_none() {
        for def in [
            serde_json::json!({ "name": "x" }),
            serde_json::json!({ "source": {} }),
            serde_json::json!({ "source": { "kind": "" } }),
        ] {
            assert_eq!(source_kind_of(&def), None, "{def}");
        }
    }

    /// `to_file` itself carries no kind — the FS listing supplies one by
    /// reading the YAML, which is what keeps a source-gated surface visible in
    /// local mode.
    ///
    /// Local NEVER promotes a revision, so a `source_kind` that only came from
    /// the compile boundary was absent for every pipeline forever, and the
    /// Reports tab could not appear locally at all. "Hidden until compiled"
    /// and "hidden" are the same thing when nothing ever compiles.
    #[test]
    fn to_file_alone_carries_no_kind() {
        assert_eq!(to_file("pipelines/x.airway.yml".into()).source_kind, None);
    }

    /// The kind the FS fallback reads must match what the boundary would
    /// report for the same file, or a surface appears in one mode and not the
    /// other.
    #[test]
    fn the_filesystem_fallback_reads_the_same_kind_as_the_boundary() {
        let yaml = "name: ue\nsource:\n  kind: ubereats\n  config:\n    base_path: s3://z\n";
        let from_fs: serde_json::Value = serde_yaml::from_str(yaml).expect("valid yaml");
        assert_eq!(source_kind_of(&from_fs).as_deref(), Some("ubereats"));

        // A pipeline that will not parse stays listed, without a kind — a
        // broken file should still be visible in the sidebar so it can be
        // fixed, rather than vanishing.
        assert!(serde_yaml::from_str::<serde_json::Value>("name: [unclosed").is_err());
    }
}

/// One marketplace the `sp_api` connector can reach.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpApiMarketplace {
    pub id: &'static str,
    pub country: &'static str,
    pub label: &'static str,
}

/// `GET /{workspace_id}/airway-pipelines/sp-api/marketplaces` — FleetOk.
///
/// Serves `agentic_airway::source_factory::NA_MARKETPLACES` so the wizard's
/// marketplace picker has ONE source of truth. The list was briefly duplicated
/// in TypeScript, which is a slow trap rather than a fast one: `SpApiSource`
/// pins the NA endpoint today, and when it learns to take a host the Rust list
/// widens — a stale copy in the frontend would then be the only thing refusing
/// a marketplace the connector accepts, with nothing failing to point at it.
///
/// Takes NO extractor, deliberately. `WorkspaceManagerReadOnly` is not inert:
/// it rejects with the needs-recompile 503 whenever the middleware left no
/// manager in the extensions, so adding it "just to sit under the workspace
/// scope" would make the one handler here that genuinely cannot fail the only
/// one that can — and it would fail on a fresh or just-deployed workspace,
/// which is exactly when someone is creating their first pipeline. The route
/// stays under the workspace path either way; auth and middleware gating are
/// unchanged.
pub async fn list_sp_api_marketplaces() -> extract::Json<Vec<SpApiMarketplace>> {
    extract::Json(
        agentic_airway::source_factory::NA_MARKETPLACES
            .iter()
            .map(|m| SpApiMarketplace {
                id: m.id,
                country: m.country,
                label: m.label,
            })
            .collect(),
    )
}
