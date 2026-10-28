use std::path::{Path, PathBuf};

use crate::config::model::Dimension;
use crate::theme::*;
use aes_gcm::aead::{Aead, Generate, Nonce as AeadNonce};
use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
use arrow::array::RecordBatch;
use async_stream::stream;
use axum::response::sse::Event;
use base64::{Engine as _, engine::general_purpose};
use csv::StringRecord;
use duckdb::Connection;
use futures::Stream;
use oxy_shared::errors::OxyError;
use serde::Serialize;
use slugify::slugify;
use syntect::{
    easy::HighlightLines,
    highlighting::{Style, ThemeSet},
    parsing::SyntaxSet,
    util::{LinesWithEndings, as_24_bit_terminal_escaped},
};
use tokio::sync::mpsc;
use tokio::task::spawn_blocking;
use tokio_util::sync::CancellationToken;

pub const MAX_DISPLAY_ROWS: usize = 100;
pub const MAX_OUTPUT_LENGTH: usize = 1000;

/// Re-export of [`oxy_platform::secrets::get_encryption_key`] for source
/// compatibility — the implementation moved to the `oxy-platform` crate so
/// `airhouse` and other leaf integrations can use it without pulling in the
/// full `oxy` crate.
pub use oxy_platform::secrets::{ENCRYPTION_KEY_VAR, get_encryption_key, parse_encryption_key};

pub fn truncate_with_ellipsis(s: &str, max_width: Option<usize>) -> String {
    // We should truncate at grapheme-boundary and compute character-widths,
    // yet the dependencies on unicode-segmentation and unicode-width are
    // not worth it.
    let mut chars = s.chars();
    let max_width = max_width.unwrap_or(MAX_OUTPUT_LENGTH) - 1;
    let mut prefix = (&mut chars).take(max_width).collect::<String>();
    if chars.next().is_some() {
        prefix.push('…');
    }
    prefix
}

pub fn truncate_datasets(
    batches: &Vec<RecordBatch>,
    max_display_rows: Option<usize>,
) -> (Vec<RecordBatch>, bool) {
    let max_rows = max_display_rows.unwrap_or(MAX_DISPLAY_ROWS);
    if !batches.is_empty() && batches[0].num_rows() > max_rows {
        return (vec![batches[0].slice(0, max_rows)], true);
    }
    (batches.to_vec(), false)
}

pub fn format_table_output(table: &str, truncated: bool) -> String {
    if truncated {
        format!(
            "{}\n{}",
            format!(
                "Results have been truncated. Showing only the first {} rows.",
                MAX_DISPLAY_ROWS
            ),
            table
        )
    } else {
        table.to_string()
    }
}

pub fn print_colored_sql(sql: &str) {
    println!("{}", "\nSQL query:".primary());
    println!("{}", get_colored_sql(sql));
    println!();
}

pub fn get_colored_sql(sql: &str) -> String {
    let ps = SyntaxSet::load_defaults_newlines();
    let ts = ThemeSet::load_defaults();
    let syntax = ps.find_syntax_by_extension("sql").unwrap();
    let mut h = HighlightLines::new(syntax, &ts.themes["base16-ocean.dark"]);

    let mut colored_sql = String::new();
    for line in LinesWithEndings::from(sql) {
        let ranges: Vec<(Style, &str)> = h.highlight_line(line, &ps).unwrap();
        let escaped = as_24_bit_terminal_escaped(&ranges[..], false);
        colored_sql.push_str(&escaped);
    }
    colored_sql
}

pub fn get_relative_path(path: PathBuf, root: PathBuf) -> Result<String, OxyError> {
    let relative_path = path
        .strip_prefix(root)
        .map_err(|_| OxyError::ArgumentError("Could not get relative path".to_string()))?;
    Ok(relative_path.to_string_lossy().to_string())
}

pub fn variant_eq<T>(a: &T, b: &T) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

pub fn get_file_directories<P: AsRef<Path>>(file_path: P) -> Result<P, OxyError> {
    create_parent_dirs(&file_path).map_err(|e| {
        OxyError::IOError(format!(
            "Error creating directories for path '{}': {}",
            file_path.as_ref().display(),
            e
        ))
    })?;
    Ok(file_path)
}

