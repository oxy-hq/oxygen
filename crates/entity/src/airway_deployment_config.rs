use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Airway's deployment-wide **operational tier** — the eight
/// `airway::config::global::GlobalConfig` settings that are installed once per
/// process, as opposed to the policy tier (`airway_source_config`) that is
/// resolved per source kind on every run.
///
/// # Singleton
///
/// One row, id `1`. Enforced in the schema by
/// `id SMALLINT PRIMARY KEY DEFAULT 1 CHECK (id = 1)`, not by convention here
/// — see `migration::m20260807_000001_airway_deployment_config`. Read it with
/// [`Entity::find_by_id`]`(1)` and write it with an upsert on `id`; nothing
/// should ever `find().all()` this table.
///
/// # NULL means airway's default, never zero
///
/// Every setting is `Option`, and `None` maps to `None` on the matching
/// `GlobalConfig` field, which `apply_to_http` / `apply_to_retry` treat as
/// "leave the compiled-in value alone". A stored `0` is a value the operator
/// chose, and airway rejects it for all four durations. Do not `unwrap_or(0)`
/// anything here, and do not introduce an Oxy-side default constant — that
/// would silently diverge from upstream's the first time airway changed a
/// built-in.
///
/// # `tls` is one setting over four columns
///
/// `GlobalConfig::tls` is an `Option<TlsConfig>`, and airway spells its inputs
/// as four flat keys. The mapping in
/// `agentic_airway::deployment_config::DeploymentValues` is what decides
/// whether the four columns amount to a configured trust store at all
/// (`TlsConfig::carries_settings`) — notably, `tls_danger_accept_invalid_certs
/// = false` alone does not. `tls_server_name` / `tls_enabled` have no column
/// because airway offers no key for them.
///
/// # Column names are airway's key spellings
///
/// Including the unit suffix (`_secs`, `_ms`). `agentic-airway` may not depend
/// on this crate (see `crates/agentic/airway/CLAUDE.md`), so it reads the
/// table through a hand-written `SELECT` over
/// `agentic_airway::deployment_config::COLUMNS`; the two lists are pinned
/// against each other by `entity_columns_match_the_airway_key_roster` in
/// `crates/api-airway/src/admin/deployment_tests.rs`.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "airway_deployment_config")]
pub struct Model {
    /// Always `1`. `auto_increment = false` because the value is fixed by the
    /// singleton `CHECK`, not allocated by a sequence.
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i16,
    /// `GlobalConfig::timeout`, in **whole seconds** (airway's `timeout_secs`).
    pub timeout_secs: Option<i64>,
    /// `GlobalConfig::max_retries`. One count for both `HttpConfig` and
    /// `RetryConfig` — airway unifies them deliberately.
    pub max_retries: Option<i32>,
    /// `GlobalConfig::user_agent`. Empty string is rejected on write (airway
    /// refuses it rather than reading it as unset); clear the column instead.
    pub user_agent: Option<String>,
    /// `GlobalConfig::retry_initial_delay`, in **milliseconds**.
    pub retry_initial_delay_ms: Option<i64>,
    /// `GlobalConfig::retry_max_delay`, in **whole seconds**.
    pub retry_max_delay_secs: Option<i64>,
    /// `GlobalConfig::retry_backoff_factor`. Must be finite and >= 1.
    pub retry_backoff_factor: Option<f64>,
    /// `GlobalConfig::cursor_lag_floor`, in **whole seconds** — a floor under
    /// every resource's declared `cursor_lag`, never a ceiling.
    ///
    /// `None` means *no floor*, and it is the only spelling for that: airway
    /// **rejects** a stored `0` rather than reading it as absence, because
    /// `max(lag, 0)` raises nothing and would be a setting that does not
    /// settle anything. The write path validates through the same
    /// `GlobalConfig::validate`, so a `0` is a `400`, not a row.
    pub cursor_lag_floor_secs: Option<i64>,
    /// `TlsConfig::ca_cert` — a path on the airway process's filesystem.
    pub tls_ca_cert: Option<String>,
    /// `TlsConfig::client_cert`. Must be set together with
    /// [`Model::tls_client_key_file`] or airway refuses half an mTLS identity.
    pub tls_client_cert: Option<String>,
    /// `TlsConfig::client_key_file`. Named for what it holds (a path) so it is
    /// not classified as a credential upstream.
    pub tls_client_key_file: Option<String>,
    /// `TlsConfig::danger_accept_invalid_certs`. `Some(false)` on its own does
    /// **not** configure a trust store — see the type-level note about `tls`.
    pub tls_danger_accept_invalid_certs: Option<bool>,
    pub updated_at: DateTimeWithTimeZone,
}

impl ActiveModelBehavior for ActiveModel {}
