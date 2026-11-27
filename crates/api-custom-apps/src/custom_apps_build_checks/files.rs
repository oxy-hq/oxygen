//! The files an attempt keeps as evidence: each screen's picture on staging
//! and on production, the diff between them, a changed screen's visible text
//! and the build's CSS digest. Two routes, two doors:
//!
//! | Route | Door | Does |
//! | --- | --- | --- |
//! | `PUT …/checks/files/{name}?attempt=N&environment=E` | the runner's (`admit::runner_in`): the platform checker on that environment, or non-production reach | keeps the body as the file; `204` |
//! | `GET …/checks/files/{name}?attempt=N&environment=E` | the console's (`admit::reader`): non-production reach; **never the checker**, which the console nest's layers refuse | the bytes, `Cache-Control: private, max-age=3600` |
//!
//! **Where they live.** In the build store, beside the build and never under
//! it: `customer-apps/<app_id>/checks/<build row id>/<environment>/<attempt>/<name>`
//! (`oxy_custom_apps::build_store::check_files`), on S3 or the filesystem as
//! the build store is. Never in Postgres: a check's row holds the **names**
//! (`detail.screens[].staging`, …), and a picture or a page's text can show an
//! org's data. They are deleted when the build's row is pruned (`gc_builds`)
//! and when the app is deleted (`build_store::delete_app`).
//!
//! **What a `PUT` holds a file to**, in this order:
//!
//! | Refusal | When |
//! | --- | --- |
//! | `400 invalid_environment` | the query names an environment that is not `staging` or `production` |
//! | `400 invalid_name` | the name is not `[a-z0-9._-]{1,120}`, or starts with a dot |
//! | `415 unsupported_type` | `Content-Type` is neither `image/png` nor `application/json` |
//! | `409 attempt_closed` | the attempt has settled: every row that is not a model's has a verdict |
//! | `409 too_many_files` | the attempt already keeps [`MAX_FILES`] files and this name is not one of them |
//! | `413 file_too_large` | the body is larger than [`MAX_FILE_BYTES`] |
//! | `400 invalid_file` | the body is not what its type says: no PNG signature, or not JSON |
//!
//! A file is kept under its name, so the same `PUT` again replaces it and
//! counts once. The count is read before the write, so two uploads racing at
//! the limit can both land: the bound is [`MAX_FILES`] plus what a runner
//! uploads at once (two).
//!
//! **A `GET` answers only the two types a `PUT` takes**, decided from the
//! bytes themselves, with `X-Content-Type-Options: nosniff`: nothing an
//! upload contains is ever served as a page.

use axum::body::Body;
use axum::extract::{Path, Query, Request};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use entity::{app_builds, apps};
use oxy::database::client::establish_connection;
use oxy_app_core::agent_refusal::RefuseSandboxAgent;
use oxy_custom_apps::build_store::check_files::{self, Attempt};
use sea_orm::DatabaseConnection;
use serde::Deserialize;
use uuid::Uuid;

use super::admit;
use super::environment::CheckEnvironment;
use super::error::{ChecksError, internal};
use super::requester::Requester;
use super::runner::attempt_of;
use super::store::{self, AttemptKey};
use crate::admin::apps::environment_scope::Caller;

/// The largest file one attempt keeps, in bytes.
pub const MAX_FILE_BYTES: usize = 2_000_000;
/// The most files one attempt keeps.
pub const MAX_FILES: usize = 200;

/// The eight bytes every PNG starts with.
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// Which attempt a file belongs to.
#[derive(Debug, Default, Deserialize)]
pub struct FileQuery {
    /// The attempt; the newest of the environment when absent.
    pub attempt: Option<i32>,
    /// `staging` or `production`; staging when absent.
    pub environment: Option<String>,
}

/// What a kept file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileType {
    Png,
    Json,
}

impl FileType {
    pub fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Json => "application/json",
        }
    }

    /// The type a request's `Content-Type` names (parameters ignored).
    pub fn named(content_type: Option<&str>) -> Result<Self, ChecksError> {
        let named = content_type.unwrap_or_default();
        let essence = named.split(';').next().unwrap_or_default().trim();
        [Self::Png, Self::Json]
            .into_iter()
            .find(|kind| essence.eq_ignore_ascii_case(kind.mime()))
            .ok_or_else(|| {
                ChecksError::coded(
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    "unsupported_type",
                    "a check's file is `image/png` or `application/json`: name one in Content-Type",
                )
            })
    }

    /// Whether `bytes` are what this type says.
    pub fn holds(self, bytes: &[u8]) -> bool {
        match self {
            Self::Png => bytes.starts_with(&PNG_SIGNATURE),
            Self::Json => serde_json::from_slice::<serde::de::IgnoredAny>(bytes).is_ok(),
        }
    }

    /// The type of stored `bytes`: a PNG by its signature, else JSON.
    pub fn of(bytes: &[u8]) -> Self {
        if bytes.starts_with(&PNG_SIGNATURE) {
            Self::Png
        } else {
            Self::Json
        }
    }
}

