//! `POST /api/customer-apps/publish` — the one-way publish entry point.
//!
//! CI (or a local `oxyc publish`) uploads a gzipped tar of the built
//! bundle. The service validates it, stores each file in S3 under a
//! per-build prefix ([`super::custom_apps_build_store`]), records an
//! `app_builds` row, upserts the `apps` row (creating it on first
//! publish), and points the draft channel (and published, with
//! `--promote`) at the new build. Replaces the old
//! `ensure` + `aws s3 sync` + callback-`/sync` dance.
//!
//! Gating: this route is deliberately **not** app-admin-gated — partners and CI
//! publish through it — so [`authorize_publish`] IS the whole decision (never
//! assume the caller is trusted staff). It resolves the caller to a
//! [`custom_apps_publish_authz::PublishActor`] and defers to the pure, tested
//! `publish_decision`: staff may publish unless the workspace locked Oxy out
//! (`workspace_oxy_lockdown`); an org **Admin+** may publish their own app; a
//! **partner** needs all three gates (assigned + `manage_apps` + client consent);
//! a plain Member and an Outsider are **denied**. We also validate that the target
//! project belongs to the named org to catch fat-finger cross-org publishes.

use std::io::Read;

use oxy_custom_apps::manifest::is_safe_relative_path;

use axum::Json;
use axum::extract::Multipart;
use axum::http::StatusCode;
use chrono::Utc;
use entity::{app_builds, app_functions, apps, organizations, workspaces};
use flate2::read::GzDecoder;
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, ModelTrait,
    QueryFilter, QueryOrder, TransactionTrait,
};
use serde::Serialize;
use tar::Archive;
use uuid::Uuid;

use super::custom_apps_build_state::{BuildState, PromoteRefusal};
use super::custom_apps_env_schedules::reconcile as env_schedules;
use super::custom_apps_nonproduction::publish as staging;
use super::custom_apps_nonproduction::staging_task;
use super::custom_apps_publish_form::PublishForm;
use super::custom_apps_publish_refusal::PublishRefusal;
use super::custom_apps_sandboxes::{
    agent_draft as sandbox_agent_draft, agent_publish as sandbox_agent_publish,
    publish as sandbox_publish, retention,
};
use super::{
    custom_apps_asset_manifest as asset_manifest, custom_apps_auth,
    custom_apps_build_store as store, custom_apps_bundle_cache as cache,
    custom_apps_migrations as migrations, custom_apps_precompress as precompress,
};
use oxy_app_core::workspace_org::{WorkspaceOrgMatch, workspace_in_org};

mod admit;
mod inflate;
mod orphan;
mod pipeline;

/// How many builds to retain per app **in each retention window**
/// (`custom_apps_sandboxes::retention`). Older builds (not currently pointed
/// at by either channel) are GC'd from the DB and S3 after each publish.
const KEEP_BUILDS: usize = 10;

/// The warning a promoting publish (`oxyc publish --promote`; `promote=true|1`
/// or `channel=published` from any CLI) answers with. It still works — staging
/// moves first, then production through the same server-side path `oxyc
/// promote` takes — but it shows nobody what production is about to change.
pub const PROMOTING_PUBLISH_DEPRECATED: &str = "`oxyc publish --promote` (a publish \
    sending `promote` or `channel=published`) is deprecated: publish to staging, then run \
    `oxyc promote <org>/<app>`, which shows what production will change — migrations, schedules, \
    unset required secrets — before it moves. This publish was promoted through the same path";

pub struct PublishInput {
    /// Org identity — accepts either a slug (`"acme"`) or a UUID
    /// (`"550e8400-e29b-41d4-a716-446655440000"`). UUIDs are useful
    /// when the slug has drifted between envs (e.g. an admin renamed
    /// the org in prod but not staging) and the publisher wants a
    /// stable handle. `resolve_org` looks at both columns. `None` means the
    /// publisher pinned only a workspace (`--project`) — the org is inferred
    /// from it, since a workspace belongs to exactly one org.
    pub org_ref: Option<OrgRef>,
    pub app_slug: String,
    pub project_id: Uuid,
    pub branch: Option<String>,
    pub build_id: String,
    pub name: Option<String>,
    pub promote: bool,
    pub tarball: Vec<u8>,
    pub manifest: Option<serde_json::Value>,
    /// Git remote URL of the app source at publish time (best-effort).
    pub source_repo: Option<String>,
    /// Commit sha the build was published from (best-effort).
    pub commit_sha: Option<String>,
    /// Authenticated publisher (app-admin). Recorded on the build for the
    /// "who deployed" audit in the admin UI. `None` for a machine publish: see
    /// [`Publisher::from_request`].
    pub published_by: Option<Uuid>,
    /// Email of the publisher — needed to resolve partner / staff authority for
    /// the third-party publish path (a partner uploading into a client).
    pub published_by_email: Option<String>,
    /// The publisher with the credential the request arrived with — what publish
    /// authority is resolved for, so an API token publishes only where its
    /// grants reach and with the standing it carries. `None` for a machine
    /// publish, which is authorized by its app id and consent alone.
    pub publisher: Option<oxy_server_authz::Caller>,
    /// Set iff the request authenticated via an **app-scoped** publish token —
    /// OIDC-minted (no human) or partner-minted (a real `created_by`, design §7).
    /// Either way the `app_id` confines the token: authorization is strictly
    /// "this token's app == the target app AND the client consents", and it can
    /// publish to that one app and nowhere else — not the user's broader gates.
    pub machine_app_id: Option<Uuid>,
    /// The OIDC-verified workflow identity of a trusted-publishing (machine)
    /// publish, recorded as `app_builds.published_via`. `None` for a user.
    pub published_via: Option<String>,
    /// Compiled semantic revision this build's STAGING requests read
    /// (`oxyc publish --semantic-branch`). Draft-only: refused with
    /// `promote`. Unrelated to [`Self::branch`], which is the APP SOURCE
    /// branch. See `custom_apps_staging_pin`.
    pub semantic_revision_id: Option<Uuid>,
}

/// Who a publish request is recorded as — the publisher fields of [`PublishInput`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publisher {
    pub published_by: Option<Uuid>,
    pub published_by_email: Option<String>,
    /// See [`PublishInput::publisher`].
    pub caller: Option<oxy_server_authz::Caller>,
    pub machine_app_id: Option<Uuid>,
    pub published_via: Option<String>,
}

impl Publisher {
    /// Resolve the request's authenticated identity into what the build records.
    ///
    /// An OIDC-minted machine token authenticates as
    /// [`AuthenticatedUser::machine_publisher`](oxy_auth::types::AuthenticatedUser::machine_publisher),
    /// whose nil id has no `users` row. Every "who" column a publish writes
    /// (`app_builds.published_by`, `app_environments.updated_by`,
    /// `app_environment_events.actor`) references `users(id)`, so a machine
    /// publish records **no user** and the verified workflow identity in
    /// `published_via` instead. Stamping the nil id 500'd every trusted publish.
    ///
    /// Authorization is unaffected: the machine branch of `authorize_publish`
    /// decides by `machine_app_id` + consent and returns before the human path
    /// that requires `published_by` and an email.
    pub fn from_request(
        user: &oxy_auth::types::AuthenticatedUser,
        marker: Option<&oxy_auth::types::AppPublishTokenAuth>,
    ) -> Self {
        let machine_app_id = marker.and_then(|m| m.app_id);
        if let Some(identity) = marker.and_then(|m| m.machine_identity.clone()) {
            return Self {
                published_by: None,
                published_by_email: None,
                caller: None,
                machine_app_id,
                published_via: Some(identity),
            };
        }
        // A trusted-access (`ci`) token: the publisher is its service account,
        // which has a `users` row, so the build names it — and carries the
        // workflow identity the exchange verified, as a machine publish does.
        let published_via = user
            .credential
            .as_ref()
            .filter(|c| c.kind == oxy_auth::token::StoredKind::Ci)
            .map(|c| c.name.clone());
        Self {
            published_by: Some(user.id),
            // `user.email`, never a display label. This value is not decoration:
            // `publish_authz::resolve_actor` feeds it to `platform_reaches`, so a
            // label that fell back to the non-unique `name` would let a user named
            // after a staff address publish with STAFF authority into any org that
            // grant reaches. `publish` already denies a `None` here, which is the
            // right answer — publishing is a developer action and a worker
            // enrolled without a mailbox has no path to it.
            published_by_email: user.email.clone(),
            caller: Some(oxy_server_authz::Caller::from_user(user)),
            machine_app_id,
            published_via,
        }
    }
}

/// How the publisher referred to the target org. Accepting both lets
/// CLI users pass whichever they have at hand: `--org acme` (slug,
/// stable for humans) or `--org 550e8400-...` (UUID, stable across
/// rename/env-drift). The server tries them in order.
#[derive(Debug)]
pub enum OrgRef {
    /// Looked up against `organizations.slug`.
    Slug(String),
    /// Looked up against `organizations.id`. Skips a slug round-trip
    /// when the publisher already knows the row id.
    Id(Uuid),
}

impl OrgRef {
    /// Auto-detect: if the input parses as a UUID, treat as `Id`;
    /// otherwise treat as `Slug`. Lets a single `--org <value>` CLI
    /// arg accept either form without forcing the user to pick a
    /// different flag.
    pub fn from_str_auto(s: &str) -> Self {
        match Uuid::parse_str(s) {
            Ok(id) => OrgRef::Id(id),
            Err(_) => OrgRef::Slug(s.to_string()),
        }
    }