pub fn create_parent_dirs<P: AsRef<Path>>(path: P) -> std::io::Result<()> {
    if let Some(parent) = path.as_ref().parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

pub fn list_by_sub_extension(dir: &PathBuf, sub_extension: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(list_by_sub_extension(&path, sub_extension));
            } else if path.is_file()
                // && path.extension().and_then(|s| s.to_str()) == Some("")
                && path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(|s| s.ends_with(sub_extension))
                    .unwrap_or(false)
            {
                files.push(path);
            }
        }
    }

    files
}

pub fn file_path_to_source_id<P: AsRef<Path>>(file_path: P) -> String {
    file_path.as_ref().to_string_lossy().to_string()
}

pub async fn asyncify<F, T>(f: F) -> Result<T, OxyError>
where
    F: FnOnce() -> Result<T, OxyError> + Send + 'static,
    T: Send + 'static,
{
    match spawn_blocking(f).await {
        Ok(res) => res,
        Err(err) => Err(OxyError::RuntimeError(format!(
            "Failed to spawn blocking task: {err}"
        ))),
    }
}

pub fn extract_csv_dimensions(
    path: &std::path::Path,
) -> Result<Vec<Dimension>, oxy_shared::errors::OxyError> {
    let conn = Connection::open_in_memory().map_err(|e| {
        oxy_shared::errors::OxyError::RuntimeError(format!("Failed to open in-memory DuckDB: {e}"))
    })?;

    let sql = format!(
        "CREATE VIEW auto_csv AS SELECT * FROM read_csv_auto('{}', SAMPLE_SIZE=10000, ALL_VARCHAR=FALSE);",
        path.display()
    );
    conn.execute(&sql, []).map_err(|e| {
        oxy_shared::errors::OxyError::RuntimeError(format!(
            "DuckDB failed to read CSV {}: {}",
            path.display(),
            e
        ))
    })?;

    let mut stmt = conn
        .prepare("PRAGMA table_info('auto_csv');")
        .map_err(|e| {
            oxy_shared::errors::OxyError::RuntimeError(format!(
                "DuckDB failed to prepare schema query: {e}"
            ))
        })?;
    let mut rows = stmt.query([]).map_err(|e| {
        oxy_shared::errors::OxyError::RuntimeError(format!("DuckDB failed to query schema: {e}"))
    })?;

    let mut columns = Vec::new();
    while let Some(row) = rows.next().map_err(|e| {
        oxy_shared::errors::OxyError::RuntimeError(format!("DuckDB failed to read schema row: {e}"))
    })? {
        let name: String = row.get(1).map_err(|e| {
            oxy_shared::errors::OxyError::RuntimeError(format!("DuckDB schema row: {e}"))
        })?;
        let dtype: String = row.get(2).map_err(|e| {
            oxy_shared::errors::OxyError::RuntimeError(format!("DuckDB schema row: {e}"))
        })?;
        columns.push((name, dtype));
    }

    let mut samples: Vec<Vec<String>> = vec![Vec::new(); columns.len()];
    const MAX_SAMPLES_PER_COLUMN: usize = 1;
    if let Ok(mut reader) = csv::Reader::from_path(path) {
        for _ in 0..5 {
            let mut row = StringRecord::new();
            if !reader
                .read_record(&mut row)
                .map_err(|e| OxyError::RuntimeError(format!("CSV read error: {e}")))?
            {
                break;
            }
            for (i, field) in row.iter().enumerate().take(columns.len()) {
                if samples[i].len() < MAX_SAMPLES_PER_COLUMN {
                    samples[i].push(field.to_string());
                }
            }
        }
    }

    let dimensions = columns
        .into_iter()
        .enumerate()
        .map(|(i, (name, dtype))| Dimension {
            name,
            description: None,
            synonyms: None,
            sample: samples.get(i).cloned().unwrap_or_default(),
            data_type: Some(dtype),
            is_partition_key: None,
        })
        .collect();

    Ok(dimensions)
}

pub fn try_unwrap_arc_mutex<T>(arc: std::sync::Arc<std::sync::Mutex<T>>) -> Result<T, OxyError> {
    std::sync::Arc::try_unwrap(arc)
        .map_err(|_| OxyError::RuntimeError("Failed to unwrap arc mutex".to_string()))?
        .into_inner()
        .map_err(|err| OxyError::RuntimeError(format!("Failed to unwrap arc mutex: {err}")))
}

pub async fn try_unwrap_arc_tokio_mutex<T>(
    arc: std::sync::Arc<tokio::sync::Mutex<T>>,
) -> Result<T, OxyError> {
    Ok(std::sync::Arc::try_unwrap(arc)
        .map_err(|_| OxyError::RuntimeError("Failed to unwrap arc mutex".to_string()))?
        .into_inner())
}

