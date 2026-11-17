//! The multipart body of `POST /api/customer-apps/publish`, parsed into a
//! [`PublishForm`] before anything is decided about it.
//!
//! Field names are the wire contract `oxyc publish` (and older clients) send;
//! none is renamed here. What each one means — `org` accepts a slug or a UUID,
//! `org_id` is the explicit UUID alias that wins when both are sent, `channel`
//! and `promote` are two spellings of one flag, `project` and `project_id` are
//! one field, `confirm_schedule_removals` is sent once per function — is the
//! handler's business and is decided here, once.

use axum::extract::Multipart;
use axum::http::StatusCode;
use uuid::Uuid;

use super::custom_apps_publish::{OrgRef, PublishInput, Publisher};
use super::custom_apps_publish_refusal::PublishRefusal;

/// The request, parsed and validated, with nothing resolved yet.
pub struct PublishForm {
    pub org_ref: Option<OrgRef>,
    pub app_slug: String,
    pub project_id: Uuid,
    pub branch: Option<String>,
    /// Minted when the client sent none.
    pub build_id: String,
    pub name: Option<String>,
    pub promote: bool,
    pub tarball: Vec<u8>,
    pub manifest: Option<serde_json::Value>,
    pub source_repo: Option<String>,
    pub commit_sha: Option<String>,
    pub semantic_revision_id: Option<Uuid>,
    /// A sandbox to publish to (`dev-<handle>`); absent or empty is the
    /// publish this route always made.
    pub environment: Option<String>,
    /// A promoting publish's confirmed schedule retirements, by function.
    pub confirm_schedule_removals: Vec<String>,
}

/// Every field as it arrived, before validation. A `_invalid` holds a value
/// that was sent and did not parse: surfaced as a `400`, never silently
/// dropped, so a typo'd UUID cannot land the publish somewhere else.
#[derive(Default)]
struct RawFields {
    org: Option<String>,
    org_id: Option<Uuid>,
    org_id_invalid: Option<String>,
    app: Option<String>,
    project_id: Option<Uuid>,
    branch: Option<String>,
    build_id: Option<String>,
    name: Option<String>,
    promote: bool,
    manifest: Option<serde_json::Value>,
    tarball: Option<Vec<u8>>,
    source_repo: Option<String>,
    commit_sha: Option<String>,
    semantic_revision_id: Option<Uuid>,
    semantic_revision_invalid: Option<String>,
    environment: Option<String>,
    confirm_schedule_removals: Vec<String>,
}

fn bad_request(message: String) -> PublishRefusal {
    PublishRefusal::text(StatusCode::BAD_REQUEST, message)
}

fn non_empty(val: String) -> Option<String> {
    Some(val).filter(|s| !s.is_empty())
}

impl RawFields {
    /// One text field. A read error is propagated rather than coerced to ""
    /// — an empty `promote` would degrade a `--promote` publish to a draft
    /// with no signal.
    fn take_text(&mut self, key: &str, val: String) {
        match key {
            "org" => self.org = Some(val),
            "org_id" => self.take_org_id(val),
            "app" => self.app = Some(val),
            "project" | "project_id" => self.project_id = Uuid::parse_str(val.trim()).ok(),
            "branch" => self.branch = non_empty(val),
            "build_id" => self.build_id = non_empty(val),
            "name" => self.name = non_empty(val),
            "source_repo" => self.source_repo = non_empty(val),
            "commit_sha" => self.commit_sha = non_empty(val),
            "environment" => self.environment = Some(val),
            // Repeated, one function each, as `oxyc promote` sends the list.
            "confirm_schedule_removals" => self.confirm_schedule_removals.extend(non_empty(val)),
            "channel" => self.promote = val == "published",
            "promote" => self.promote = val == "true" || val == "1",
            // Staging pin (`--semantic-branch`). Not `branch`, which is the
            // app's SOURCE branch.
            "semantic_revision_id" => self.take_semantic_revision(val),
            _ => {}
        }
    }

    /// Empty is "field omitted", like every other field here; anything else
    /// must be a UUID.
    fn take_org_id(&mut self, val: String) {
        let trimmed = val.trim();
        if trimmed.is_empty() {
            return;
        }
        match Uuid::parse_str(trimmed) {
            Ok(id) => self.org_id = Some(id),
            Err(_) => self.org_id_invalid = Some(trimmed.to_string()),
        }
    }

