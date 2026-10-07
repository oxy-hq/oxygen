use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// One row per successful publish of a custom app. The bundle's files
/// live in S3 under `s3_prefix`; `apps.draft_build_id` /
/// `apps.published_build_id` point at the build currently serving each
/// channel. Keeping every build (bounded by a keep-last-N GC) is what
/// makes one-click rollback cheap.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "app_builds")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub app_id: Uuid,
    /// Engineer-facing version string (git sha or CI run id). Unique per
    /// app via `(app_id, build_id)`.
    pub build_id: String,
    /// S3 prefix holding this build's files:
    /// `customer-apps/<app_id>/builds/<build_id>/`.
    pub s3_prefix: String,
    /// Optional build/runtime manifest captured at publish time
    /// (`oxy-app.json`). Drives the future `artifact_type` serve branch.
    pub manifest_json: Option<Json>,
    pub created_at: DateTimeWithTimeZone,
    /// User (app-admin) who ran the publish. NULL for builds created before
    /// this column existed, and for a trusted-publishing (OIDC) build, which no
    /// user published — see `published_via`. Powers the "who deployed" audit.
    pub published_by: Option<Uuid>,
    /// The verified machine identity that published this build via trusted
    /// publishing (GitHub OIDC), e.g.
    /// `github-oidc:acme/app/.github/workflows/oxy-publish.yml@refs/heads/main env=production`.
    /// NULL for a user's publish. At most one of this and `published_by` is set.
    pub published_via: Option<String>,
    /// Git remote URL of the app's source at publish time (raw, e.g.
    /// `git@github.com:org/repo.git` or `https://github.com/org/repo`).
    /// Captured best-effort by `oxyc publish`; NULL for non-git / legacy builds.
    pub source_repo: Option<String>,
    /// Commit sha the build was published from.
    pub commit_sha: Option<String>,
    /// Branch the build was published from.
    pub source_branch: Option<String>,
    /// Recorded bundle-validation outcome: `passed` | `pending` | `failed`.
    /// Promotion to live is gated on `passed` (the validator-can't-be-bypassed
    /// invariant). Gate 1 (fast byte-level checks at publish) stamps `passed`;
    /// a deeper deploy-time render probe (gate 2 — tracked follow-up) may
    /// downgrade to `failed`. Defaults to `passed` for builds predating the
    /// column (they are already serving).
    pub validation_status: String,
    /// Human-readable reason when `validation_status = failed`.
    pub validation_detail: Option<String>,
    /// The compiled semantic revision this build's STAGING requests read
    /// (`oxyc publish --semantic-branch`). NULL = no pin. Only honoured on a
    /// staging request for the draft build; the live channel always reads
    /// `workspaces.current_revision_id`. Retention never prunes a revision a
    /// build references.
    pub semantic_revision_id: Option<Uuid>,
    /// The **sandbox agent token** (`api_tokens.id`) that published this build
    /// — to a sandbox of its own, or as a draft to staging. NULL for every
    /// build a person or a CI job published. No foreign key: a dangling id
    /// still says a token published it. Never updated after the insert. While
    /// it is set, production never falls back to the build and no promote or
    /// rollback ships it (`custom_apps_agent_built`).
    pub published_token_id: Option<Uuid>,
    #[sea_orm(
        belongs_to,
        from = "app_id",
        to = "id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    #[serde(skip)]
    pub apps: BelongsTo<super::apps::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}
