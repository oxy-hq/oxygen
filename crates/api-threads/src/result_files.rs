use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use axum::body::Body;
use axum::extract::Path;
use axum::http::{StatusCode, header};
use axum::response::Response;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::WorkingCopy;
use oxy::connector::load_result;
use oxy_app::surface::WorkspaceManagerReadOnly;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use tokio::fs::File;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

/// Convert Arrow result to Parquet and save to results directory
///
/// This utility function:
/// 1. Loads the Arrow result from the temp file
/// 2. Gets the results directory from the project manager
/// 3. Generates a new UUID-based filename with .parquet extension
/// 4. Converts and writes the data as Parquet format
/// 5. Cleans up the temporary file
/// 6. Returns the filename
///
/// # Arguments
/// * `workspace_manager` - The project manager containing config
/// * `temp_file_path` - Path to the temporary Arrow result file
///
/// # Returns
/// * `Ok(String)` - The filename of the Parquet result file
/// * `Err(String)` - Error message if any step fails
pub async fn store_result_file(
    workspace_manager: &WorkspaceManager<WorkingCopy>,
    temp_file_path: &str,
) -> Result<String, String> {
    let (batches, schema) =
        load_result(temp_file_path).map_err(|e| format!("Failed to load Arrow result: {}", e))?;

    // The capability-free accessor, paired with the writer in `typed_stream`.
    // A result parquet is a runtime artifact, not a workspace file — it is
    // mirrored to S3 and read back through that mirror — so the local copy has
    // to be writable on a replica too.
    let results_dir = workspace_manager.config_manager.results_dir();

    // Generate a new filename with .parquet extension
    let file_name = format!("{}.parquet", uuid::Uuid::new_v4());
    let dest_path = results_dir.join(&file_name);

    write_parquet(&dest_path, &batches, schema)
        .map_err(|e| format!("Failed to write Parquet file: {}", e))?;

    // Mirror to S3 so a serve replica OTHER than this one can still serve the
    // file via GET /{ws}/results/files/{id} (round-robin fleet). Best-effort;
    // no-op when no bucket (dev/single-node). See server::runtime_artifact.
    if let Ok(bytes) = tokio::fs::read(&dest_path).await {
        let key = oxy_app::surface::runtime_artifact::result_key(
            workspace_manager.workspace_id,
            &file_name,
        );
        oxy_app::surface::runtime_artifact::mirror(&key, bytes, "application/vnd.apache.parquet")
            .await;
    }

    let _ = tokio::fs::remove_file(temp_file_path).await;

    Ok(file_name)
}

/// Write record batches to a Parquet file
fn write_parquet(
    file_path: &std::path::Path,
    batches: &[RecordBatch],
    schema: SchemaRef,
) -> Result<(), String> {
    let file =
        std::fs::File::create(file_path).map_err(|e| format!("Failed to create file: {}", e))?;

    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .build();

    let mut writer = ArrowWriter::try_new(file, schema, Some(props))
        .map_err(|e| format!("Failed to create Parquet writer: {}", e))?;

    for batch in batches {
        writer
            .write(batch)
            .map_err(|e| format!("Failed to write batch: {}", e))?;
    }

    writer
        .close()
        .map_err(|e| format!("Failed to close writer: {}", e))?;

    Ok(())
}