    fn take_semantic_revision(&mut self, val: String) {
        match val.trim() {
            "" => {}
            v => match Uuid::parse_str(v) {
                Ok(id) => self.semantic_revision_id = Some(id),
                Err(_) => self.semantic_revision_invalid = Some(v.to_string()),
            },
        }
    }

    /// A present-but-unparseable manifest is a hard 400, not a silent drop.
    /// Degrading to `None` strips the bundle's whole `functions` block and
    /// publishes a function-less app with a 200 — then `useFunction` 404s and
    /// the author is misdiagnosed as never having declared the function.
    fn take_manifest(&mut self, raw: String) -> Result<(), PublishRefusal> {
        if raw.trim().is_empty() {
            return Ok(());
        }
        self.manifest = Some(
            serde_json::from_str(&raw)
                .map_err(|e| bad_request(format!("manifest field is not valid JSON: {e}")))?,
        );
        Ok(())
    }

    async fn read(mut multipart: Multipart) -> Result<Self, PublishRefusal> {
        let mut raw = Self::default();
        while let Some(field) = multipart
            .next_field()
            .await
            .map_err(|e| bad_request(format!("multipart error: {e}")))?
        {
            let key = field.name().unwrap_or_default().to_string();
            match key.as_str() {
                "bundle" => {
                    let bytes = field
                        .bytes()
                        .await
                        .map_err(|e| bad_request(format!("bundle read: {e}")))?;
                    raw.tarball = Some(bytes.to_vec());
                }
                "manifest" => {
                    let text = field
                        .text()
                        .await
                        .map_err(|e| bad_request(format!("manifest read: {e}")))?;
                    raw.take_manifest(text)?;
                }
                _ => {
                    let text = field
                        .text()
                        .await
                        .map_err(|e| bad_request(format!("field '{key}' read: {e}")))?;
                    raw.take_text(&key, text);
                }
            }
        }
        Ok(raw)
    }
}

impl PublishForm {
    /// Read and validate the body. Every refusal is a `400` naming the field,
    /// as the route has always answered.
    pub async fn from_multipart(multipart: Multipart) -> Result<Self, PublishRefusal> {
        let raw = RawFields::read(multipart).await?;
        // Reject a bundle-supplied display name with control/JSON-breaking
        // chars before it lands in the apps row (defense against injection via
        // the uploaded manifest / multipart field).
        if let Some(n) = &raw.name {
            crate::admin::apps::handlers::validate_display_name(n).map_err(bad_request)?;
        }
        if let Some(bad) = raw.semantic_revision_invalid {
            return Err(bad_request(format!(
                "semantic_revision_id is not a valid UUID: {bad:?}"
            )));
        }
        if let Some(bad) = raw.org_id_invalid {
            return Err(bad_request(format!("org_id is not a valid UUID: {bad:?}")));
        }
        refuse_confirmations_without_promote(&raw)?;
        // No org sent — `publish` infers it from the pinned workspace (a
        // workspace belongs to exactly one org), which is why the workspace
        // is required and the refusal names the flag that sets it.
        let org_ref = match (raw.org_id, raw.org) {
            (Some(id), _) => Some(OrgRef::Id(id)),
            (None, Some(s)) => Some(OrgRef::from_str_auto(&s)),
            (None, None) => None,
        };
        Ok(Self {
            org_ref,
            app_slug: raw.app.ok_or_else(|| bad_request("missing app".into()))?,
            project_id: raw.project_id.ok_or_else(|| {
                bad_request(
                    "missing/invalid project: a publish must name the workspace the app \
                     belongs to — pass `--project <workspace-uuid>` to `oxyc publish` (or \
                     the `project` multipart field)"
                        .into(),
                )
            })?,
            branch: raw.branch,
            build_id: raw
                .build_id
                .unwrap_or_else(|| Uuid::new_v4().simple().to_string()),
            name: raw.name,
            promote: raw.promote,
            tarball: raw
                .tarball
                .ok_or_else(|| bad_request("missing bundle".into()))?,
            manifest: raw.manifest,
            source_repo: raw.source_repo,
            commit_sha: raw.commit_sha,
            semantic_revision_id: raw.semantic_revision_id,
            environment: raw.environment,
            confirm_schedule_removals: raw.confirm_schedule_removals,
        })
    }

