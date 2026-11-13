//! Onboarding data on a cloud role: uploaded files and the demo dataset go
//! into the workspace's Airhouse, not into its checkout.
//!
//! `OXY_ROLE=all` (a laptop, the CLI, `oxy seed`) keeps the file DuckDB it has
//! always had: uploads land in `<workspace>/.db/` and the demo reads
//! `.db/oxymart.csv`. Every other role is a cloud deployment, where a
//! workspace's data lives in Airhouse; there the bytes come from the request
//! or from the binary, are loaded through `airhouse::import`, and the
//! workspace's database is `type: airhouse_managed`. A workspace without a
//! tenant is given one first (idempotent), so nothing here needs a person.

use airhouse::import::{IfExists, ImportError, ImportFile, ImportOutcome, WorkspaceImporter};
use axum::Json;
use axum::extract::Multipart;
use axum::http::StatusCode;
use oxy_app_core::process_role::Role;
use uuid::Uuid;

use super::dto::{SkippedUpload, UploadDestination, UploadWarehouseFilesResponse, UploadedTable};
use super::ops::{
    MAX_FILE_BYTES, MAX_FILES_PER_REQUEST, drain_field, has_supported_extension, parse_subdir,
    sanitise_upload_filename, stream_field_to_file,
};

/// Whether this process keeps a workspace's onboarding data in Airhouse:
/// every role but the all-in-one local one.
pub(super) fn data_in_airhouse(role: Role) -> bool {
    role != Role::All
}

/// The status an import failure answers with.
pub(super) fn import_status(e: &ImportError) -> StatusCode {
    use airhouse::EnsureTenantError;
    match e {
        ImportError::NotConfigured | ImportError::Tenant(EnsureTenantError::NotConfigured) => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        ImportError::AlreadyExists(_)
        | ImportError::NoTenant(_)
        | ImportError::Tenant(EnsureTenantError::PendingDelete) => StatusCode::CONFLICT,
        ImportError::InvalidName(_) => StatusCode::BAD_REQUEST,
        ImportError::Read(_) => StatusCode::UNPROCESSABLE_ENTITY,
        ImportError::Tenant(_) | ImportError::Load(_) | ImportError::Credential(_) => {
            StatusCode::BAD_GATEWAY
        }
    }
}

fn refused(e: ImportError) -> (StatusCode, String) {
    (import_status(&e), e.to_string())
}

/// A scratch file in the process's temp directory, removed when dropped.
fn scratch_file() -> Result<tempfile::NamedTempFile, (StatusCode, String)> {
    tempfile::Builder::new()
        .prefix("oxy-airhouse-import-")
        .tempfile()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to create a scratch file for the upload: {e}"),
            )
        })
}

/// `upload-warehouse-files` on a cloud role: load each `.csv` / `.parquet`
/// field into a table of the workspace's Airhouse, named the way a file
/// DuckDB names the file (`airhouse::import::ImportFile::named_like_a_file_database`).
///
/// The same limits as the file upload (25 files, 200 MiB each, 500 MiB in
/// all) and the same refusal of a name already taken (409). Each file is
/// streamed to a scratch file outside any checkout and removed once loaded.
///
/// The files load one after another inside the request, each in its own
/// transaction, and the answer waits for the last. A request that outlasts
/// the ingress is cut off mid-way with the files before the cut committed and
/// the rest not: on staging (2026-11-09) 17 files totalling ~56 MB in one
/// request were answered 408 part-way. Send large data in several requests;
/// a re-sent file that already loaded answers 409.
pub(super) async fn upload_into_airhouse(
    importer: &dyn WorkspaceImporter,
    workspace_id: Uuid,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<UploadWarehouseFilesResponse>), (StatusCode, String)> {
    // Before any bytes are read: a deployment without Airhouse, or a tenant
    // that cannot be provisioned, is told so at once.
    importer.ready(workspace_id).await.map_err(refused)?;

    let mut files_out: Vec<String> = Vec::new();
    let mut skipped: Vec<SkippedUpload> = Vec::new();
    let mut tables: Vec<UploadedTable> = Vec::new();
    let mut total_bytes: u64 = 0;
    let mut files_seen: usize = 0;

    while let Some(field) = multipart.next_field().await.map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("Failed to parse multipart body: {e}"),
        )
    })? {
        match field.name().unwrap_or("") {
            "subdir" => {
                // Accepted for the request shape; nothing is written to a
                // directory here. Still validated, as the file upload does.
                let raw = field.text().await.map_err(|e| {
                    (
                        StatusCode::BAD_REQUEST,
                        format!("Failed to read subdir field: {e}"),
                    )
                })?;
                parse_subdir(&raw)?;
            }
            "file" => {
                files_seen += 1;
                if files_seen > MAX_FILES_PER_REQUEST {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        format!("Too many files in upload request (max {MAX_FILES_PER_REQUEST})"),
                    ));
                }
                let raw_name = field.file_name().unwrap_or("").to_string();
                let sanitised = match sanitise_upload_filename(&raw_name) {
                    Ok(n) => n,
                    Err(reason) => {
                        skipped.push(SkippedUpload {
                            name: raw_name,
                            reason: reason.to_string(),
                        });
                        drain_field(field).await?;
                        continue;
                    }
                };
                if !has_supported_extension(&sanitised) {
                    skipped.push(SkippedUpload {
                        name: sanitised,
                        reason: "unsupported_extension".to_string(),
                    });
                    drain_field(field).await?;
                    continue;
                }

                let scratch = scratch_file()?;
                let file = match ImportFile::named_like_a_file_database(
                    scratch.path(),
                    &sanitised,
                    IfExists::Fail,
                ) {
                    Ok(file) => file,
                    Err(e) => {
                        skipped.push(SkippedUpload {
                            name: sanitised,
                            reason: e.to_string(),
                        });
                        drain_field(field).await?;
                        continue;
                    }
                };
                let bytes =
                    stream_field_to_file(field, scratch.path(), MAX_FILE_BYTES, &mut total_bytes)
                        .await?;
                let outcome = importer
                    .import(workspace_id, &file)
                    .await
                    .map_err(|e| match e {
                        ImportError::AlreadyExists(table) => (
                            StatusCode::CONFLICT,
                            format!(
                                "A table named '{table}' already exists in this workspace's \
                                 Airhouse. Rename the file or drop the table before re-uploading."
                            ),
                        ),
                        other => refused(other),
                    })?;
                tracing::info!(
                    %workspace_id,
                    filename = %sanitised,
                    bytes,
                    table = %outcome.table,
                    rows = outcome.rows,
                    "Loaded an uploaded warehouse file into Airhouse"
                );
                tables.push(uploaded_table(&sanitised, &outcome));
                files_out.push(sanitised);
            }
            _ => drain_field(field).await?,
        }
    }

    if files_out.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "No supported files uploaded. Upload at least one .csv or .parquet file.".to_string(),
        ));
    }

    Ok((
        StatusCode::CREATED,
        Json(UploadWarehouseFilesResponse {
            subdir: String::new(),
            files: files_out,
            skipped,
            destination: UploadDestination::Airhouse,
            tables,
        }),
    ))
}

fn uploaded_table(file: &str, outcome: &ImportOutcome) -> UploadedTable {
    UploadedTable {
        file: file.to_string(),
        schema: outcome.table.schema.clone(),
        table: outcome.table.table.clone(),
        aliases: outcome.aliases.iter().map(ToString::to_string).collect(),
        rows: outcome.rows,
    }
}

#[cfg(test)]
#[path = "airhouse_tests.rs"]
mod tests;