/// `name`, when it may be a kept file's.
pub fn named(name: &str) -> Result<&str, ChecksError> {
    if check_files::is_valid_name(name) {
        return Ok(name);
    }
    Err(ChecksError::bad_request(
        "invalid_name",
        format!(
            "a check's file name is 1 to {} of a-z, 0-9, `.`, `_` and `-`, and does not start \
             with a dot",
            check_files::MAX_NAME_CHARS
        ),
    ))
}

fn too_large() -> ChecksError {
    ChecksError::coded(
        StatusCode::PAYLOAD_TOO_LARGE,
        "file_too_large",
        format!("a check's file is at most {MAX_FILE_BYTES} bytes"),
    )
}

/// The request's body, refused past [`MAX_FILE_BYTES`]: by its declared
/// length before a byte is read, else as soon as it runs over.
async fn body_within_limit(headers: &HeaderMap, body: Body) -> Result<Vec<u8>, ChecksError> {
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    if declared.is_some_and(|length| length > MAX_FILE_BYTES) {
        return Err(too_large());
    }
    let bytes = axum::body::to_bytes(body, MAX_FILE_BYTES)
        .await
        .map_err(|_| too_large())?;
    Ok(bytes.to_vec())
}

fn place<'a>(app: &apps::Model, key: &'a AttemptKey) -> Attempt<'a> {
    Attempt {
        app_id: app.id,
        build_pk: key.build_pk,
        environment: &key.environment,
        attempt: key.attempt,
    }
}

/// Whether `key`'s attempt has room for a file called `name`: it exists, has
/// not settled, and keeps fewer than [`MAX_FILES`] files or already this one.
async fn admits_file(
    db: &DatabaseConnection,
    app: &apps::Model,
    key: &AttemptKey,
    name: &str,
) -> Result<(), ChecksError> {
    let rows = store::rows_of(db, key).await?;
    if rows.is_empty() {
        return Err(ChecksError::NotFound);
    }
    if store::is_settled(&rows) {
        return Err(ChecksError::conflict(
            "attempt_closed",
            format!(
                "every check of attempt {} already has a verdict: it keeps no more files",
                key.attempt
            ),
        ));
    }
    let kept = check_files::names(&place(app, key))
        .await
        .map_err(internal)?;
    if kept.len() >= MAX_FILES && !kept.iter().any(|k| k == name) {
        return Err(ChecksError::conflict(
            "too_many_files",
            format!(
                "attempt {} already keeps {MAX_FILES} files, the most one attempt may",
                key.attempt
            ),
        ));
    }
    Ok(())
}

/// The attempt a file's query names, for a runner admitted to its environment.
async fn runners_attempt(
    db: &DatabaseConnection,
    caller: &Caller<'_>,
    (app, build): (&apps::Model, &app_builds::Model),
    q: &FileQuery,
) -> Result<AttemptKey, ChecksError> {
    let environment = CheckEnvironment::named(q.environment.as_deref())?;
    admit::runner_in(db, caller, app, environment).await?;
    attempt_of(db, build, environment, q.attempt).await
}