    fn describe(&self) -> String {
        match self {
            OrgRef::Slug(s) => s.clone(),
            OrgRef::Id(id) => id.to_string(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct PublishResult {
    pub app_id: Uuid,
    pub build_id: String,
    pub url: String,
    /// `draft`, `published`, or `sandbox` when the publish named one.
    pub channel: String,
    /// The environment this publish moved: `staging`, `production` (with
    /// `promote`), or the sandbox it named.
    pub environment: String,
    /// That environment's own host; `null` when it has none (no custom-apps
    /// zone on this deployment, or a host label over 63 bytes).
    pub environment_url: Option<String>,
    /// Canonical org slug the app row landed on, after the server
    /// resolved whatever `OrgRef` the publisher sent. Echoing the
    /// server's view lets the CLI render `Registered new app
    /// acme/store-pulse` even when the engineer passed a UUID (which
    /// would otherwise echo back as `550e8400-…/store-pulse` —
    /// technically correct but jarring).
    pub org_slug: String,
    /// `true` when this publish created the app row, `false` when it
    /// updated an existing row. Lets the CLI tell the engineer whether
    /// they just registered a brand-new app or shipped a new version
    /// of one that was already in the system — surfaces accidental
    /// re-registration vs. intentional re-publish.
    pub is_new_app: bool,
    /// Non-fatal problems the operator should see even though the publish
    /// succeeded — e.g. a function shipped but its cron schedule failed to
    /// register, so it silently won't fire. Omitted from the JSON when empty, so
    /// this is an additive wire change: existing clients that don't read it are
    /// unaffected.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    #[error("organization {0:?} not found")]
    UnknownOrg(String),
    #[error("project {0} is not part of org {1:?}")]
    UnknownProject(Uuid, String),
    #[error(
        "org {org:?} has not granted Oxy access for workspace {project} — an org owner must enable it (workspace settings → Oxy access)"
    )]
    OxyAccessDenied { org: String, project: Uuid },
    #[error("invalid bundle: {0}")]
    BadTarball(String),
    /// A build with this `build_id` already exists for the app. Surfaced as 409
    /// — see the check in [`publish`] for why this cannot be allowed to
    /// overwrite.
    #[error(
        "build {build_id:?} already exists for app {app_slug:?} — a build id must be unique per publish because its stored bytes are immutable and cached by id. Pass a different --build-id."
    )]
    DuplicateBuild { app_slug: String, build_id: String },
    /// The app already exists in a different workspace than this publish
    /// names. Surfaced as 409 — see `ensure_same_workspace` for why a
    /// publish, draft or not, never moves an app.
    #[error(
        "app {app_slug:?} belongs to workspace {existing_project}, but this publish targets workspace {requested_project}. A publish never moves an app to another workspace: publish with --project {existing_project}. To move the app, an app admin changes its workspace first (PATCH /api/customer-apps/{app_id} with {{\"project_id\": \"{requested_project}\"}}, e.g. `oxyc api -X PATCH /api/customer-apps/{app_id} -f project_id={requested_project}`), then publishes again."
    )]
    ProjectMismatch {
        app_slug: String,
        app_id: Uuid,
        existing_project: Uuid,
        requested_project: Uuid,
    },
    /// The `build_id` cannot safely become a store key / path segment.
    /// Surfaced as 422 — see `custom_apps_build_store::is_valid_build_id`.
    #[error(
        "invalid build id {0:?} — use only letters, digits, `.`, `_` and `-` (no leading dot, max 200 chars)"
    )]
    InvalidBuildId(String),
    /// The `app_slug` cannot safely become a schema/role name, a `repo_path`
    /// segment, or the served `/customer-apps/{org}/{slug}/` base path. Surfaced
    /// as 422 — see `admin::apps::is_valid_slug`. Crucially, an `_` in a slug
    /// would alias a hyphenated sibling onto one OLTP writer (see
    /// `oxy_oltp::schema::app_writer_name`), which `is_valid_slug` forbids.
    #[error(
        "invalid app slug {0:?} — use 1–63 lowercase letters, digits and single hyphens (no leading/trailing/double hyphen, no underscore)"
    )]
    InvalidSlug(String),
    /// The manifest declares functions whose bundled JS the bundle does not
    /// carry. Surfaced as 422 — see `missing_function_artifacts`.
    #[error(
        "oxy-app.json declares {} function(s) with no bundled artifact: {}. Each declared function must ship in the bundle as functions/<name>.js; `oxy publish` bundles them from oxy CLI 0.5.96. Upgrade the oxy CLI (>= 0.5.96), run `oxy publish` from the app directory that holds oxy-app.json, and publish again.",
        .missing.len(),
        .missing.join(", ")
    )]
    MissingFunctionArtifacts { missing: Vec<String> },
    /// The manifest marks `shared` a key that must be set per environment: one
    /// a function writes, or a `webhook.secretVar`. Surfaced as 422 — see
    /// `custom_apps_secrets::shared_env::check_shared_env`.
    #[error(
        "oxy-app.json marks env keys \"shared\" that each environment must hold its own value of: {}. Remove `\"shared\": true` from them and set a staging value instead.",
        .conflicts.join("; ")
    )]
    SharedEnvConflict { conflicts: Vec<String> },
    /// A fast bundle-validation check failed (design doc §8, gate 1). Carries an
    /// actionable check/message/remediation; surfaced as 422.
    #[error("{0}")]
    Invalid(crate::custom_apps_validate::BundleValidation),
    /// A declared schema migration could not be shipped. Status is delegated to
    /// the error itself (`is_author_fault` / `is_retryable`): an edited
    /// migration is a 422 the publisher must fix, a contended apply is a 409
    /// worth retrying, and our own database trouble is a 500.
    #[error("{0}")]
    Migration(migrations::MigrationError),
    /// The requested `semantic_revision_id` cannot be pinned. `WithPromote`
    /// is a 400 (the request is contradictory); the rest are 422.
    #[error("{0}")]
    SemanticPin(crate::custom_apps_staging_pin::PinRefusal),
    /// `nonProduction.destinations` cannot ship: a 422 the author fixes, or a
    /// 409 to retry once the workspace has a compiled config to check it by.
    #[error("{0}")]
    DestinationMapping(crate::custom_apps_nonproduction::MappingRefusal),
    /// The `environment` field names something that is not a sandbox.
    /// Production and staging are reached by leaving the field out.
    #[error(
        "environment {0:?} is not a sandbox name: use dev-<handle>, where the handle is 1 to 12 characters of a-z, 0-9 and single hyphens. Leave the field out to publish to staging."
    )]
    InvalidEnvironment(String),
    /// A sandbox publish moves one sandbox's pointer; it cannot also promote.
    #[error(
        "a publish to a sandbox cannot promote: drop --promote (or channel=published), or drop the environment to publish to staging and production"
    )]
    SandboxWithPromote,
    /// The publisher may not use the app's sandboxes: not staff with reach
    /// over its org, or authenticated by a publish token.
    #[error(
        "publishing to a sandbox is for Oxy staff who may open this app's non-production environments, signed in with a login token or an API key — a publish token cannot"
    )]
    SandboxRefused,
    /// A sandbox agent token publishing to the app's channels, or promoting:
    /// it publishes to a sandbox it created and nowhere else. The one publish
    /// refusal answered as JSON with a `code` (`custom_apps_publish_refusal`).
    #[error(
        "a sandbox agent token publishes only to a sandbox it created: name it with the environment field (`oxyc publish --env dev-<handle>`), without --promote. Staging and production are published with a login token"
    )]
    SandboxTokenRefused,
    /// No such sandbox — or no such app: a sandbox publish never creates one.
    #[error(
        "no sandbox {name} to publish to: the app does not exist, or it has no sandbox of that name. Create it first (`oxyc env create <app> {name}`)."
    )]
    UnknownEnvironment { name: String },
    /// The sandbox is being torn down.
    #[error("sandbox {name} is being deleted; its name is free once the teardown finishes")]
    EnvironmentDeleting { name: String },
    /// A sandbox agent token's draft to staging, refused by one of its own
    /// guards. Status and code are the refusal's
    /// (`custom_apps_sandboxes::agent_draft`); answered as JSON with the code.
    #[error("{0}")]
    AgentDraft(sandbox_agent_draft::AgentDraftRefusal),
    #[error("database error: {0}")]
    Db(String),
    #[error("storage error: {0}")]
    S3(String),
}

/// Where a publish moves a pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishTarget {
    /// Today's publish: staging's pointer, and production's with `promote`.
    Channels,
    /// One sandbox's pointer and nothing else. Always an `AppEnvironment::Dev`.
    Sandbox(AppEnvironment),
    /// Staging's pointer, for a sandbox agent token granted the app's
    /// staging: a draft of an app that is already live, which never writes
    /// the app row and never promotes (`custom_apps_sandboxes::agent_draft`).
    /// Made by `sandbox_publish::target_of` alone.
    AgentDraft,
}

impl PublishTarget {
    /// The environment a publish to this target moves, and the `channel` the
    /// response reports for it.
    fn moved(&self, promote: bool) -> (AppEnvironment, &'static str) {
        match self {
            PublishTarget::Sandbox(environment) => (environment.clone(), "sandbox"),
            PublishTarget::Channels if promote => (AppEnvironment::Production, "published"),
            PublishTarget::Channels | PublishTarget::AgentDraft => {
                (AppEnvironment::Staging, "draft")
            }
        }
    }

    /// What `app_builds.published_to` records: the environment the publish
    /// moves first — staging for the channels, with or without `promote`,
    /// and for an agent's draft.
    fn published_to(&self) -> AppEnvironment {
        match self {
            PublishTarget::Sandbox(environment) => environment.clone(),
            PublishTarget::Channels | PublishTarget::AgentDraft => AppEnvironment::Staging,
        }
    }

    /// A sandbox publish moves one sandbox's pointer; it cannot also promote.
    fn refuse_promote(&self, input: &PublishInput) -> Result<(), PublishError> {
        match self {
            PublishTarget::Sandbox(_) if input.promote => Err(PublishError::SandboxWithPromote),
            // A sandbox agent token's draft never promotes: refused again
            // here, where every promoting step below reads `input.promote`.
            PublishTarget::AgentDraft if input.promote => Err(PublishError::SandboxTokenRefused),
            _ => Ok(()),
        }
    }

    /// What a finished publish to this target answers: the build, the channel
    /// and environment it moved, and that environment's own host.
    fn result(
        &self,
        org_slug: &str,
        input: PublishInput,
        (app_id, is_new_app): (Uuid, bool),
        warnings: Vec<String>,
    ) -> PublishResult {
        let (environment, channel) = self.moved(input.promote);
        PublishResult {
            app_id,
            url: format!("/customer-apps/{org_slug}/{}/", input.app_slug),
            channel: channel.to_string(),
            environment_url: oxy_app_core::custom_apps_host_dispatch::environment_url_for(
                &environment,
                org_slug,
                &input.app_slug,
            ),
            environment: environment.name(),
            build_id: input.build_id,
            org_slug: org_slug.to_string(),
            is_new_app,
            warnings,
        }
    }
}

impl From<migrations::MigrationError> for PublishError {
    fn from(e: migrations::MigrationError) -> Self {
        PublishError::Migration(e)
    }
}

impl From<store::BuildStoreError> for PublishError {
    fn from(e: store::BuildStoreError) -> Self {
        PublishError::S3(e.to_string())
    }
}