    /// The form, as the publish takes it, recorded as `publisher`.
    pub fn into_input(self, publisher: Publisher) -> PublishInput {
        PublishInput {
            org_ref: self.org_ref,
            app_slug: self.app_slug,
            project_id: self.project_id,
            branch: self.branch,
            build_id: self.build_id,
            name: self.name,
            promote: self.promote,
            tarball: self.tarball,
            manifest: self.manifest,
            source_repo: self.source_repo,
            commit_sha: self.commit_sha,
            published_by: publisher.published_by,
            published_by_email: publisher.published_by_email,
            publisher: publisher.caller,
            machine_app_id: publisher.machine_app_id,
            published_via: publisher.published_via,
            semantic_revision_id: self.semantic_revision_id,
            confirm_schedule_removals: self.confirm_schedule_removals,
        }
    }
}

/// A confirmed retirement belongs to a promoting publish: a draft retires no
/// production schedule, so a confirmation sent with one is a `400`, not
/// silently ignored — it says the publisher meant to promote.
fn refuse_confirmations_without_promote(raw: &RawFields) -> Result<(), PublishRefusal> {
    if raw.promote || raw.confirm_schedule_removals.is_empty() {
        return Ok(());
    }
    Err(bad_request(format!(
        "confirm_schedule_removals ({}) confirms retiring a production schedule, which only a \
         promoting publish does: add --promote (channel=published), or drop --remove-schedule",
        raw.confirm_schedule_removals.join(", ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pure field rules, without a multipart body: both spellings of the
    /// promote flag, the empty-means-omitted rule, and the invalid-UUID
    /// capture that becomes a 400.
    #[test]
    fn text_fields_follow_the_routes_rules() {
        let mut raw = RawFields::default();
        raw.take_text("channel", "published".into());
        assert!(raw.promote);
        raw.take_text("promote", "0".into());
        assert!(!raw.promote);
        raw.take_text("promote", "1".into());
        assert!(raw.promote);
        raw.take_text("branch", "".into());
        assert_eq!(raw.branch, None);
        raw.take_text("org_id", "  ".into());
        assert_eq!((raw.org_id, raw.org_id_invalid.as_deref()), (None, None));
        raw.take_text("org_id", "not-a-uuid".into());
        assert_eq!(raw.org_id_invalid.as_deref(), Some("not-a-uuid"));
        raw.take_text("semantic_revision_id", "nope".into());
        assert_eq!(raw.semantic_revision_invalid.as_deref(), Some("nope"));
        let id = Uuid::new_v4();
        raw.take_text("project", id.to_string());
        assert_eq!(raw.project_id, Some(id));
    }

    /// One function per field, as `oxyc publish --remove-schedule` repeats
    /// it; an empty one is omitted like every other field.
    #[test]
    fn schedule_confirmations_collect_one_function_per_field() {
        let mut raw = RawFields::default();
        raw.take_text("confirm_schedule_removals", "nightly".into());
        raw.take_text("confirm_schedule_removals", "".into());
        raw.take_text("confirm_schedule_removals", "hourly".into());
        assert_eq!(raw.confirm_schedule_removals, vec!["nightly", "hourly"]);
    }

    /// A confirmation needs a promote to confirm anything.
    #[test]
    fn a_confirmation_without_promote_is_refused() {
        let mut raw = RawFields::default();
        assert!(refuse_confirmations_without_promote(&raw).is_ok());
        raw.take_text("confirm_schedule_removals", "nightly".into());
        let refused = refuse_confirmations_without_promote(&raw).expect_err("a draft");
        let text = format!("{refused:?}");
        assert!(text.contains("400") && text.contains("nightly"), "{text}");
        raw.take_text("channel", "published".into());
        assert!(refuse_confirmations_without_promote(&raw).is_ok());
    }

    #[test]
    fn an_unparseable_manifest_is_refused_and_an_empty_one_is_omitted() {
        let mut raw = RawFields::default();
        assert!(raw.take_manifest("  ".into()).is_ok());
        assert_eq!(raw.manifest, None);
        assert!(raw.take_manifest("{not json".into()).is_err());
        assert!(raw.take_manifest(r#"{"functions":{}}"#.into()).is_ok());
        assert!(raw.manifest.is_some());
    }
}
