//! `AppSource` — facade over the runtime source for a custom app.
//!
//! Each registered app has a `source_type` column with a `source_config`
//! payload. At request time the bundle-serve handler parses the model into an
//! [`AppSource`] and dispatches on it.
//!
//! There is one source today: [`AppSource::S3`], the build-store pipeline. The
//! store itself picks S3 or the filesystem from `OXY_CUSTOMER_APPS_S3_BUCKET`, so
//! "s3" names the pipeline, not the bucket.
//!
//! ## Removed sources (2026-09-17)
//!
//! Two more used to exist, and neither had users:
//!
//! - **`v0`** reverse-proxied an app hosted elsewhere (v0.dev, Vercel) through
//!   oxy, in a module of its own that had to strip cookies, rewrite forwarding
//!   headers and police `Set-Cookie` coming back — a security boundary kept
//!   for nobody.
//! - **`local`** served a directory on the oxy host straight off disk, which
//!   bypassed builds and channels entirely and so never exercised the real
//!   serving path (the seed already avoided it for that reason). Linking one
//!   also needed a filesystem browser in the admin API.
//!
//! A row still carrying either type is not migrated: [`AppSource::from_model`]
//! reports it as an unknown type, serving answers `500` for that one app and
//! records it, and the fleet health view shows it as down. Nothing else about
//! the registry depends on the type parsing.
//!
//! The facade stays so that adding a source again is contained: extend the enum,
//! `from_model`, `SourceSpec`, and the serve dispatch.

use entity::apps;
use serde::{Deserialize, Serialize};

/// The runtime sources oxy knows how to render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppSource {
    /// Bundle served from the build store — the published or draft build the
    /// app's channel points at.
    S3,
}

/// Wire representation of `source_config` per variant. Kept as a tagged
/// union so the API contract is explicit on both ends, and so a create request
/// naming a removed source fails to deserialize rather than being stored.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SourceSpec {
    S3,
}

impl SourceSpec {
    /// Serialise to the `(source_type, source_config)` column pair stored on
    /// the `apps` table.
    pub fn into_columns(self) -> (String, serde_json::Value) {
        match self {
            SourceSpec::S3 => ("s3".to_string(), serde_json::json!({})),
        }
    }
}

#[derive(Debug)]
pub enum ParseError {
    /// `source_type` is not one of the known variants — including the removed
    /// `v0` and `local`.
    UnknownType(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::UnknownType(t) => write!(f, "unknown source_type {t:?}"),
        }
    }
}

impl std::error::Error for ParseError {}

impl AppSource {
    /// Read `source_type` from an `apps` row and produce a typed variant.
    /// Existing rows from before the source column land with
    /// `source_type = "s3"`, so they round-trip cleanly to [`AppSource::S3`].
    pub fn from_model(app: &apps::Model) -> Result<Self, ParseError> {
        match app.source_type.as_str() {
            "s3" => Ok(AppSource::S3),
            other => Err(ParseError::UnknownType(other.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use entity::apps;
    use serde_json::json;

    fn fake_app(source_type: &str, source_config: serde_json::Value) -> apps::Model {
        apps::Model {
            visibility: "org".to_string(),
            id: uuid::Uuid::nil(),
            slug: "x".to_string(),
            name: "X".to_string(),
            org_id: uuid::Uuid::nil(),
            project_id: uuid::Uuid::nil(),
            branch: "main".to_string(),
            source_repo: "oxy-hq/customer-apps".to_string(),
            status: "created".to_string(),
            source_type: source_type.to_string(),
            source_config,
            bootstrap_pr_url: None,
            last_synced_at: None,
            manifest_override: None,
            published_at: None,
            repo_path: None,
            draft_build_id: None,
            published_build_id: None,
            last_promoted_by: None,
            last_promoted_at: None,
            created_at: chrono::Utc::now().fixed_offset(),
            updated_at: chrono::Utc::now().fixed_offset(),
        }
    }

    #[test]
    fn from_model_s3_ignores_config() {
        let app = fake_app("s3", json!({}));
        assert_eq!(AppSource::from_model(&app).unwrap(), AppSource::S3);
    }

    /// A row left over from before the removal must not be served as if it were
    /// something else. It fails to parse, which serving turns into a recorded
    /// 500 for that one app — visible in fleet health, not silent.
    #[test]
    fn a_removed_source_type_is_unknown_not_reinterpreted() {
        for removed in ["v0", "local"] {
            let app = fake_app(removed, json!({ "url": "https://x", "path": "/p" }));
            let err = AppSource::from_model(&app).unwrap_err();
            assert!(
                matches!(&err, ParseError::UnknownType(t) if t == removed),
                "{removed}: {err}"
            );
        }
    }

    #[test]
    fn from_model_unknown_type_errors() {
        let app = fake_app("vercel", json!({}));
        let err = AppSource::from_model(&app).unwrap_err();
        assert!(matches!(err, ParseError::UnknownType(t) if t == "vercel"));
    }

    /// The create API deserializes a `SourceSpec`, so this is what turns a
    /// request for a removed source into a 4xx instead of a stored row.
    #[test]
    fn a_create_request_for_a_removed_source_does_not_deserialize() {
        for body in [
            json!({ "type": "v0", "url": "https://v0.dev/x" }),
            json!({ "type": "local", "path": "/p" }),
        ] {
            assert!(
                serde_json::from_value::<SourceSpec>(body.clone()).is_err(),
                "{body}"
            );
        }
        assert_eq!(
            serde_json::from_value::<SourceSpec>(json!({ "type": "s3" })).unwrap(),
            SourceSpec::S3
        );
    }

    #[test]
    fn source_spec_into_columns() {
        let (ty, cfg) = SourceSpec::S3.into_columns();
        assert_eq!(ty, "s3");
        assert!(cfg.is_object());
    }
}