impl PublishError {
    /// The status the publish route answers this refusal with.
    pub fn status(&self) -> StatusCode {
        match self {
            PublishError::UnknownOrg(_) | PublishError::UnknownProject(..) => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            PublishError::OxyAccessDenied { .. } => StatusCode::FORBIDDEN,
            PublishError::BadTarball(_) | PublishError::Invalid(_) => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            PublishError::DuplicateBuild { .. } | PublishError::ProjectMismatch { .. } => {
                StatusCode::CONFLICT
            }
            PublishError::InvalidBuildId(_)
            | PublishError::InvalidSlug(_)
            | PublishError::MissingFunctionArtifacts { .. }
            | PublishError::SharedEnvConflict { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            PublishError::Migration(e) => {
                if e.is_retryable() {
                    StatusCode::CONFLICT
                } else if e.is_author_fault() {
                    StatusCode::UNPROCESSABLE_ENTITY
                } else {
                    StatusCode::INTERNAL_SERVER_ERROR
                }
            }
            PublishError::SemanticPin(r) => {
                use crate::custom_apps_staging_pin::PinRefusal;
                match r {
                    PinRefusal::WithPromote => StatusCode::BAD_REQUEST,
                    PinRefusal::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
                    _ => StatusCode::UNPROCESSABLE_ENTITY,
                }
            }
            PublishError::DestinationMapping(r) => {
                use crate::custom_apps_nonproduction::MappingRefusal;
                match r {
                    MappingRefusal::Author(_) => StatusCode::UNPROCESSABLE_ENTITY,
                    MappingRefusal::Unchecked(_) => StatusCode::CONFLICT,
                }
            }
            PublishError::InvalidEnvironment(_) | PublishError::SandboxWithPromote => {
                StatusCode::BAD_REQUEST
            }
            PublishError::SandboxRefused | PublishError::SandboxTokenRefused => {
                StatusCode::FORBIDDEN
            }
            PublishError::UnknownEnvironment { .. } => StatusCode::NOT_FOUND,
            PublishError::EnvironmentDeleting { .. } => StatusCode::CONFLICT,
            PublishError::AgentDraft(refused) => refused.status(),
            PublishError::Db(_) | PublishError::S3(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// The machine-readable code of a refusal a client branches on. `None`
    /// for every refusal answered as plain text, as the route always has.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            PublishError::SandboxTokenRefused => Some("sandbox_token_refused"),
            PublishError::AgentDraft(refused) => Some(refused.code()),
            _ => None,
        }
    }
}

/// Decompression ceilings. The request body is capped at 64 MiB **compressed**
/// (`router/global.rs`), but gzip ratios turn that into tens of GB decompressed,
/// so without these a single publish — reachable by any org Admin or partner
/// since publish stopped being staff-only — can OOM the serve process and take
/// down every tenant on the replica. Generous for a real JS/CSS/asset bundle.
const MAX_DECOMPRESSED_BYTES: u64 = 256 * 1024 * 1024; // total across the bundle
const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024; // any single file
const MAX_FILES: usize = 20_000;

/// Parse the bundle's `oxy-app.json` if it ships one. A present-but-unparseable
/// manifest is a hard error rather than a silent `None`: it is the canonical
/// manifest `oxyc publish` uploads, so dropping it strips the whole `functions`
/// block and publishes a function-less app. Absent → `Ok(None)` (the caller
/// falls back to the explicit multipart `manifest` field).
fn parse_embedded_manifest(
    files: &[(String, Vec<u8>)],
) -> Result<Option<serde_json::Value>, PublishError> {
    files
        .iter()
        .find(|(p, _)| p == "oxy-app.json")
        .map(|(_, b)| {
            serde_json::from_slice::<serde_json::Value>(b).map_err(|e| {
                PublishError::BadTarball(format!(
                    "oxy-app.json in the bundle is not valid JSON: {e}"
                ))
            })
        })
        .transpose()
}

/// Decompress a gzipped tar into `(relative_path, bytes)` pairs, rejecting
/// absolute paths and `..` traversal. Directories are skipped. Bounded in total
/// bytes, per-entry bytes, and file count so a decompression bomb can't OOM the
/// process (see the limit constants above).
pub fn unpack_tar_gz(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, PublishError> {
    unpack_tar_gz_bounded(bytes, MAX_DECOMPRESSED_BYTES, MAX_ENTRY_BYTES, MAX_FILES)
}

/// The enforcement core, parameterized on its limits so the bomb-rejection paths
/// are testable without allocating the production-sized ceilings.
fn unpack_tar_gz_bounded(
    bytes: &[u8],
    max_total: u64,
    max_entry: u64,
    max_files: usize,
) -> Result<Vec<(String, Vec<u8>)>, PublishError> {
    let mut archive = Archive::new(GzDecoder::new(bytes));
    let entries = archive
        .entries()
        .map_err(|e| PublishError::BadTarball(e.to_string()))?;
    let mut out = Vec::new();
    let mut total_bytes: u64 = 0;
    for entry in entries {
        let entry = entry.map_err(|e| PublishError::BadTarball(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| PublishError::BadTarball(e.to_string()))?;
        if !is_safe_relative_path(&path) {
            return Err(PublishError::BadTarball(format!(
                "unsafe path in tarball: {}",
                path.display()
            )));
        }
        if entry.header().entry_type().is_dir() {
            continue;
        }
        let rel = path.to_string_lossy().trim_start_matches("./").to_string();
        if rel.is_empty() {
            continue;
        }
        if out.len() >= max_files {
            return Err(PublishError::BadTarball(format!(
                "bundle exceeds the {max_files}-file limit"
            )));
        }
        // `.take(cap + 1)` bounds the read itself — a header can lie about size,
        // so we never trust it and never let one entry allocate unbounded.
        let mut buf = Vec::new();
        entry
            .take(max_entry + 1)
            .read_to_end(&mut buf)
            .map_err(|e| PublishError::BadTarball(e.to_string()))?;
        if buf.len() as u64 > max_entry {
            return Err(PublishError::BadTarball(format!(
                "file '{rel}' exceeds the per-file {max_entry}-byte limit"
            )));
        }
        total_bytes = total_bytes.saturating_add(buf.len() as u64);
        if total_bytes > max_total {
            return Err(PublishError::BadTarball(format!(
                "bundle exceeds the {max_total}-byte decompressed limit"
            )));
        }
        out.push((rel, buf));
    }
    if !out.iter().any(|(p, _)| p == "index.html") {
        return Err(PublishError::BadTarball(
            "bundle has no index.html at its root".to_string(),
        ));
    }
    Ok(out)
}

async fn resolve_org(
    db: &DatabaseConnection,
    org_ref: &OrgRef,
) -> Result<organizations::Model, PublishError> {
    let row = match org_ref {
        OrgRef::Slug(slug) => {
            organizations::Entity::find()
                .filter(organizations::Column::Slug.eq(slug))
                .one(db)
                .await
        }
        OrgRef::Id(id) => organizations::Entity::find_by_id(*id).one(db).await,
    };
    row.map_err(|e| PublishError::Db(e.to_string()))?
        .ok_or_else(|| PublishError::UnknownOrg(org_ref.describe()))
}

/// Infer the org from the target workspace when the publisher didn't send one
/// (`--project` without `--org`). A workspace belongs to exactly one org, so
/// the project id alone determines it. Errors if the workspace is unknown or
/// somehow orphaned (no `org_id`).
async fn org_for_project(
    db: &DatabaseConnection,
    project_id: Uuid,
) -> Result<organizations::Model, PublishError> {
    let ws = workspaces::Entity::find_by_id(project_id)
        .one(db)
        .await
        .map_err(|e| PublishError::Db(e.to_string()))?
        .ok_or_else(|| PublishError::UnknownProject(project_id, "(inferred org)".to_string()))?;
    let org_id = ws.org_id.ok_or_else(|| {
        PublishError::UnknownProject(project_id, "(orphaned workspace)".to_string())
    })?;
    resolve_org(db, &OrgRef::Id(org_id)).await
}

/// Best-effort cross-org guard: if the project id resolves to a workspace,
/// its org must match. Unknown ids are rejected so a typo can't silently
/// create a row pointing at nothing.
///
/// Defers to the shared [`super::workspace_org`] predicate — the same one the
/// admin create/move endpoints use — and collapses all three of its failure
/// answers into one `UnknownProject`, which says only that this org has no such
/// project. Telling a missing workspace apart from another tenant's would leak
/// that the id exists somewhere.
async fn validate_project(
    db: &DatabaseConnection,
    project_id: Uuid,
    org_id: Uuid,
    org_slug: &str,
) -> Result<(), PublishError> {
    let matched = workspace_in_org(db, project_id, org_id)
        .await
        .map_err(|e| PublishError::Db(e.to_string()))?;
    match matched {
        WorkspaceOrgMatch::InOrg => Ok(()),
        _ => Err(PublishError::UnknownProject(
            project_id,
            org_slug.to_string(),
        )),
    }
}

/// The publish authorization gate. This route is **not** staff-gated at the router
/// (partners and CI publish through it), so this function is the whole decision —
/// it must never fail open. It resolves the caller to a
/// [`custom_apps_publish_authz::PublishActor`] and defers to the pure, tested
/// decision: an outsider or a plain Member is denied, an org **Admin+** may publish
/// their own app, a **partner** needs all three gates, and **staff** may publish
/// unless the workspace has locked Oxy out (`workspace_oxy_lockdown`). App-scoped
/// machine tokens take the confined `app_id`-match path above, before any of this.
async fn authorize_publish(
    db: &DatabaseConnection,
    org: &organizations::Model,
    input: &PublishInput,
) -> Result<(), PublishError> {
    // OIDC-minted machine token: authorize by "this token's app is the target app
    // AND the client consents". No user gates — the publisher registration the
    // exchange verified IS the assignment, and consent is re-checked here so a
    // revoke denies the next publish.
    if let Some(machine_app_id) = input.machine_app_id {
        let target =
            find_app(db, org.id, &input.app_slug)
                .await?
                .ok_or(PublishError::OxyAccessDenied {
                    org: org.slug.clone(),
                    project: input.project_id,
                })?;
        let consent = crate::custom_apps_publish_authz::consent_enabled(db, org.id).await;
        if target.id == machine_app_id && consent {
            return Ok(());
        }
        return Err(PublishError::OxyAccessDenied {
            org: org.slug.clone(),
            project: input.project_id,
        });
    }

    // An API token holding `app_publish` grants (API-tokens design §3.2): it
    // publishes the apps its grants name and no other, whatever else it reaches.
    if let Some(caller) = input.publisher.as_ref().filter(|c| c.holds_app_publish()) {
        let deny = || PublishError::OxyAccessDenied {
            org: org.slug.clone(),
            project: input.project_id,
        };
        let target = find_app(db, org.id, &input.app_slug)
            .await?
            .ok_or_else(deny)?;
        if !caller.publishes_app(org.id, target.id) {
            return Err(deny());
        }
        // A service account — a trust policy's `ci` token, or the account's
        // own — is authorized by the grant itself: an admin of the app's own
        // org put it there, and an account's grants are only ever in its own
        // org. That is why no `partner_publish_consent` is asked: consent is
        // an org agreeing to *someone else* publishing into it, and here the
        // org is publishing its own app. A person's token falls through: the
        // grant narrowed it to this app, and the person's own authority
        // decides, below, as it would without the grant.
        if caller.account_role_in(org.id).is_some() {
            return Ok(());
        }
        if caller.is_service_account() {
            return Err(deny());
        }
    }

    // Every non-machine publish routes through the ONE pure, tested decision
    // (`publish_decision`): it denies an outsider and a plain Member, allows an org
    // Admin+ and a partner with all three gates, and lets staff publish unless the
    // workspace locked Oxy out. Conflating "staff" with "not a member" here is
    // exactly how an outsider could publish into another tenant — so resolve the
    // actor once and let the policy decide, never a bare `!locked ⇒ Ok`.
    use crate::custom_apps_publish_authz as authz;
    let deny = || PublishError::OxyAccessDenied {
        org: org.slug.clone(),
        project: input.project_id,
    };
    // Both the recorded identity and the caller: a publisher with no address has
    // no path here, and authority is resolved for the caller's credential.
    let (Some(_), Some(_), Some(caller)) = (
        input.published_by,
        input.published_by_email.as_deref(),
        input.publisher.as_ref(),
    ) else {
        return Err(deny());
    };
    let actor = authz::resolve_actor(db, caller, org.id).await;

    // Read each side-fact only for the actor it applies to: the lockdown for staff,
    // consent for a partner. Everyone else pays no extra query.
    let staff_locked_out = if matches!(actor, authz::PublishActor::Staff) {
        custom_apps_auth::is_oxy_locked_down(db, input.project_id)
            .await
            .map_err(|e| PublishError::Db(e.to_string()))?
    } else {
        false
    };
    let consent = matches!(actor, authz::PublishActor::Partner { .. })
        && authz::consent_enabled(db, org.id).await;

    authz::publish_decision(&actor, staff_locked_out, consent).map_err(|_| deny())
}

async fn find_app(
    db: &DatabaseConnection,
    org_id: Uuid,
    slug: &str,
) -> Result<Option<apps::Model>, PublishError> {
    apps::Entity::find()
        .filter(apps::Column::OrgId.eq(org_id))
        .filter(apps::Column::Slug.eq(slug))
        .one(db)
        .await
        .map_err(|e| PublishError::Db(e.to_string()))
}

/// Delete the `app_builds` row a failed publish left behind, loudly.
///
/// Both callers are rollback paths that already know something went wrong, and
/// the failure here is usually the same transient DB problem that triggered the
/// rollback. That matters more than it used to: while the row survives, its
/// `build_id` is *taken*, so the operator's natural next move — retry the same
/// publish — now 409s where before it just worked. `gc_builds` reaps the row
/// eventually, but only on a later *successful* publish, which is exactly what
/// the operator can't do yet.
///
/// So the errors are still swallowed (a rollback must not mask the original
/// error with its own), but never silently: the warning is what lets an
/// operator tell "that id is taken" from "that id is taken *because cleanup
/// failed*", which have different remedies.
async fn discard_build_row(db: &DatabaseConnection, build_pk: Uuid, build_id: &str) {
    match app_builds::Entity::find_by_id(build_pk).one(db).await {
        Ok(Some(row)) => {
            if let Err(e) = row.delete(db).await {
                tracing::warn!(
                    "publish rollback: could not delete build row {build_pk} ({build_id:?}): {e} \
                     — retrying this publish with the same --build-id will now 409"
                );
            }
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(
            "publish rollback: could not load build row {build_pk} ({build_id:?}) to delete it: \
             {e} — retrying this publish with the same --build-id may now 409"
        ),
    }
}

/// True when this app already has a build under `build_id`.
///
/// There is no unique index on `(app_id, build_id)` to lean on, so this is the
/// enforcement point for the uniqueness the storage layer and the bundle cache
/// both assume. It races against a concurrent publish of the same id — two
/// publishes could both read `false` — but that window is orders of magnitude
/// narrower than the "CI re-run three days later" case this closes, and the
/// honest fix for the rest is a DB constraint, not a lock here.
async fn build_id_taken(
    db: &DatabaseConnection,
    app_id: Uuid,
    build_id: &str,
) -> Result<bool, PublishError> {
    app_builds::Entity::find()
        .filter(app_builds::Column::AppId.eq(app_id))
        .filter(app_builds::Column::BuildId.eq(build_id))
        .one(db)
        .await
        .map(|row| row.is_some())
        .map_err(|e| PublishError::Db(e.to_string()))
}

/// Insert (first publish) or update the `apps` row. Returns the app id
/// and `is_new = true` iff this call inserted a fresh row — the CLI uses
/// that to print "Registered new app" vs "Published new version of …"
/// so engineers spot accidental re-registration and intentional updates
/// without scanning the diff.
/// How to undo the app-row mutation `upsert_app` made, if a later step fails
/// before the build is durable. The row is mutated up front (a brand-new app
/// needs its id minted, and an existing one gets the new branch/name), so a
/// failure in `put_build` must not leave a half-published row behind.
///
/// An existing app's `project_id` is written only to re-home an app whose
/// workspace is deleted, orphaned or in another org — any other publish that
/// names another workspace is refused by `ensure_same_workspace`. Restoring it
/// here undoes that re-home on a failed publish, and is otherwise a no-op kept
/// as defence in depth:
/// `window.__OXY_APP__.projectId` is read from this row, and a wrong value
/// redirects a working bundle at another workspace's data plane.
enum AppMutationRollback {
    /// Newly created — undo by deleting the row.
    Created,
    /// Pre-existing — undo by restoring the values from before this publish.
    Updated {
        project_id: Uuid,
        branch: String,
        name: String,
        last_synced_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    },
    /// A sandbox publish, which never writes the app row: nothing to undo,
    /// and no cached row to drop.
    Untouched,
}

impl AppMutationRollback {
    async fn apply(self, db: &DatabaseConnection, app_id: Uuid) {
        match self {
            AppMutationRollback::Untouched => return,
            AppMutationRollback::Created => {
                if let Err(e) = apps::Entity::delete_by_id(app_id).exec(db).await {
                    tracing::warn!(
                        "publish rollback: could not delete newly-created app {app_id}: {e}"
                    );
                }
            }
            AppMutationRollback::Updated {
                project_id,
                branch,
                name,
                last_synced_at,
            } => {
                let active = apps::ActiveModel {
                    id: ActiveValue::Set(app_id),
                    project_id: ActiveValue::Set(project_id),
                    branch: ActiveValue::Set(branch),
                    name: ActiveValue::Set(name),
                    // Restore too, so a rolled-back row doesn't claim a sync that
                    // never durably happened.
                    last_synced_at: ActiveValue::Set(last_synced_at),
                    updated_at: ActiveValue::Set(Utc::now().fixed_offset()),
                    ..Default::default()
                };
                if let Err(e) = active.update(db).await {
                    tracing::warn!(
                        "publish rollback: could not restore prior state for app {app_id}: {e}"
                    );
                }
            }
        }
        // Restoring the row is only half the undo. `upsert_app` mutates it up
        // front and the success-path invalidation is at the very end of
        // `publish`, so the whole of `put_build` — seconds to minutes on a
        // real bundle — sits in between. A serve request landing in that
        // window caches the *mutated* row, and without this the cache keeps
        // serving the rolled-back values for up to `CACHE_TTL`: an `Updated`
        // app hands out a `window.__OXY_APP__.projectId` pointing at another
        // workspace while the old bytes still serve (the exact hazard this
        // type's doc comment above says it exists to prevent), and a
        // `Created` one keeps resolving after its row is deleted.
        //
        // Unconditional, and after the restore: a failed restore leaves the
        // cache just as wrong, and invalidating first would let a concurrent
        // request re-cache the bad row before the write lands.
        //
        // `crates/app/tests/custom_apps_cache_invalidation.rs` cannot catch
        // this — its detector is file-level and this file already contains a
        // call on the success path.
        //
        // This drops the whole map, not one app's entry, so every failed
        // publish costs a re-resolution storm across every app this replica
        // serves — the storm the cache exists to eliminate. Accepted for the
        // same reason renames take the wholesale drop: a minute of extra
        // queries beats a minute of a wrong `project_id`. Note the cost is
        // now reachable from a *failing* request, and a `put_build` failure
        // is guaranteed on a multi-replica deploy with no bucket configured
        // — so a publisher retrying into a bad bucket flushes the map on
        // every attempt.
        super::custom_apps_cache::invalidate_app_resolution_cache();
    }
}

/// Refuse a publish that names a different workspace than the app lives in.
///
/// `apps.project_id` is what `window.__OXY_APP__.projectId` is injected from,
/// and what the app's connectors, secrets and schedules resolve against. A
/// publish used to overwrite it — drafts included, since the draft and live
/// channels share the one row — so a draft publish with a different
/// `--project` silently moved the LIVE app onto another workspace's data.
/// Moving an app is an explicit admin action (`PATCH /api/customer-apps/{id}`),
/// never a side effect of shipping a build.
///
/// `branch` and `name` stay overwritable on publish: they label the app and
/// move no data.
///
/// The one exception: the app's workspace is not a live workspace of the app's
/// org — the shared [`super::workspace_org`] predicate answering anything but
/// `InOrg`. That covers three cases, one per failing variant:
///
/// - **Deleted.** `apps.project_id` has no foreign key, and deleting a
///   workspace leaves `apps` alone.
/// - **Orphaned.** The workspace exists but its `org_id` is NULL.
/// - **In another org.** The admin `PATCH /api/customer-apps/{id}` wrote
///   `project_id` without checking the workspace's org until #3174, so rows
///   written before that fix can name another org's workspace.
///
/// Refusing any of these is a dead end, because the `--project` the 409 names
/// cannot repair the app. A deleted workspace is gone, and an orphaned one
/// belongs to no org. Another org's workspace fails `validate_project` when the
/// publisher names this org, and otherwise resolves to that org, where
/// `find_app` misses this app and the publish registers a second one. Moving
/// the app by PATCH is app-admin only. Returns `Ok(true)` for the re-home,
/// which the caller writes, and `Ok(false)` when the workspaces already match.
///
/// A re-home never moves an app onto a workspace outside its own org, because
/// of what `publish` resolved first: `org` contains `input.project_id`
/// (`validate_project` when the publisher named an org, `org_for_project`
/// derives the org from the workspace when it did not), and `existing` was
/// found by `find_app` under that same `org.id`.
pub(super) async fn ensure_same_workspace(
    db: &DatabaseConnection,
    existing: &apps::Model,
    input: &PublishInput,
) -> Result<bool, PublishError> {
    if existing.project_id == input.project_id {
        return Ok(false);
    }
    let current = workspace_in_org(db, existing.project_id, existing.org_id)
        .await
        .map_err(|e| PublishError::Db(e.to_string()))?;
    if current != WorkspaceOrgMatch::InOrg {
        return Ok(true);
    }
    Err(PublishError::ProjectMismatch {
        app_slug: existing.slug.clone(),
        app_id: existing.id,
        existing_project: existing.project_id,
        requested_project: input.project_id,
    })
}

async fn upsert_app(
    db: &DatabaseConnection,
    org: &organizations::Model,
    input: &PublishInput,
) -> Result<(Uuid, bool, AppMutationRollback), PublishError> {
    let now = Utc::now().fixed_offset();
    let existing = find_app(db, org.id, &input.app_slug).await?;
    if let Some(row) = existing {
        // Authoritative check, before the row is touched. The fast path in
        // `publish` catches this before the bundle is unpacked, but the row can
        // change between the two reads.
        let rehome = ensure_same_workspace(db, &row, input).await?;
        let id = row.id;
        let slug = row.slug.clone();
        // Snapshot the fields this update overwrites, so a later failure can
        // restore them rather than stranding the app on a half-publish.
        let prior = AppMutationRollback::Updated {
            project_id: row.project_id,
            branch: row.branch.clone(),
            name: row.name.clone(),
            last_synced_at: row.last_synced_at,
        };
        let stray_workspace = row.project_id;
        let mut active: apps::ActiveModel = row.into();
        // `project_id` is written only for a re-home: otherwise
        // `ensure_same_workspace` above has already proved it equal.
        if rehome {
            active.project_id = ActiveValue::Set(input.project_id);
        }
        if let Some(b) = &input.branch {
            active.branch = ActiveValue::Set(b.clone());
        }
        if let Some(name) = &input.name {
            active.name = ActiveValue::Set(name.clone());
        }
        active.last_synced_at = ActiveValue::Set(Some(now));
        active.updated_at = ActiveValue::Set(now);
        active
            .update(db)
            .await
            .map_err(|e| PublishError::Db(e.to_string()))?;
        if rehome {
            tracing::warn!(
                "publish re-homed app {}/{slug} ({id}) from workspace {stray_workspace} (deleted, orphaned, or in another org) to workspace {}",
                org.slug,
                input.project_id
            );
        }
        return Ok((id, false, prior));
    }

    let id = Uuid::new_v4();
    let name = input
        .name
        .clone()
        .unwrap_or_else(|| humanize_slug(&input.app_slug));
    let model = apps::ActiveModel {
        // Leave to the DB default ('org'): a new app is org-visible unless
        // explicitly restricted later.
        visibility: sea_orm::ActiveValue::NotSet,
        id: ActiveValue::Set(id),
        slug: ActiveValue::Set(input.app_slug.clone()),
        name: ActiveValue::Set(name),
        org_id: ActiveValue::Set(org.id),
        project_id: ActiveValue::Set(input.project_id),
        branch: ActiveValue::Set(input.branch.clone().unwrap_or_else(|| "main".to_string())),
        source_repo: ActiveValue::Set("oxy-hq/customer-apps".to_string()),
        status: ActiveValue::Set("created".to_string()),
        source_type: ActiveValue::Set("s3".to_string()),
        source_config: ActiveValue::Set(serde_json::json!({})),
        last_synced_at: ActiveValue::Set(Some(now)),
        manifest_override: ActiveValue::NotSet,
        bootstrap_pr_url: ActiveValue::NotSet,
        // Stays a draft until promoted (here or via /admin/apps).
        published_at: ActiveValue::NotSet,
        repo_path: ActiveValue::Set(Some(format!("{}/{}", org.slug, input.app_slug))),
        last_promoted_by: ActiveValue::NotSet,
        last_promoted_at: ActiveValue::NotSet,
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    };
    model
        .insert(db)
        .await
        .map_err(|e| PublishError::Db(e.to_string()))?;
    Ok((id, true, AppMutationRollback::Created))
}

fn humanize_slug(slug: &str) -> String {
    slug.split('-')
        .filter(|s| !s.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

async fn record_build(
    db: &DatabaseConnection,
    app_id: Uuid,
    input: &PublishInput,
    (s3_prefix, published_to): (String, AppEnvironment),
    manifest_json: Option<serde_json::Value>,
    // The sandbox agent token that published this build, to a sandbox or as
    // a draft; `None` for everyone else's (`agent_publish::author`).
    published_token_id: Option<Uuid>,
) -> Result<Uuid, PublishError> {
    let build_pk = Uuid::new_v4();
    let model = app_builds::ActiveModel {
        id: ActiveValue::Set(build_pk),
        app_id: ActiveValue::Set(app_id),
        build_id: ActiveValue::Set(input.build_id.clone()),
        s3_prefix: ActiveValue::Set(s3_prefix),
        manifest_json: ActiveValue::Set(manifest_json),
        created_at: ActiveValue::Set(Utc::now().fixed_offset()),
        published_by: ActiveValue::Set(input.published_by),
        published_via: ActiveValue::Set(input.published_via.clone()),
        source_repo: ActiveValue::Set(input.source_repo.clone()),
        commit_sha: ActiveValue::Set(input.commit_sha.clone()),
        source_branch: ActiveValue::Set(input.branch.clone()),
        // Gate 1 (byte-level validation) already ran and passed to reach here —
        // publish 422s otherwise, before storing. Record it so promotion can be
        // gated on a persisted status. A future deploy-time probe (gate 2) may
        // downgrade this to `failed`; nothing sets `pending` yet.
        validation_status: ActiveValue::Set("passed".to_string()),
        validation_detail: ActiveValue::NotSet,
        // Validated in `publish` before any work (draft-only, same workspace).
        semantic_revision_id: ActiveValue::Set(input.semantic_revision_id),
        // With the row, before any pointer moves: retention and read-back tell
        // a sandbox's build from a channel's by this, not by a pointer event
        // that may not have committed yet.
        published_to: ActiveValue::Set(Some(published_to.name())),
        published_token_id: ActiveValue::Set(published_token_id),
    };
    model
        .insert(db)
        .await
        .map_err(|e| PublishError::Db(e.to_string()))?;
    Ok(build_pk)
}

/// Extract `(name, per-function manifest JSON)` pairs from the bundle
/// manifest's `functions` block. Empty when the manifest declares none
/// (today's static-bundle default), so this is a no-op for function-less apps.
pub(super) fn function_specs(
    manifest_json: Option<&serde_json::Value>,
) -> Vec<(String, serde_json::Value)> {
    manifest_json
        .and_then(|m| m.get("functions"))
        .and_then(|f| f.as_object())
        .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

/// Whether `name` is safe as a `/fn/<name>` route key and an artifact-key path
/// segment. THE canonical function-name rule (`^[a-z][a-z0-9-]{0,63}$`): the CLI
/// (`oxyc publish`), the vite-plugin build gate, and the SDK manifest loader all
/// mirror it, and `custom_apps_serve::sources::s3_object_key` relies on every
/// stored name matching it to prove a rewritten request path can never resolve
/// to a function artifact. Enforced at publish by [`check_declared_functions`],
/// before the build is stored, so a bundle POSTed straight to the API —
/// bypassing the CLI — can't store a name that breaks the invariant;
/// [`record_functions`] checks it a second time, as defence in depth.
pub(crate) fn is_valid_function_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Reject any declared function name that isn't [`is_valid_function_name`],
/// as one `PublishError` before any row is written. Split out so it is
/// unit-testable without a database.
fn validate_function_names(specs: &[(String, serde_json::Value)]) -> Result<(), PublishError> {
    for (name, _) in specs {
        if !is_valid_function_name(name) {
            return Err(PublishError::BadTarball(format!(
                "invalid function name {name:?} — must match ^[a-z][a-z0-9-]{{0,63}}$ \
                 (a lowercase letter, then up to 63 lowercase letters, digits or hyphens)"
            )));
        }
    }
    Ok(())
}

/// Build-store key of a function's bundled JS artifact, matching the layout
/// `oxyc publish` uploads (`<build_prefix>functions/<name>.js`) and the
/// `app_functions.artifact_key` contract.
fn function_artifact_key(build_prefix: &str, name: &str) -> String {
    format!("{build_prefix}functions/{name}.js")
}

/// Declared functions whose bundled JS (`functions/<name>.js`, the key
/// [`function_artifact_key`] records) the bundle does not carry, sorted.
///
/// Without this check such a bundle published with a 200: `record_functions`
/// wrote a row per declared function pointing at an artifact that was never
/// uploaded, so the app went live with every one of those functions broken.
/// That is what an `oxy` CLI older than 0.5.96 produces for an app with
/// functions, because it never bundled them.
fn missing_function_artifacts(
    files: &[(String, Vec<u8>)],
    specs: &[(String, serde_json::Value)],
) -> Vec<String> {
    if specs.is_empty() {
        return Vec::new();
    }
    let present: std::collections::HashSet<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
    let mut missing: Vec<String> = specs
        .iter()
        .map(|(name, _)| name)
        .filter(|name| !present.contains(function_artifact_key("", name).as_str()))
        .cloned()
        .collect();
    missing.sort();
    missing
}

/// Refuse a bundle whose manifest declares a function it cannot ship: an
/// invalid function name first, then a declared function without its bundled
/// JS. Split out of `publish` so the order is unit-testable without a database.
///
/// Names go first because an invalid one (`Bad_Name`, `../../evil`) has no
/// `functions/<name>.js` either: the CLI refuses to bundle it, and a traversal
/// name cannot be a tar entry at all. Checking artifacts first reported it as
/// missing and told the publisher to upgrade the CLI, instead of naming the
/// function that is wrong.
fn check_declared_functions(
    files: &[(String, Vec<u8>)],
    specs: &[(String, serde_json::Value)],
) -> Result<(), PublishError> {
    validate_function_names(specs)?;
    refuse_reserved_notify_kinds(specs)?;
    let missing = missing_function_artifacts(files, specs);
    if !missing.is_empty() {
        return Err(PublishError::MissingFunctionArtifacts { missing });
    }
    Ok(())
}

/// A function may not declare a `notify` kind in the platform's `oxy.`
/// namespace: `ctx.notify` refuses to send one, so the publish says so first.
fn refuse_reserved_notify_kinds(specs: &[(String, serde_json::Value)]) -> Result<(), PublishError> {
    use oxy_app_core::notifications::input::{RESERVED_KIND_PREFIX, reserved_kinds_declared};
    for (name, spec) in specs {
        if let Some(kind) = reserved_kinds_declared(spec).first() {
            return Err(PublishError::Invalid(
                crate::custom_apps_validate::BundleValidation::new(
                    "notify_kind_reserved",
                    format!(
                        "Function {name:?} declares the notify kind {kind:?}. Kinds starting \
                         with {RESERVED_KIND_PREFIX:?} are reserved for Oxygen's own notices."
                    ),
                    format!(
                        "Rename the kind in functions.{name}.notify.kinds and in the ctx.notify calls that send it."
                    ),
                ),
            ));
        }
    }
    Ok(())
}

/// The manifest's `notifications` block, checked
/// (`notifications::catalog::validate`): the first problem refuses the
/// publish as a 422 naming it; otherwise its warnings join the publish's.
fn check_notification_catalogue(
    manifest: Option<&serde_json::Value>,
) -> Result<Vec<String>, PublishError> {
    oxy_app_core::notifications::catalog::validate::check(manifest).map_err(|p| {
        PublishError::Invalid(crate::custom_apps_validate::BundleValidation::new(
            p.check,
            p.message,
            p.remediation,
        ))
    })
}

/// The manifest's `widgets` block, checked (`oxy_app_core::widgets::validate`):
/// the first problem refuses the publish as a 422 naming it.
fn check_widgets(manifest: Option<&serde_json::Value>) -> Result<(), PublishError> {
    oxy_app_core::widgets::validate::check(manifest).map_err(|p| {
        PublishError::Invalid(crate::custom_apps_validate::BundleValidation::new(
            p.check,
            p.message,
            p.remediation,
        ))
    })
}

/// Record one `app_functions` row per declared function, keyed to this build,
/// so the `/fn/<name>` route can resolve them. Without this the bundled JS
/// ships in the build store but the runtime can never find it (→ 404).
async fn record_functions(
    db: &DatabaseConnection,
    app_id: Uuid,
    build_pk: Uuid,
    build_prefix: &str,
    specs: &[(String, serde_json::Value)],
) -> Result<(), PublishError> {
    // Names become artifact-key path segments + `/fn/<name>` route keys; a bundle
    // published straight through the API skips the CLI's check, so enforce the
    // shape here — the load-bearing spot for `sources::s3_object_key`. Reject the
    // whole publish (the caller rolls the stored build back out). `publish`
    // already checked this before storing anything (`check_declared_functions`);
    // this stays as defence in depth.
    validate_function_names(specs)?;
    for (name, spec) in specs {
        let model = app_functions::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            app_id: ActiveValue::Set(app_id),
            build_id: ActiveValue::Set(build_pk),
            name: ActiveValue::Set(name.clone()),
            manifest_json: ActiveValue::Set(Some(spec.clone())),
            artifact_key: ActiveValue::Set(function_artifact_key(build_prefix, name)),
            created_at: ActiveValue::Set(Utc::now().fixed_offset()),
        };
        model
            .insert(db)
            .await
            .map_err(|e| PublishError::Db(e.to_string()))?;
    }
    Ok(())
}

/// Point the channel(s) at the new build: staging always; production too when
/// promoting. One transaction holding the app's row: staging's pointer moves
/// and its `staging / publish` event is recorded **first**, then production's
/// moves through the one production-pointer function
/// (`custom_apps_promote::move_locked`), which finds that event as the
/// evidence invariant 1 asks for. The pointers are the environments' rows
/// (`custom_apps_environments::record_move`), moved in that one transaction.
/// (A promoting publish applied the build's migrations before calling this.)
async fn set_pointers(
    db: &DatabaseConnection,
    app_id: Uuid,
    build_pk: Uuid,
    promote: bool,
    actor: Option<Uuid>,
) -> Result<(), PublishError> {
    if promote {
        // Promotion gate (validator-can't-be-bypassed): only a build whose
        // validation is recorded `passed` may go live. Redundant on this path —
        // the build was just gate-1 validated — but keeps every promotion point
        // honest, so a build that hasn't passed can never reach the live channel.
        gate_promotion(db, build_pk).await?;
    }
    let db_err = |e: sea_orm::DbErr| PublishError::Db(e.to_string());
    let txn = db.begin().await.map_err(db_err)?;
    let row = super::custom_apps_promote::production::lock_app(&txn, app_id)
        .await
        .map_err(|e| PublishError::Db(format!("app {app_id} could not be locked: {e}")))?;
    super::custom_apps_environments::record_move(
        &txn,
        app_id,
        &AppEnvironment::Staging,
        Some(build_pk),
        super::custom_apps_environments::EnvAction::Publish,
        actor,
    )
    .await
    .map_err(db_err)?;
    if promote {
        let mv = super::custom_apps_promote::ProductionMove {
            app_id,
            build: build_pk.into(),
            action: super::custom_apps_environments::EnvAction::Promote,
            actor,
            expect_current: None,
            refuse_same: false,
        };
        super::custom_apps_promote::move_locked(&txn, row, &mv)
            .await
            .map_err(|e| PublishError::Db(format!("production's pointer could not move: {e}")))?;
    }
    txn.commit().await.map_err(db_err)?;
    Ok(())
}

/// Promotion gate (the enforcement half of "the validator can't be bypassed"):
/// a build may reach the published/live channel only if its recorded validation
/// status is `passed`. Gate 1 stamps `passed` at publish; a future deploy-time
/// probe (gate 2) may downgrade to `failed`. Checked at every promotion point so
/// no build can go live without a recorded pass. Today every stored build is
/// `passed`, so this is dormant — but the boundary now exists and is honest.
async fn gate_promotion(db: &DatabaseConnection, build_pk: Uuid) -> Result<(), PublishError> {
    let build = app_builds::Entity::find_by_id(build_pk)
        .one(db)
        .await
        .map_err(|e| PublishError::Db(e.to_string()))?
        .ok_or_else(|| PublishError::Db(format!("build {build_pk} vanished mid-publish")))?;
    if let Err(PromoteRefusal::ValidationNotPassed { status }) =
        BuildState::of(&build).validation.check()
    {
        return Err(PublishError::Invalid(
            crate::custom_apps_validate::BundleValidation::new(
                "validation_not_passed",
                format!(
                    "build validation status is '{status}', not 'passed' — it cannot be promoted to live"
                ),
                "Re-publish after the bundle passes validation.",
            ),
        ));
    }
    Ok(())
}

/// Delete builds beyond retention — the newest `KEEP_BUILDS` builds production
/// served, the newest `KEEP_BUILDS` only a sandbox ever served, and the newest
/// `KEEP_BUILDS` of the rest (`custom_apps_sandboxes::retention`) — never touching a build a channel pointer
/// or an `app_environments` row references. Row before bytes; best-effort on the S3 side.
async fn gc_builds(db: &DatabaseConnection, app_id: Uuid, protect: &[Uuid]) {
    let Some(protect) = gc_protected(db, app_id, protect).await else {
        return;
    };
    let Some(beyond) = gc_beyond_retention(db, app_id).await else {
        return;
    };
    for build in beyond {
        if protect.contains(&build.id) {
            continue;
        }
        let build_label = build.build_id.clone();
        // Captured before the row goes: once it is deleted, this log line is the only
        // record of where an orphan from a failed byte delete lives.
        let s3_prefix = build.s3_prefix.clone();
        // Row first. If anything still names this build, the environment FK refuses
        // and the bytes are never touched; a failed byte delete afterwards only
        // leaves an orphan prefix, never a row pointing at deleted bytes.
        if let Err(e) = build.delete(db).await {
            tracing::warn!("gc_builds row delete failed ({build_label}): {e}");
            continue;
        }
        if let Err(e) = store::delete_build(app_id, &build_label).await {
            tracing::warn!("gc_builds S3 delete failed ({build_label}, prefix {s3_prefix}): {e}");
        }
    }
}

/// Every build GC must leave alone: the caller's, the builds the live channels
/// point at, and the builds environment rows name. `None` — skip GC — when
/// any of them cannot be read: unknown is never taken for unprotected.
async fn gc_protected(
    db: &DatabaseConnection,
    app_id: Uuid,
    protect: &[Uuid],
) -> Option<Vec<Uuid>> {
    // Always protect the builds the live environments point at, regardless of
    // what the caller passed. GC that reaps the currently-served build is a
    // silent outage: the app 404s (`custom_apps_serve/sources.rs`) with no
    // publish having touched the live environment. The environments' rows are
    // read below, so the guarantee can't be lost by a caller forgetting to
    // thread them through; the app row is read only to skip GC for a gone app.
    let mut protect: Vec<Uuid> = protect.to_vec();
    match apps::Entity::find_by_id(app_id).one(db).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            tracing::warn!(
                "gc_builds: app {app_id} not found; skipping GC so a live build can't be reaped"
            );
            return None;
        }
        Err(e) => {
            tracing::warn!("gc_builds: could not load app {app_id} ({e}); skipping GC");
            return None;
        }
    }
    // Environment rows name the builds — production's, staging's and every
    // sandbox's. Same fail-safe as above: unknown means skip GC.
    match super::custom_apps_environments::protected_build_ids(db, app_id).await {
        Ok(ids) => protect.extend(ids),
        Err(e) => {
            tracing::warn!(
                "gc_builds: could not load environment builds for app {app_id} ({e}); skipping GC"
            );
            return None;
        }
    }
    Some(protect)
}

/// The app's builds beyond retention, newest first. Builds production served,
/// builds only a sandbox ever served, and the drafts left over are each kept
/// in a window of their own (`retention::beyond`), so neither an afternoon of
/// sandbox publishes nor any number of drafts pushes out a build production
/// would roll back to. `None` — skip GC — when the builds or their events
/// cannot be read.
async fn gc_beyond_retention(
    db: &DatabaseConnection,
    app_id: Uuid,
) -> Option<Vec<app_builds::Model>> {
    let builds = match app_builds::Entity::find()
        .filter(app_builds::Column::AppId.eq(app_id))
        .order_by_desc(app_builds::Column::CreatedAt)
        .all(db)
        .await
    {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("gc_builds list failed for app {app_id}: {e}");
            return None;
        }
    };
    let newest_first: Vec<Uuid> = builds.iter().map(|build| build.id).collect();
    let beyond = match retention::beyond(db, app_id, &newest_first, KEEP_BUILDS).await {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!(
                "gc_builds: could not tell an app's builds apart for app {app_id} ({e}); skipping GC"
            );
            return None;
        }
    };
    Some(
        builds
            .into_iter()
            .filter(|build| beyond.contains(&build.id))
            .collect(),
    )
}

/// Today's publish: [`publish_to`] the app's channels — staging, and
/// production with `promote`.
pub async fn publish(input: PublishInput) -> Result<PublishResult, PublishError> {
    publish_to(input, PublishTarget::Channels).await
}

/// Store `input`'s bundle as a build and point `target` at it.
///
/// A [`PublishTarget::Sandbox`] publish stores and records the build exactly
/// as any other, and differs only where `custom_apps_sandboxes::publish` says:
/// it is admitted as staging is opened, never writes the app row or another
/// environment's pointer, registers no schedule, applies no migration to
/// production or to staging, and queues the sandbox's own instead of
/// staging's — its Airhouse sibling's, and its own OLTP schema on the org's
/// staging branch, seeded if need be and migrated with the bundle's OLTP
/// files.
///
/// A sandbox agent token is refused here, before anything else, unless the
/// target is a sandbox and the publish does not promote, or it is a draft to
/// the staging the token was granted ([`PublishTarget::AgentDraft`]) —
/// whoever the caller of this function is
/// (`custom_apps_sandboxes::agent_publish`).
pub async fn publish_to(
    input: PublishInput,
    target: PublishTarget,
) -> Result<PublishResult, PublishError> {
    let agent = sandbox_agent_publish::AgentPublish::of(&input, &target)?;
    let published = pipeline::store_and_point(input, target).await;
    match agent {
        Some(agent) => published.map_err(|refused| agent.sees(refused)),
        None => published,
    }
}

/// The app row a publish works on: `(id, created by this publish, how to undo
/// the write)`. The channels' publish upserts it. A sandbox's (`sandbox_app`
/// is its admitted app) never writes it: `upsert_app` rewrites the row's
/// `branch` and `name`, and two sandboxes would fight over production's. Nor
/// does a sandbox agent token's draft, admitted the same way: it moves the
/// draft pointer and leaves every other column of the row as it found it.
async fn app_row_for(
    db: &DatabaseConnection,
    org: &organizations::Model,
    input: &PublishInput,
    sandbox_app: Option<Uuid>,
) -> Result<(Uuid, bool, AppMutationRollback), PublishError> {
    match sandbox_app {
        Some(app_id) => Ok((app_id, false, AppMutationRollback::Untouched)),
        None => upsert_app(db, org, input).await,
    }
}

/// The pointer move of a publish to `target`. A sandbox's is its own row
/// alone; one deleted since it was admitted has no row to move, and the
/// caller rolls the stored build back.
async fn move_pointers(
    db: &DatabaseConnection,
    target: &PublishTarget,
    app_id: Uuid,
    build_pk: Uuid,
    input: &PublishInput,
) -> Result<(), PublishError> {
    match target {
        PublishTarget::Channels => {
            set_pointers(db, app_id, build_pk, input.promote, input.published_by).await
        }
        // Staging's pointer and its mirror, under the app's row lock, with
        // the draft's guards asked again of the locked row — and never
        // production's, whatever the input says.
        PublishTarget::AgentDraft => {
            sandbox_agent_draft::move_draft_pointer(db, app_id, build_pk, input).await
        }
        PublishTarget::Sandbox(environment) => {
            let mover = sandbox_publish::Mover {
                actor: input.published_by,
                own: sandbox_agent_publish::token_of(input),
            };
            sandbox_publish::move_pointer(db, app_id, environment, build_pk, mover).await
        }
    }
}

/// Reconcile the schedule rows of the non-production environment `target`'s
/// publish moved (`custom_apps_env_schedules`): staging for the channels and
/// an agent's draft, the sandbox for a sandbox's. A failure is the publish's
/// warning, never its failure; the next publish or flip of the switch retries.
async fn reconcile_moved_environment(
    db: &DatabaseConnection,
    target: &PublishTarget,
    app_id: Uuid,
) -> Vec<String> {
    let environment = target.published_to();
    let app = match apps::Entity::find_by_id(app_id).one(db).await {
        Ok(Some(app)) => app,
        Ok(None) => return Vec::new(),
        Err(e) => return vec![environment_schedules_warning(&environment, &e)],
    };
    env_schedules::reconcile_environment(db, &app, &environment)
        .await
        .unwrap_or_else(|e| vec![environment_schedules_warning(&environment, &e)])
}

fn environment_schedules_warning(environment: &AppEnvironment, e: &sea_orm::DbErr) -> String {
    tracing::warn!("publish: the {environment} environment's schedules were not reconciled: {e}");
    format!(
        "the {environment} environment's schedules could not be reconciled ({e}); publish to it \
         again, or flip its schedules switch, to retry"
    )
}

/// Drop the app-resolution cache after a publish that moved a pointer it
/// caches. A sandbox publish moved none — a sandbox's build is read from its
/// row on every request — so it drops nothing.
fn drop_resolution_cache(target: &PublishTarget) {
    // A sandbox agent token's draft dropped it where its pointer moved
    // (`agent_draft::move_draft_pointer`).
    if matches!(target, PublishTarget::Channels) {
        super::custom_apps_cache::invalidate_app_resolution_cache();
    }
}

/// Queue the migrations of the homes `target`'s pointer move reaches; the
/// warnings are the publish's. The channels' publish moved staging's pointer:
/// staging's Airhouse sibling and the org's OLTP staging branch. A sandbox's
/// moved the sandbox's: its own Airhouse sibling and its own schema on that
/// branch, each under a task kind of its own — never staging's schema.
async fn queue_migrations_for(
    db: &DatabaseConnection,
    target: &PublishTarget,
    built: staging_task::StagingBuild<'_>,
    oltp: &[migrations::DeclaredMigration],
    airhouse: &[migrations::DeclaredMigration],
) -> Vec<String> {
    match target {
        PublishTarget::Channels | PublishTarget::AgentDraft => {
            staging_task::queue_staging_migrations(db, built, oltp, airhouse).await
        }
        PublishTarget::Sandbox(environment) => {
            let sandbox_build = sandbox_publish::SandboxBuild {
                app_id: built.app_id,
                app_slug: built.app_slug,
                workspace_id: built.workspace_id,
                org_id: built.org_id,
                build_pk: built.build_pk,
                environment,
            };
            sandbox_publish::queue_migrations(db, sandbox_build, oltp, airhouse).await
        }
    }
}

/// `POST /api/customer-apps/publish` — thin multipart shim over [`publish`].
///
/// The app-admin guard middleware has already authenticated the caller; we
/// pull the user via `AuthenticatedUserExtractor` (before `Multipart`, which
/// consumes the body) to stamp `published_by` on the build.
pub async fn publish_handler(
    // The authenticated user, with the key or token the request used and where
    // it came from: what the publish is audited as (`custom_apps_publish_audit`).
    actor: oxy_app_core::audit::RequestActor,
    // Present iff authenticated via an app publish token; its `app_id` is set only
    // for OIDC-minted machine tokens.
    marker: Option<axum::Extension<oxy_auth::types::AppPublishTokenAuth>>,
    multipart: Multipart,
) -> Result<Json<PublishResult>, PublishRefusal> {
    let user = &actor.user;
    let marker = marker.as_ref().map(|axum::Extension(m)| m);
    let publisher = Publisher::from_request(user, marker);
    let credential = sandbox_publish::PublishCredential::of(user, marker);
    let form = PublishForm::from_multipart(multipart).await?;
    let target = sandbox_publish::target_of(form.environment.as_deref(), form.promote, credential)?;
    let input = form.into_input(publisher);
    // A sandbox agent token is told a sandbox that is not its own as its
    // other routes tell it; everyone else reads the text they always did.
    let agent = credential.is_agent();
    let published = publish_to(input, target)
        .await
        .map_err(|refused| PublishRefusal::told(refused, agent))?;
    // One audit row for every publish that succeeded, whatever the credential.
    if let Ok(db) = oxy::database::client::establish_connection().await {
        super::custom_apps_publish_audit::published(&db, &actor, marker, &published).await;
    }
    Ok(Json(published))
}

#[cfg(test)]
mod publisher_tests {
    use super::*;
    use oxy_auth::types::{AppPublishTokenAuth, AuthenticatedUser};

    fn human() -> AuthenticatedUser {
        AuthenticatedUser {
            id: Uuid::new_v4(),
            email: Some("admin@example.com".into()),
            ..AuthenticatedUser::machine_publisher()
        }
    }

    #[test]
    fn a_machine_publish_records_no_user_and_its_workflow() {
        let app = Uuid::new_v4();
        let marker = AppPublishTokenAuth {
            token_id: Uuid::new_v4(),
            app_id: Some(app),
            machine_identity: Some(
                "github-oidc:acme/app/.github/workflows/p.yml@refs/heads/main env=prod".into(),
            ),
        };
        let p = Publisher::from_request(&AuthenticatedUser::machine_publisher(), Some(&marker));
        // The nil principal id must never reach a `users(id)` FK.
        assert_eq!(p.published_by, None);
        assert_eq!(p.published_by_email, None);
        assert_eq!(p.machine_app_id, Some(app));
        assert_eq!(p.published_via, marker.machine_identity);
    }

    #[test]
    fn a_session_publish_stamps_its_user() {
        let user = human();
        let p = Publisher::from_request(&user, None);
        assert_eq!(p.published_by, Some(user.id));
        assert_eq!(p.published_by_email, user.email);
        assert_eq!(p.machine_app_id, None);
        assert_eq!(p.published_via, None);
    }

    #[test]
    fn a_partner_minted_app_scoped_token_stamps_its_minter() {
        // App-scoped but human-owned: confined to the app, attributed to a person.
        let user = human();
        let app = Uuid::new_v4();
        let marker = AppPublishTokenAuth {
            token_id: Uuid::new_v4(),
            app_id: Some(app),
            machine_identity: None,
        };
        let p = Publisher::from_request(&user, Some(&marker));
        assert_eq!(p.published_by, Some(user.id));
        assert_eq!(p.machine_app_id, Some(app));
        assert_eq!(p.published_via, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    fn make_tar_gz(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, bytes) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, path, *bytes).unwrap();
        }
        let tar_bytes = builder.into_inner().unwrap();
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&tar_bytes).unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn unpack_roundtrips_and_strips_dot_slash() {
        let gz = make_tar_gz(&[
            ("./index.html", b"<html>"),
            ("assets/app.js", b"console.log(1)"),
        ]);
        let files = unpack_tar_gz(&gz).expect("unpack");
        assert!(
            files
                .iter()
                .any(|(p, b)| p == "index.html" && b == b"<html>")
        );
        assert!(files.iter().any(|(p, _)| p == "assets/app.js"));
    }

    #[test]
    fn unpack_rejects_missing_index() {
        let gz = make_tar_gz(&[("assets/app.js", b"x")]);
        let err = unpack_tar_gz(&gz).unwrap_err();
        assert!(matches!(err, PublishError::BadTarball(_)));
    }

    #[test]
    fn unpack_rejects_oversized_single_entry() {
        // 20 bytes with a 10-byte per-entry cap → rejected, and the read is
        // bounded by `.take(cap+1)` so a lying header can't OOM us first.
        let gz = make_tar_gz(&[("index.html", &[b'a'; 20])]);
        let err = unpack_tar_gz_bounded(&gz, 1_000, 10, 100).unwrap_err();
        assert!(
            matches!(&err, PublishError::BadTarball(m) if m.contains("per-file")),
            "expected per-file limit error, got {err:?}"
        );
    }

    #[test]
    fn unpack_rejects_oversized_total() {
        // Each file is under the per-entry cap, but together they blow the total.
        let gz = make_tar_gz(&[
            ("index.html", &[b'a'; 8]),
            ("a.js", &[b'b'; 8]),
            ("b.js", &[b'c'; 8]),
        ]);
        let err = unpack_tar_gz_bounded(&gz, 20, 100, 100).unwrap_err();
        assert!(
            matches!(&err, PublishError::BadTarball(m) if m.contains("decompressed limit")),
            "expected total-bytes limit error, got {err:?}"
        );
    }

    #[test]
    fn unpack_rejects_too_many_files() {
        let gz = make_tar_gz(&[("index.html", b"x"), ("a.js", b"x"), ("b.js", b"x")]);
        let err = unpack_tar_gz_bounded(&gz, 1_000, 100, 2).unwrap_err();
        assert!(
            matches!(&err, PublishError::BadTarball(m) if m.contains("file limit")),
            "expected file-count limit error, got {err:?}"
        );
    }

    #[test]
    fn embedded_manifest_present_but_malformed_is_rejected() {
        // The canonical `oxyc publish` path: a malformed oxy-app.json must 4xx,
        // not silently strip the functions block and publish function-less.
        let files = vec![("oxy-app.json".to_string(), b"{ not: json".to_vec())];
        let err = parse_embedded_manifest(&files).unwrap_err();
        assert!(
            matches!(&err, PublishError::BadTarball(m) if m.contains("oxy-app.json")),
            "expected a BadTarball naming oxy-app.json, got {err:?}"
        );
    }

    #[test]
    fn embedded_manifest_absent_falls_through() {
        let files = vec![("index.html".to_string(), b"<html>".to_vec())];
        assert!(parse_embedded_manifest(&files).unwrap().is_none());
    }

    #[test]
    fn embedded_manifest_valid_parses() {
        let files = vec![(
            "oxy-app.json".to_string(),
            br#"{"slug":"x","functions":{}}"#.to_vec(),
        )];
        assert!(parse_embedded_manifest(&files).unwrap().is_some());
    }

    #[test]
    fn unpack_accepts_a_normal_bundle_within_limits() {
        let gz = make_tar_gz(&[("index.html", b"<html>"), ("app.js", b"x")]);
        let files = unpack_tar_gz_bounded(&gz, 1_000, 100, 100).expect("within limits");
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn function_specs_extracts_declared_functions() {
        let manifest = serde_json::json!({
            "slug": "hello-oxy",
            "functions": {
                "top-stores": { "route": true, "timeoutSeconds": 15 },
            },
        });
        let specs = function_specs(Some(&manifest));
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].0, "top-stores");
        assert_eq!(specs[0].1["route"], serde_json::json!(true));
        assert_eq!(specs[0].1["timeoutSeconds"], serde_json::json!(15));
    }

    #[test]
    fn function_specs_empty_when_no_functions_block() {
        // A static-bundle manifest (today's default) records no functions.
        assert!(function_specs(Some(&serde_json::json!({ "slug": "x" }))).is_empty());
        assert!(function_specs(None).is_empty());
        // A present-but-empty block is also a no-op.
        assert!(function_specs(Some(&serde_json::json!({ "functions": {} }))).is_empty());
    }

    #[test]
    fn function_artifact_key_matches_build_store_layout() {
        assert_eq!(
            function_artifact_key("customer-apps/abc/builds/v1/", "top-stores"),
            "customer-apps/abc/builds/v1/functions/top-stores.js"
        );
    }

    #[test]
    fn missing_function_artifacts_lists_declared_functions_the_bundle_lacks() {
        let specs = function_specs(Some(&serde_json::json!({
            "functions": { "top-stores": {}, "notify": {}, "digest": {} }
        })));
        let files = vec![
            ("index.html".to_string(), b"<html>".to_vec()),
            (
                "functions/notify.js".to_string(),
                b"export default 1".to_vec(),
            ),
            // Neither a source file nor a differently-named artifact counts.
            ("functions/top-stores.ts".to_string(), b"x".to_vec()),
            ("assets/digest.js".to_string(), b"x".to_vec()),
        ];
        assert_eq!(
            missing_function_artifacts(&files, &specs),
            vec!["digest".to_string(), "top-stores".to_string()]
        );
    }

    #[test]
    fn missing_function_artifacts_is_empty_when_all_ship_or_none_are_declared() {
        let specs = function_specs(Some(&serde_json::json!({ "functions": { "notify": {} } })));
        let files = vec![
            ("index.html".to_string(), b"<html>".to_vec()),
            ("functions/notify.js".to_string(), b"x".to_vec()),
        ];
        assert!(missing_function_artifacts(&files, &specs).is_empty());
        assert!(missing_function_artifacts(&files, &function_specs(None)).is_empty());
    }

    #[test]
    fn an_invalid_function_name_without_an_artifact_gets_the_invalid_name_error() {
        // Neither name has a bundled artifact. Checked the other way round, both
        // came back as MissingFunctionArtifacts ("upgrade the CLI"), which sends
        // the publisher after the wrong problem.
        let files = vec![("index.html".to_string(), b"<html>".to_vec())];
        for name in ["Bad_Name", "../../evil"] {
            let specs = vec![(name.to_string(), serde_json::json!({}))];
            let err = check_declared_functions(&files, &specs).unwrap_err();
            assert!(
                matches!(&err, PublishError::BadTarball(m)
                    if m.contains("invalid function name") && m.contains(name)),
                "expected the invalid-name error for {name:?}, got {err:?}"
            );
        }

        // A valid name without its artifact is still reported as missing, and
        // passes once the artifact ships.
        let specs = vec![("notify".to_string(), serde_json::json!({}))];
        match check_declared_functions(&files, &specs) {
            Err(PublishError::MissingFunctionArtifacts { missing }) => {
                assert_eq!(missing, vec!["notify".to_string()]);
            }
            other => panic!("expected MissingFunctionArtifacts, got {other:?}"),
        }
        let mut shipped = files.clone();
        shipped.push(("functions/notify.js".to_string(), b"x".to_vec()));
        assert!(check_declared_functions(&shipped, &specs).is_ok());
    }

    /// The bundle is well-formed but incomplete for what it declares: 422, and
    /// the message names the functions and the remedy, because the CLI prints
    /// the raw body.
    #[test]
    fn missing_function_artifacts_is_unprocessable_and_names_the_remedy() {
        let e = PublishError::MissingFunctionArtifacts {
            missing: vec!["notify".to_string(), "top-stores".to_string()],
        };
        assert_eq!(e.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let msg = e.to_string();
        assert!(msg.contains("notify, top-stores"), "{msg}");
        assert!(msg.contains("functions/<name>.js"), "{msg}");
        assert!(
            msg.contains("0.5.96"),
            "names the CLI version to upgrade to: {msg}"
        );
    }

    #[test]
    fn a_function_declaring_a_platform_notify_kind_is_refused() {
        let specs = vec![(
            "remind".to_string(),
            serde_json::json!({ "notify": { "kinds": { "oxy.billing_past_due": {} } } }),
        )];
        let err = check_declared_functions(&[], &specs).unwrap_err();
        assert!(
            matches!(&err, PublishError::Invalid(v) if v.check == "notify_kind_reserved"),
            "expected notify_kind_reserved, got {err:?}"
        );
        assert_eq!(err.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn a_malformed_notification_catalogue_is_a_422_naming_the_check() {
        let manifest = serde_json::json!({
            "functions": { "lists": { "notify": { "kinds": { "late": {} } } } },
            "notifications": { "kinds": { "late": { "level": "loud" } } }
        });
        let err = check_notification_catalogue(Some(&manifest)).unwrap_err();
        assert!(
            matches!(&err, PublishError::Invalid(v) if v.check == "notify_level_invalid"),
            "{err:?}"
        );
        assert_eq!(err.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let manifest = serde_json::json!({ "notifications": { "kinds": { "late": {} } } });
        let warnings = check_notification_catalogue(Some(&manifest)).expect("a warning only");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }

    #[test]
    fn validate_function_names_rejects_traversal() {
        let bad = vec![("../../evil".to_string(), serde_json::json!({}))];
        let err = validate_function_names(&bad).unwrap_err();
        assert!(
            matches!(&err, PublishError::BadTarball(m) if m.contains("function name")),
            "expected a BadTarball naming the function, got {err:?}"
        );
    }

    #[test]
    fn validate_function_names_rejects_bad_shapes() {
        // Uppercase, underscore, leading hyphen, empty, and a path separator all
        // fail — these are exactly the names the sources.rs rewrite argument and
        // the artifact-key path segment must never see.
        for name in ["Notify", "send_email", "-lead", "", "a/b", "a.b"] {
            let specs = vec![(name.to_string(), serde_json::json!({}))];
            assert!(
                validate_function_names(&specs).is_err(),
                "expected {name:?} to be rejected"
            );
        }
    }

    #[test]
    fn validate_function_names_accepts_valid() {
        let ok = vec![
            ("notify".to_string(), serde_json::json!({})),
            ("top-stores".to_string(), serde_json::json!({})),
            ("f0".to_string(), serde_json::json!({})),
        ];
        assert!(validate_function_names(&ok).is_ok());
    }

    #[test]
    fn unpack_path_guard_rejects_unsafe() {
        // The tar *writer* already refuses to emit `..`, so we exercise the
        // guard predicate directly — it must reject traversal and absolute
        // paths from archives built by other tools, and allow normal ones.
        use std::path::Path;
        assert!(!is_safe_relative_path(Path::new("../escape.html")));
        assert!(!is_safe_relative_path(Path::new("a/../../b")));
        assert!(!is_safe_relative_path(Path::new("/abs/secret")));
        assert!(is_safe_relative_path(Path::new("index.html")));
        assert!(is_safe_relative_path(Path::new("assets/app.js")));
    }

    #[test]
    fn humanize_slug_titlecases_words() {
        assert_eq!(humanize_slug("weekly-sales"), "Weekly Sales");
        assert_eq!(humanize_slug("dashboard"), "Dashboard");
    }

    #[test]
    fn oxy_access_denied_is_forbidden() {
        let e = PublishError::OxyAccessDenied {
            org: "acme".to_string(),
            project: Uuid::nil(),
        };
        assert_eq!(e.status(), StatusCode::FORBIDDEN);
        assert!(e.to_string().contains("has not granted Oxy access"));
    }

    /// A malformed id is the bundle-is-fine-but-your-input-isn't case: 422,
    /// and the message has to say what the accepted shape is.
    #[test]
    fn invalid_build_id_is_unprocessable_and_states_the_rule() {
        let e = PublishError::InvalidBuildId("../../etc".to_string());
        assert_eq!(e.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let msg = e.to_string();
        assert!(msg.contains("../../etc"));
        assert!(msg.contains("letters, digits"), "{msg}");
    }

    /// A reused build id is a conflict, not a bad request — the bundle is
    /// fine, the id is taken. The message has to name the remedy, because the
    /// CLI surfaces the raw body (`sdk/cli/src/publish/server.ts`).
    #[test]
    fn duplicate_build_is_a_conflict_and_names_the_remedy() {
        let e = PublishError::DuplicateBuild {
            app_slug: "store-pulse".to_string(),
            build_id: "abc123".to_string(),
        };
        assert_eq!(e.status(), StatusCode::CONFLICT);
        let msg = e.to_string();
        assert!(msg.contains("store-pulse") && msg.contains("abc123"));
        assert!(
            msg.contains("--build-id"),
            "the error must tell the publisher how to proceed: {msg}"
        );
    }

    /// Naming another workspace conflicts with the existing row; the bundle is
    /// fine. The message names both workspaces and the deliberate way to move
    /// an app, because the CLI surfaces the raw body.
    #[test]
    fn project_mismatch_is_a_conflict_and_names_both_workspaces_and_the_move() {
        let (app_id, existing, requested) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let e = PublishError::ProjectMismatch {
            app_slug: "bookkeeping".to_string(),
            app_id,
            existing_project: existing,
            requested_project: requested,
        };
        assert_eq!(e.status(), StatusCode::CONFLICT);
        let msg = e.to_string();
        assert!(msg.contains("bookkeeping"), "{msg}");
        assert!(msg.contains(&format!("--project {existing}")), "{msg}");
        assert!(msg.contains(&requested.to_string()), "{msg}");
        assert!(
            msg.contains(&format!("PATCH /api/customer-apps/{app_id}")),
            "the error must name the admin path that moves an app: {msg}"
        );
    }
}