/// `PUT /api/customer-apps/{id}/builds/{build}/checks/files/{name}`.
pub async fn put(
    _: RefuseSandboxAgent,
    who: Requester,
    Path((id, build, name)): Path<(Uuid, String, String)>,
    Query(q): Query<FileQuery>,
    request: Request,
) -> Result<StatusCode, ChecksError> {
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    let db = establish_connection().await.map_err(internal)?;
    let caller = who.caller();
    let (app, build) = admit::runner(&db, &caller, id, &build).await?;
    let key = runners_attempt(&db, &caller, (&app, &build), &q).await?;
    let name = named(&name)?;
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok());
    let kind = FileType::named(content_type)?;
    admits_file(&db, &app, &key, name).await?;
    let bytes = body_within_limit(&headers, body).await?;
    if !kind.holds(&bytes) {
        return Err(ChecksError::bad_request(
            "invalid_file",
            format!("the body is not {}", kind.mime()),
        ));
    }
    let size = bytes.len();
    check_files::put(&place(&app, &key), name, bytes)
        .await
        .map_err(internal)?;
    tracing::info!(app_id = %app.id, build = %build.build_id, attempt = key.attempt,
        environment = %key.environment, name, size, "build checks: file kept");
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/customer-apps/{id}/builds/{build}/checks/files/{name}`: the
/// console's read of one kept file.
pub async fn get(
    _: RefuseSandboxAgent,
    who: Requester,
    Path((id, build, name)): Path<(Uuid, String, String)>,
    Query(q): Query<FileQuery>,
) -> Result<Response, ChecksError> {
    let db = establish_connection().await.map_err(internal)?;
    let caller = who.caller();
    let (app, build) = admit::reader(&db, &caller, id, &build).await?;
    let environment = CheckEnvironment::named(q.environment.as_deref())?;
    let name = named(&name)?;
    let key = attempt_of(&db, &build, environment, q.attempt).await?;
    let bytes = check_files::get(&place(&app, &key), name)
        .await
        .map_err(internal)?
        .ok_or(ChecksError::NotFound)?;
    let kind = FileType::of(&bytes);
    let headers = [
        (header::CONTENT_TYPE, HeaderValue::from_static(kind.mime())),
        (
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, max-age=3600"),
        ),
        (
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ),
    ];
    Ok((headers, bytes).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_a_png_or_json_by_its_content_type_and_by_its_bytes() {
        assert_eq!(FileType::named(Some("image/png")).unwrap(), FileType::Png);
        assert_eq!(
            FileType::named(Some("Application/JSON; charset=utf-8")).unwrap(),
            FileType::Json
        );
        for refused in [
            None,
            Some(""),
            Some("text/html"),
            Some("image/svg+xml"),
            Some("image/jpeg"),
            Some("application/octet-stream"),
        ] {
            let e = FileType::named(refused).unwrap_err();
            assert_eq!(e.code(), Some("unsupported_type"), "{refused:?}");
        }
        let mut png = PNG_SIGNATURE.to_vec();
        png.extend_from_slice(b"\0\0\0\rIHDR");
        assert!(FileType::Png.holds(&png));
        assert!(!FileType::Png.holds(b"<html><script>alert(1)</script>"));
        assert!(!FileType::Png.holds(b""));
        assert!(FileType::Json.holds(br#"{"text":"Acme payroll"}"#));
        assert!(!FileType::Json.holds(b"<html>"));
        assert!(!FileType::Json.holds(&png));
        // What is stored is answered as one of the two, never anything else.
        assert_eq!(FileType::of(&png), FileType::Png);
        assert_eq!(FileType::of(br#"{"rules":[]}"#), FileType::Json);
        assert_eq!(FileType::of(b"<html>").mime(), "application/json");
    }

    #[test]
    fn a_name_outside_the_rule_is_refused_saying_the_rule() {
        assert_eq!(
            named("shot-staging-root-desk.png").unwrap(),
            "shot-staging-root-desk.png"
        );
        for bad in ["", "..", ".env", "a/b.png", "Shot.png", &"a".repeat(121)] {
            let e = named(bad).unwrap_err();
            assert_eq!(e.code(), Some("invalid_name"), "{bad:?}");
        }
    }

    #[tokio::test]
    async fn a_body_past_the_limit_is_refused_by_its_length_or_as_it_runs_over() {
        let mut declared = HeaderMap::new();
        declared.insert(header::CONTENT_LENGTH, HeaderValue::from_static("2000001"));
        let refused = body_within_limit(&declared, Body::empty())
            .await
            .unwrap_err();
        assert_eq!(refused.code(), Some("file_too_large"));
        // No declared length: the bytes themselves are counted.
        let over = Body::from(vec![0u8; MAX_FILE_BYTES + 1]);
        let refused = body_within_limit(&HeaderMap::new(), over)
            .await
            .unwrap_err();
        assert_eq!(refused.code(), Some("file_too_large"));
        let exactly = Body::from(vec![0u8; MAX_FILE_BYTES]);
        let kept = body_within_limit(&HeaderMap::new(), exactly).await.unwrap();
        assert_eq!(kept.len(), MAX_FILE_BYTES);
    }
}