/// Serve Parquet result files for query results
///
/// This endpoint streams Parquet files from the results directory
/// Files are named with UUIDs and stored in the state directory
pub async fn get_result_file(
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Path((_workspace_id, file_name)): Path<(Uuid, String)>,
) -> Result<Response, StatusCode> {
    if !file_name.ends_with(".parquet") {
        tracing::warn!("Invalid file format: {}", file_name);
        return Err(StatusCode::BAD_REQUEST);
    }

    // Extract the UUID part and validate it to prevent directory traversal
    let file_uuid = file_name
        .strip_suffix(".parquet")
        .ok_or(StatusCode::BAD_REQUEST)?;

    if Uuid::parse_str(file_uuid).is_err() {
        tracing::warn!("Invalid UUID in filename: {}", file_uuid);
        return Err(StatusCode::BAD_REQUEST);
    }

    // Must match the writer's directory, or every cross-request read misses the
    // local fast path and falls all the way through to S3.
    let results_dir = workspace_manager.config_manager.results_dir();

    // Construct the full file path
    let file_path = results_dir.join(&file_name);

    // Cross-node read-through: on the stateless serve fleet the file may have
    // been written on a DIFFERENT replica, so a local miss is expected — fall
    // back to the S3 mirror before 404ing. No-op store + always-miss fetch in
    // dev (no bucket) means this collapses to the old "404 on local miss".
    if !file_path.exists() {
        let key = oxy_app::surface::runtime_artifact::result_key(
            workspace_manager.workspace_id,
            &file_name,
        );
        if let Some(bytes) = oxy_app::surface::runtime_artifact::fetch(&key).await {
            let len = bytes.len() as u64;
            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/vnd.apache.parquet")
                .header(header::CONTENT_LENGTH, len)
                .header(
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{}\"", file_name),
                )
                .body(Body::from(bytes))
                .map_err(|e| {
                    tracing::error!("Failed to build S3 result response: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR
                });
        }
        tracing::warn!("Result file not found (local + S3): {:?}", file_path);
        return Err(StatusCode::NOT_FOUND);
    }

    let file = File::open(&file_path).await.map_err(|e| {
        tracing::error!("Failed to open result file: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Get file metadata for content length
    let metadata = file.metadata().await.map_err(|e| {
        tracing::error!("Failed to get file metadata: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    // Build response with appropriate headers
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/vnd.apache.parquet")
        .header(header::CONTENT_LENGTH, metadata.len())
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", file_name),
        )
        .body(body)
        .map_err(|e| {
            tracing::error!("Failed to build response: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(response)
}

/// Delete a result file
///
/// This endpoint allows cleanup of temporary result files
pub async fn delete_result_file(
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Path((_workspace_id, file_id)): Path<(Uuid, String)>,
) -> Result<StatusCode, StatusCode> {
    if !file_id.ends_with(".parquet") {
        tracing::warn!("Invalid file format for deletion: {}", file_id);
        return Err(StatusCode::BAD_REQUEST);
    }

    // Extract the UUID part
    let file_uuid = file_id
        .strip_suffix(".parquet")
        .ok_or(StatusCode::BAD_REQUEST)?;

    // Validate it's a valid UUID
    if Uuid::parse_str(file_uuid).is_err() {
        tracing::warn!("Invalid UUID in file_id for deletion: {}", file_uuid);
        return Err(StatusCode::BAD_REQUEST);
    }

    // Every write path mirrors the parquet to S3 (`store_result_file`,
    // `typed_stream`), and `get_result_file` reads through to that mirror on a
    // local miss — which is what makes the GET fleet-safe. The delete never
    // removed it, so a 204 here was followed by a 200 from the same `file_id` on
    // any replica: the user is told the file is gone and it keeps being served.
    //
    // Removal comes first and is unconditional, because the node handling the
    // DELETE is often not the node holding the local copy. Gating it behind the
    // local unlink would leave the durable copy undeletable from anywhere else.
    let mirror_removed = oxy_app::surface::runtime_artifact::remove(
        &oxy_app::surface::runtime_artifact::result_key(_workspace_id, &file_id),
    )
    .await;

    // The local copy, on whichever node has one. An absent results dir is not a
    // failure here — a replica that never held the parquet has still satisfied
    // the request once the mirror is gone.
    let file_path = workspace_manager
        .config_manager
        .results_dir()
        .join(&file_id);
    let local_removed = if file_path.exists() {
        tokio::fs::remove_file(&file_path).await.map_err(|e| {
            tracing::error!("Failed to delete result file: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        true
    } else {
        false
    };

    // Nothing anywhere: the caller named a file that does not exist.
    if !local_removed && !mirror_removed {
        tracing::warn!("Result file not found for deletion: {}", file_id);
        return Err(StatusCode::NOT_FOUND);
    }

    tracing::info!(
        file_id,
        local_removed,
        mirror_removed,
        "deleted result file"
    );
    Ok(StatusCode::NO_CONTENT)
}