/// Converts a file path to a valid OpenAI function name.
///
/// This function takes a file path and transforms it into a slug-friendly string
/// suitable for use as an OpenAI function name. The transformation process:
/// 1. Converts the path to be relative to the project root (if possible)
/// 2. Removes the file extension
/// 3. Slugifies the result using underscores as separators
/// 4. Limits the length to 60 characters
///
/// # Arguments
///
/// * `file_path` - A reference to a PathBuf representing the file path to convert
///
/// # Returns
///
/// * `Ok(String)` - A slugified function name derived from the file path
/// * `Err(OxyError)` - If the project path cannot be found or other processing errors occur
///
/// ```
pub fn to_openai_function_name(
    file_path: &PathBuf,
    project_path: &PathBuf,
) -> Result<String, OxyError> {
    // Get the relative path from project root, falling back to the original path
    let relative_path = file_path.strip_prefix(project_path).unwrap_or(file_path);

    let path_without_extension = remove_file_extension(relative_path);

    let path_string = path_without_extension.to_string_lossy();
    let function_name = slugify!(&path_string, separator = "_", max_length = 60);

    Ok(function_name)
}

/// Removes the file extension from a path, returning a new PathBuf.
///
/// # Arguments
///
/// * `path` - The path to remove the extension from
///
/// # Returns
///
/// A new PathBuf with the file extension removed
fn remove_file_extension(path: &Path) -> PathBuf {
    let mut result = path.to_path_buf();

    if let Some(file_name) = path.file_name() {
        let file_str = file_name.to_string_lossy();
        if let Some(dot_index) = file_str.find('.') {
            let name_without_ext = &file_str[..dot_index];
            result.set_file_name(name_without_ext);
        }
    }

    result
}

pub fn create_sse_broadcast<T: std::fmt::Debug + Serialize + Clone>(
    items: Vec<T>,
    mut receiver: tokio::sync::broadcast::Receiver<T>,
) -> impl futures::Stream<Item = Result<Event, axum::Error>> {
    stream! {
        tracing::debug!("SSE broadcast started with {} items", items.len());
        for item in items {
            match serde_json::to_string(&item) {
                Ok(json_data) => {
                    yield Ok::<_, axum::Error>(
                        Event::default()
                            .event("message")
                            .data(json_data)
                    );
                }
                Err(e) => {
                    tracing::error!("Failed to serialize data: {}", e);
                    let error_event = serde_json::json!({
                        "message": "Error serializing data"
                    });
                    yield Ok::<_, axum::Error>(
                        Event::default()
                            .event("error")
                            .data(error_event.to_string())
                    );
                }
            }
        }
        tracing::debug!("SSE broadcast waiting for new items...");
        loop {
            let item = receiver.recv().await;
            if let Err(e) = item {
                tracing::warn!("Error receiving item from broadcast channel: {:?}", e);
                break;
            }
            let item = item.unwrap();

            tracing::debug!("Received new item for SSE broadcast: {:?}", item);
            match serde_json::to_string(&item) {
                Ok(json_data) => {
                    yield Ok::<_, axum::Error>(
                        Event::default()
                            .event("message")
                            .data(json_data)
                    );
                }
                Err(e) => {
                    tracing::error!("Failed to serialize data: {}", e);
                    let error_event = serde_json::json!({
                        "message": "Error serializing data"
                    });
                    yield Ok::<_, axum::Error>(
                        Event::default()
                            .event("error")
                            .data(error_event.to_string())
                    );
                }
            }
        }

        tracing::debug!("SSE broadcast ended, no more items to process.");
    }
}

pub fn create_sse_stream<T: Serialize>(
    mut receiver: mpsc::Receiver<T>,
) -> impl futures::Stream<Item = Result<Event, axum::Error>> {
    stream! {
        while let Some(item) = receiver.recv().await {
            match serde_json::to_string(&item) {
                Ok(json_data) => {
                    yield Ok::<_, axum::Error>(
                        Event::default()
                            .event("message")
                            .data(json_data)
                    );
                }
                Err(e) => {
                    tracing::error!("Failed to serialize data: {}", e);
                    let error_event = serde_json::json!({
                        "message": "Error serializing data"
                    });
                    yield Ok::<_, axum::Error>(
                        Event::default()
                            .event("error")
                            .data(error_event.to_string())
                    );
                }
            }
        }
    }
}

pub fn create_sse_stream_with_cancellation<T: Serialize>(
    mut receiver: mpsc::Receiver<T>,
    cancellation_token: CancellationToken,
) -> impl futures::Stream<Item = Result<Event, axum::Error>> {
    stream! {
        loop {
            tokio::select! {
                item = receiver.recv() => {
                    match item {
                        Some(item) => {
                            match serde_json::to_string(&item) {
                                Ok(json_data) => {
                                    yield Ok::<_, axum::Error>(
                                        Event::default()
                                            .event("message")
                                            .data(json_data)
                                    );
                                }
                                Err(e) => {
                                    tracing::error!("Failed to serialize data: {}", e);
                                    let error_event = serde_json::json!({
                                        "message": "Error serializing data"
                                    });
                                    yield Ok::<_, axum::Error>(
                                        Event::default()
                                            .event("error")
                                            .data(error_event.to_string())
                                    );
                                }
                            }
                        }
                        None => break,
                    }
                }
                _ = cancellation_token.cancelled() => {
                    tracing::debug!("Stream cancelled");
                    break;
                }
            }
        }
    }
}

pub fn create_sse_stream_from_stream<T: Serialize>(
    mut stream: impl Stream<Item = T> + Unpin,
) -> impl futures::Stream<Item = Result<Event, axum::Error>> {
    stream! {
        while let Some(item) =futures::StreamExt::next( &mut stream).await {
            match serde_json::to_string(&item) {
                Ok(json_data) => {
                    yield Ok::<_, axum::Error>(
                        Event::default()
                            .event("message")
                            .data(json_data)
                    );
                }
                Err(e) => {
                    tracing::error!("Failed to serialize data: {}", e);
                    let error_event = serde_json::json!({
                        "message": "Error serializing log data",
                    });
                    yield Ok::<_, axum::Error>(
                        Event::default()
                            .event("error")
                            .data(error_event.to_string())
                    );
                }
            }
        }
    }
}

pub fn get_file_stem<P: AsRef<Path>>(path: P) -> String {
    path.as_ref()
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or(path.as_ref().to_string_lossy().to_string())
}

pub fn encrypt_value(key: &[u8; 32], value: &str) -> Result<String, OxyError> {
    let cipher = Aes256Gcm::new(&Key::<Aes256Gcm>::from(*key));
    let nonce = AeadNonce::<Aes256Gcm>::generate();

    let ciphertext = cipher
        .encrypt(&nonce, value.as_bytes())
        .map_err(|e| OxyError::SecretManager(format!("Encryption failed: {e}")))?;

    // Combine nonce and ciphertext, then base64 encode
    let mut combined = nonce.to_vec();
    combined.extend_from_slice(&ciphertext);

    Ok(general_purpose::STANDARD.encode(&combined))
}

/// TODO(secrets): no golden vector pins backward compatibility here. The
/// aes-gcm 0.10 → 0.11 bump changed only key/nonce *generation* (the removed
/// `aead::OsRng` → crypto-common's `Generate`), leaving the cipher, the 12-byte
/// nonce, and the `nonce || ciphertext` framing untouched — so secrets sealed
/// by a pre-0.11 binary still open. The round-trip tests cannot show that:
/// they seal and open with the same build, which proves self-consistency and
/// would keep passing even if the format had silently moved. A real vector has
/// to be captured from a pre-0.11 binary and checked in as bytes. Until then,
/// treat any change to the framing above as unverifiable from the test suite.
pub fn decrypt_value(key: &[u8; 32], encrypted_value: &str) -> Result<String, OxyError> {
    let combined = general_purpose::STANDARD
        .decode(encrypted_value)
        .map_err(|e| OxyError::SecretManager(format!("Invalid encrypted value format: {e}")))?;

    if combined.len() < 12 {
        return Err(OxyError::SecretManager(
            "Invalid encrypted value: too short".to_string(),
        ));
    }

    let (nonce_bytes, ciphertext) = combined.split_at(12);
    let nonce = <&Nonce<_>>::try_from(nonce_bytes)
        .map_err(|_| OxyError::SecretManager("Invalid encrypted value: bad nonce".to_string()))?;

    let cipher = Aes256Gcm::new(&Key::<Aes256Gcm>::from(*key));
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| OxyError::SecretManager(format!("Decryption failed: {e}")))?;

    String::from_utf8(plaintext)
        .map_err(|e| OxyError::SecretManager(format!("Invalid UTF-8 in decrypted value: {e}")))
}
