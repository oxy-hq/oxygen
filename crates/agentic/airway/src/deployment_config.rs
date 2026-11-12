//! The **deployment (operational) tier**: airway's process-wide
//! [`GlobalConfig`], sourced from the singleton `airway_deployment_config` row
//! and installed exactly once per process.
//!
//! Distinct from the **policy tier** ([`crate::AirwayAdmission`], from
//! `airway_source_config`), which is per source kind, resolved per run, and
//! passed explicitly into `Source::try_from_connector_with`. This tier is
//! per *process*: airway keeps it in a `OnceLock` that `HttpConfig::default`
//! and `RetryConfig::default` read, which is what lets it reach transports
//! nobody remembered to plumb.
//!
//! **Two kinds of reader, since 0.1.24.** The seven original settings are read
//! by those two `Default` impls, so they reach a transport by being *built*.
//! `cursor_lag_floor_secs` is read by `global::resolve_cursor_lookback`, which
//! a source calls per resource while deciding how far back to pull. That is a
//! different reach — a source with no incremental cursor never asks — and it
//! is why the roster below says "every airway load" rather than "every HTTP
//! client".
//!
//! # Three properties shape everything here
//!
//! 1. **`install` is one-shot.** A second call returns `Err`, and airway is
//!    explicit that quietly keeping the first would leave a caller looking at
//!    settings that never took effect. So a change to the row needs a process
//!    restart, and the admin surface has to *say* so rather than implying a
//!    save is live.
//! 2. **Absence is not zero.** A NULL column is `None` on the matching
//!    `GlobalConfig` field, and `apply_to_http` / `apply_to_retry` are
//!    gap-fillers — `None` leaves airway's own compiled-in constant in place.
//!    There is no Oxy-side default constant in this module, deliberately: one
//!    would drift from upstream's the moment airway changed a built-in.
//! 3. **Parsing and validation are airway's, not ours.** [`DeploymentValues`]
//!    renders itself as the flat string keys airway already reads and hands
//!    them to [`GlobalConfig::from_lookup`], which parses *and* calls
//!    `GlobalConfig::validate`. Every rule — zero durations (including a zero
//!    `cursor_lag_floor_secs`, refused because `max(lag, 0)` raises nothing and
//!    so is indistinguishable from omitting the key), an empty `user_agent`, a
//!    `user_agent` that cannot become a header, a backoff factor below 1, half
//!    an mTLS identity, a TLS struct that carries settings it would drop —
//!    arrives for free and stays in one place. A second copy on the oxy side
//!    is a copy that goes stale. The *warning* above an implausibly large
//!    floor (~30 days) is upstream's too, and is deliberately not restated:
//!    it is a caution rather than a refusal, and duplicating it would double
//!    the line an operator sees.
//!
//! # Where this tier reaches
//!
//! Stated as a roster rather than a claim, the way airway states its own —
//! "has a reader" is not "reaches every client".
//!
//! [`install_once`] runs at **process boot**, from `oxy-app`'s
//! `oxy_api_airway::boot::install_deployment_tier`, wired into each of the three entry
//! points that can build a source connector (`oxy serve` — and so `oxy start`
//! — under every `OXY_ROLE`; `oxy worker`; `oxy airway`). That is what makes
//! the roster below reach everything: the tier is a *process-wide* `OnceLock`,
//! so one install covers every connector the process builds, with no
//! signature to thread and nothing to remember at a new call site.
//!
//! The `cursor_lag_floor_secs` column of the table is not "reached" but
//! "consumed": every site below installs the whole tier, and the floor is the
//! one setting that only *some* of them can act on, since only an extract asks
//! `resolve_cursor_lookback` anything.
//!
//! | site | reached | consumes the cursor floor |
//! |---|---|---|
//! | `worker::run_pipeline` — every airway load: HTTP, schedule, `oxy airway run`, automation step, backfill | yes. It also still **calls** [`install_once`] itself — see below | yes — this is the only site that extracts |
//! | `agentic_pipeline::airway_run::discover_airway_source_tables` (`POST /sources/discover`, the create-pipeline wizard's table picker) | yes, via the boot install. This one does connect to the vendor, and before the hoist it ran on airway's built-in timeout, retry and TLS — an operator's custom CA bundle worked for runs and not for the wizard, which reads as a broken source rather than a config gap | no — it lists tables, it never pulls rows against a cursor |
//! | `oxy-app`'s admin policy preview (`airway_config::preview`) | yes, via the boot install — though it needs nothing: it builds a connector only to read `contracts()` and makes no request at all | no, for the same reason it needs nothing else |
//! | any connector site added later inside an oxy-app process | yes, by construction | only if it extracts |
//! | a process with no database (`oxy run`, which falls back to no-op storage) | no, and correctly: there is no row to read, so airway's own compiled-in settings stay in force | n/a |
//!
//! One place the floor deliberately does **not** show up: [`crate::contract`]'s
//! `ResourceContract`, the per-resource projection the run UI renders. That
//! reports the *declared* contract — a vendor claim — and the floor is a
//! deployment position layered over it at extract time. Folding
//! `max(cursor_lag, floor)` in there by hand would be the second copy of an
//! upstream rule this module exists to avoid, and it would silently restate an
//! operator's setting as something the vendor said.
//!
//! **`run_pipeline` keeps its call**, and it is not a redundant second
//! install. It is the fallback for any process with no oxy-app boot seam — an
//! integration test, a future binary, an embedder of this crate — and it is
//! where a malformed row is *raised* rather than logged: boot must not refuse
//! to start a serve replica over a setting most of it never touches, so it
//! warns, and the `OnceCell` (which does not cache an `Err`) leaves the next
//! run to re-read the row and fail onto the operator's SSE stream. After a
//! successful boot install the second call short-circuits without running its
//! closure, so it logs nothing and costs one `OnceCell` read.
//!
//! Run this to find any new connector site:
//!
//! ```text
//! grep -rn 'build_source_connector\|discover_source_tables' crates/ --include='*.rs'
//! ```
//!
//! # Why a hand-written `SELECT`
//!
//! `crates/agentic/airway/CLAUDE.md` bars this crate from depending on
//! `entity`. The row's read-side type therefore lives in `entity` for the
//! admin API and is *not* reachable here, so [`load`] issues its own query
//! over [`COLUMNS`]. Those column names are airway's own key spellings, and
//! the two lists are pinned against each other by
//! `entity_columns_match_the_airway_key_roster` in `oxy-app`, which can see
//! both crates.

use std::time::Duration;

use airway::config::global::{self, GlobalConfig};
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
use serde::{Deserialize, Serialize};
use tokio::sync::OnceCell;

use crate::error::AirwayError;

/// The singleton row's table and id. There is exactly one row and its name is
/// `1` — see the migration for how the schema holds that.
pub const TABLE: &str = "airway_deployment_config";
/// The singleton row's primary key.
pub const SINGLETON_ID: i16 = 1;

/// The eight settings, as the eleven columns that carry them — spelled with
/// airway's own key names so the unit is never in doubt at any layer.
///
/// Read by [`load`]'s `SELECT` and by the entity-drift test. `tls` is one
/// setting over the four `tls_*` columns. `environment` and `contract_policy`
/// are absent on purpose: they are the policy tier, resolved per source kind
/// per run, and installing them process-wide would silently outrank the
/// per-run admission oxy already passes explicitly.
///
/// In airway's own `GlobalConfig::KEYS` order, minus those two — so the
/// per-setting drift report reads in the same order the operator meets the
/// keys upstream.
pub const COLUMNS: &[&str] = &[
    global::TIMEOUT_SECS,
    global::MAX_RETRIES,
    global::USER_AGENT,
    global::RETRY_INITIAL_DELAY_MS,
    global::RETRY_MAX_DELAY_SECS,
    global::RETRY_BACKOFF_FACTOR,
    global::CURSOR_LAG_FLOOR_SECS,
    global::TLS_CA_CERT,
    global::TLS_CLIENT_CERT,
    global::TLS_CLIENT_KEY_FILE,
    global::TLS_DANGER_ACCEPT_INVALID_CERTS,
];

/// The deployment tier as stored and as displayed.
///
/// **Every field is `Option`, and `None` means "airway's built-in default"** —
/// not zero, not disabled, not "off". This type is the wire shape of both the
/// configured row and the installed process state on `/admin/airway`, which is
/// why the two can be compared field by field at all.
///
/// Durations are named for their unit and stored in it: `timeout_secs` and
/// `retry_max_delay_secs` in whole seconds, `retry_initial_delay_ms` in
/// milliseconds. That is airway's spelling, carried unchanged through the
/// column name, this struct, the JSON, and the admin field label.
///
/// # Rolling-deploy skew on a newly added field
///
/// A new setting means a new column, and for the length of a rollout the
/// migration has already run while processes on the previous image are still
/// serving `PUT /api/admin/airway/deployment-config` with a `DeploymentValues`
/// that has no such field. Two versions of that hazard, and they land
/// differently — worth separating, because only one of them is a bug:
///
/// * **An older *process* writing the row cannot clear the new column.** The
///   write is column-scoped, not a full-row overwrite:
///   `deployment::upsert_deployment` enumerates its
///   `OnConflict::update_columns`, so a build touches exactly the columns it
///   knows about, and SeaORM's `INSERT` carries only the `Set` members of the
///   `ActiveModel` (and fires only when there is no row, hence no configured
///   value to lose). An old replica's save leaves `cursor_lag_floor_secs`
///   exactly as it found it. Pinned by
///   `an_older_writer_cannot_clear_the_cursor_lag_floor`, which drives the raw
///   statement a pre-#111 build would emit. Note this is a property of that one
///   call site, not of the type: a refactor to `Entity::update` — SeaORM writes
///   every column in the model — would reintroduce it for the *next* field
///   added here, which is what that test exists to catch.
/// * **An older *client* body against a current process does clear it, by
///   design.** The endpoint is a replace, not a patch, and `#[serde(default)]`
///   below is what makes that work: a missing key deserializes to `None`, which
///   is how the UI clears a setting back to airway's built-in default. A
///   browser tab holding a pre-deploy bundle posts the ten fields it knows and
///   so clears the eleventh. That is the same behaviour as clearing any other
///   field, it is visible on the page immediately after (the read renders
///   configured beside installed), and it changes nothing until a process
///   restarts. Narrowing it would mean making omission and explicit-null
///   different — the distinction this surface deliberately refuses everywhere
///   else, since it is what lets `null` mean "airway's default" and nothing
///   else.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeploymentValues {
    pub timeout_secs: Option<u64>,
    /// One count for `HttpConfig` **and** `RetryConfig` — airway unifies them
    /// on purpose; there is no separate retry-layer count to configure.
    pub max_retries: Option<u32>,
    pub user_agent: Option<String>,
    pub retry_initial_delay_ms: Option<u64>,
    pub retry_max_delay_secs: Option<u64>,
    pub retry_backoff_factor: Option<f64>,
    /// A **floor** under every resource's declared `cursor_lag`, in whole
    /// seconds — never a ceiling. `None` is *no floor*, and it is the only way
    /// to say that: airway refuses a zero, since `max(lag, 0)` raises nothing
    /// and a stored `0` would read as a deployment position that changes
    /// nothing.
    pub cursor_lag_floor_secs: Option<u64>,
    pub tls_ca_cert: Option<String>,
    pub tls_client_cert: Option<String>,
    pub tls_client_key_file: Option<String>,
    pub tls_danger_accept_invalid_certs: Option<bool>,
}

impl DeploymentValues {
    /// Render one key the way airway's config loader would have read it.
    ///
    /// `None` for a key this tier does not carry, which is how the two policy
    /// keys stay out: [`GlobalConfig::from_lookup`] asks for `environment` and
    /// `contract_policy` too, and answering them here would install a
    /// process-wide policy that outranks nothing visibly and confuses the
    /// per-run one.
    fn lookup(&self, key: &str) -> Option<String> {
        match key {
            global::TIMEOUT_SECS => self.timeout_secs.map(|v| v.to_string()),
            global::MAX_RETRIES => self.max_retries.map(|v| v.to_string()),
            global::USER_AGENT => self.user_agent.clone(),
            global::RETRY_INITIAL_DELAY_MS => self.retry_initial_delay_ms.map(|v| v.to_string()),
            global::RETRY_MAX_DELAY_SECS => self.retry_max_delay_secs.map(|v| v.to_string()),
            global::RETRY_BACKOFF_FACTOR => self.retry_backoff_factor.map(|v| v.to_string()),
            global::CURSOR_LAG_FLOOR_SECS => self.cursor_lag_floor_secs.map(|v| v.to_string()),
            global::TLS_CA_CERT => self.tls_ca_cert.clone(),
            global::TLS_CLIENT_CERT => self.tls_client_cert.clone(),
            global::TLS_CLIENT_KEY_FILE => self.tls_client_key_file.clone(),
            global::TLS_DANGER_ACCEPT_INVALID_CERTS => {
                self.tls_danger_accept_invalid_certs.map(|v| v.to_string())
            }
            _ => None,
        }
    }

    /// Parse and validate into airway's own type.
    ///
    /// **Every rule this feature enforces is enforced here, by airway.** The
    /// oxy side owns no validation of its own — see the module doc. An
    /// operator mistake comes back as an `Err` naming the key (and never the
    /// value), which the admin write path turns into a `400`.
    pub fn to_global(&self) -> Result<GlobalConfig, AirwayError> {
        GlobalConfig::from_lookup(&|key| self.lookup(key)).map_err(AirwayError::Engine)
    }

    /// Read back off an airway [`GlobalConfig`] — the installed process state,
    /// or the normalised form of a configured row.
    ///
    /// The `tls` handling is the part worth reading: airway collapses the four
    /// `tls_*` keys into one `Option<TlsConfig>` and decides presence from
    /// *what the values say*, so `tls_danger_accept_invalid_certs = false`
    /// alone yields `None`. Reading back through this function on both sides
    /// of a comparison is what keeps that from surfacing as phantom drift.
    pub fn from_global(config: &GlobalConfig) -> Self {
        Self {
            timeout_secs: config.timeout.map(|d| d.as_secs()),
            max_retries: config.max_retries,
            user_agent: config.user_agent.clone(),
            retry_initial_delay_ms: config
                .retry_initial_delay
                .map(|d| duration_as_millis_u64(d)),
            retry_max_delay_secs: config.retry_max_delay.map(|d| d.as_secs()),
            retry_backoff_factor: config.retry_backoff_factor,
            // Seconds, matching the key's `_secs` suffix. Lossless in
            // practice and by construction on the round trip: the only way a
            // value reaches `GlobalConfig::cursor_lag_floor` on this path is
            // `Duration::from_secs` in airway's own `from_lookup`, so there is
            // no sub-second remainder to truncate.
            cursor_lag_floor_secs: config.cursor_lag_floor.map(|d| d.as_secs()),
            tls_ca_cert: config.tls.as_ref().and_then(|t| t.ca_cert.clone()),
            tls_client_cert: config.tls.as_ref().and_then(|t| t.client_cert.clone()),
            tls_client_key_file: config.tls.as_ref().and_then(|t| t.client_key_file.clone()),
            tls_danger_accept_invalid_certs: config
                .tls
                .as_ref()
                .map(|t| t.danger_accept_invalid_certs),
        }
    }

    /// What airway would actually install for these values.
    ///
    /// The drift comparison runs on this form, not on the raw row, because the
    /// raw row has spellings airway normalises away — see [`Self::from_global`].
    pub fn effective(&self) -> Result<Self, AirwayError> {
        Ok(Self::from_global(&self.to_global()?))
    }
}

/// `Duration::as_millis` is `u128`; every value that reaches here was built
/// from a `u64` count of milliseconds, so the saturating cast cannot lose a
/// configured value. Written as a named function rather than a bare `as` so
/// the reason is attached to the cast.
fn duration_as_millis_u64(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Which settings the running process disagrees with the database about.
///
/// Both sides must already be in [`DeploymentValues::effective`] form. Returns
/// airway's key names, in [`COLUMNS`] order; empty means the process is
/// running exactly what is configured.
///
/// **`None` on both sides is agreement, not absence of information** — both
/// mean "airway's built-in default", which is a perfectly definite state. A
/// `None` against a `Some` is real drift in either direction: one of them
/// took the built-in and the other did not.
///
/// The exhaustive destructure is load-bearing: a new field added to
/// [`DeploymentValues`] fails to compile here until someone decides whether it
/// drifts, rather than being silently excluded from the comparison.
pub fn drift(configured: &DeploymentValues, installed: &DeploymentValues) -> Vec<&'static str> {
    let DeploymentValues {
        timeout_secs,
        max_retries,
        user_agent,
        retry_initial_delay_ms,
        retry_max_delay_secs,
        retry_backoff_factor,
        cursor_lag_floor_secs,
        tls_ca_cert,
        tls_client_cert,
        tls_client_key_file,
        tls_danger_accept_invalid_certs,
    } = configured;

    let mut drifted = Vec::new();
    let mut check = |differs: bool, key: &'static str| {
        if differs {
            drifted.push(key);
        }
    };
    check(
        *timeout_secs != installed.timeout_secs,
        global::TIMEOUT_SECS,
    );
    check(*max_retries != installed.max_retries, global::MAX_RETRIES);
    check(*user_agent != installed.user_agent, global::USER_AGENT);
    check(
        *retry_initial_delay_ms != installed.retry_initial_delay_ms,
        global::RETRY_INITIAL_DELAY_MS,
    );
    check(
        *retry_max_delay_secs != installed.retry_max_delay_secs,
        global::RETRY_MAX_DELAY_SECS,
    );
    check(
        *retry_backoff_factor != installed.retry_backoff_factor,
        global::RETRY_BACKOFF_FACTOR,
    );
    check(
        *cursor_lag_floor_secs != installed.cursor_lag_floor_secs,
        global::CURSOR_LAG_FLOOR_SECS,
    );
    check(*tls_ca_cert != installed.tls_ca_cert, global::TLS_CA_CERT);
    check(
        *tls_client_cert != installed.tls_client_cert,
        global::TLS_CLIENT_CERT,
    );
    check(
        *tls_client_key_file != installed.tls_client_key_file,
        global::TLS_CLIENT_KEY_FILE,
    );
    check(
        *tls_danger_accept_invalid_certs != installed.tls_danger_accept_invalid_certs,
        global::TLS_DANGER_ACCEPT_INVALID_CERTS,
    );
    drifted
}

/// The tier **this process** installed, if it installed one.
///
/// `None` means this process never resolved the tier — either it has no
/// oxy-app boot seam (a test, an embedder) or its boot install failed and only
/// logged. A caller presenting this to an operator must label it as this
/// process's state; it is not the deployment's, and on a rolling deploy the
/// replicas need not agree. See
/// `crates/api-airway/src/admin/deployment.rs`.
pub fn installed_values() -> Option<DeploymentValues> {
    global::installed().map(DeploymentValues::from_global)
}

/// Read the singleton row. `Ok(None)` = no row, i.e. airway's defaults.
///
/// A hand-written `SELECT` because this crate may not depend on `entity`; see
/// the module doc.
pub async fn load(db: &DatabaseConnection) -> Result<Option<DeploymentValues>, AirwayError> {
    let sql = format!(
        "SELECT {} FROM {TABLE} WHERE id = {SINGLETON_ID}",
        COLUMNS.join(", ")
    );
    let backend = db.get_database_backend();
    let row = db
        .query_one_raw(Statement::from_string(backend, sql))
        .await
        .map_err(|e| AirwayError::Other(format!("reading {TABLE}: {e}")))?;
    let Some(row) = row else {
        return Ok(None);
    };

    // Postgres has no unsigned integer, so the stored width is signed and the
    // conversion can fail on a row written outside the API (the schema's
    // `>= 0` CHECKs make that a hand-rolled `UPDATE`). Named rather than
    // silently clamped: a negative here is a broken row, not a default.
    let get_u64 = |key: &'static str| -> Result<Option<u64>, AirwayError> {
        let raw: Option<i64> = row
            .try_get("", key)
            .map_err(|e| AirwayError::Other(format!("{TABLE}.{key}: {e}")))?;
        raw.map(|v| {
            u64::try_from(v).map_err(|_| {
                AirwayError::Other(format!("{TABLE}.`{key}` is negative, which is not a value"))
            })
        })
        .transpose()
    };
    let get_string = |key: &'static str| -> Result<Option<String>, AirwayError> {
        row.try_get("", key)
            .map_err(|e| AirwayError::Other(format!("{TABLE}.{key}: {e}")))
    };
    let max_retries: Option<i32> = row
        .try_get("", global::MAX_RETRIES)
        .map_err(|e| AirwayError::Other(format!("{TABLE}.max_retries: {e}")))?;
    let max_retries = max_retries
        .map(|v| {
            u32::try_from(v).map_err(|_| {
                AirwayError::Other(format!(
                    "{TABLE}.`{}` is negative, which is not a value",
                    global::MAX_RETRIES
                ))
            })
        })
        .transpose()?;

    Ok(Some(DeploymentValues {
        timeout_secs: get_u64(global::TIMEOUT_SECS)?,
        max_retries,
        user_agent: get_string(global::USER_AGENT)?,
        retry_initial_delay_ms: get_u64(global::RETRY_INITIAL_DELAY_MS)?,
        retry_max_delay_secs: get_u64(global::RETRY_MAX_DELAY_SECS)?,
        retry_backoff_factor: row
            .try_get("", global::RETRY_BACKOFF_FACTOR)
            .map_err(|e| AirwayError::Other(format!("{TABLE}.retry_backoff_factor: {e}")))?,
        cursor_lag_floor_secs: get_u64(global::CURSOR_LAG_FLOOR_SECS)?,
        tls_ca_cert: get_string(global::TLS_CA_CERT)?,
        tls_client_cert: get_string(global::TLS_CLIENT_CERT)?,
        tls_client_key_file: get_string(global::TLS_CLIENT_KEY_FILE)?,
        tls_danger_accept_invalid_certs: row
            .try_get("", global::TLS_DANGER_ACCEPT_INVALID_CERTS)
            .map_err(|e| {
            AirwayError::Other(format!("{TABLE}.tls_danger_accept_invalid_certs: {e}"))
        })?,
    }))
}

/// Guards the one-shot install so repeated runs in one process read the row
/// once and call airway's `install` once. `get_or_try_init` does **not** cache
/// an `Err`, which is what we want: a broken row fixed between runs is picked
/// up by the next run rather than poisoning the process.
static INSTALLED: OnceCell<()> = OnceCell::const_new();

/// Resolve the deployment tier from the database and install it for this
/// process — **once**, however many runs this process drains.
///
/// Called at **process boot** by `oxy-api-airway`'s `boot`, and again from
/// `run_pipeline` immediately before the first source is built — the second
/// call is the fallback for a process with no oxy-app boot seam, and a no-op
/// once boot has succeeded. See the module doc for the split, and `worker.rs`
/// for why `run_pipeline` is that crate's seam and `AirwayWorker::new` is not.
///
/// # No row is still an install
///
/// An absent row installs an all-`None` [`GlobalConfig`], which behaves
/// identically to not installing at all (every `apply_to_*` is a gap-filler).
/// It is done anyway so that `installed()` returning `Some` is a reliable
/// signal that *this process has resolved its tier* — otherwise the admin
/// surface could not tell "running airway's defaults" from "this replica never
/// resolved the row at all", and those need different words.
///
/// # A malformed row is an `Err`, and the caller decides how loud that is
///
/// Rather than falling back to defaults. That is airway's own rule for this
/// tier and it is the right one: a silent fallback means the operator sets a
/// timeout, sees the old one, and has nothing to read. The write path
/// validates through the same [`DeploymentValues::to_global`], so a row that
/// fails here was written around the API.
///
/// The two callers make different, deliberate choices with that `Err`: boot
/// logs it and starts anyway (a serve replica must not refuse to come up over
/// a setting most of it never touches), `run_pipeline` fails the run with it
/// (where it reaches the operator on the SSE stream). Both are reachable
/// because `get_or_try_init` does **not** cache an `Err` — a row still broken
/// at run time is re-read and re-raised, and one fixed between boot and the
/// run is simply picked up.
///
/// # A second installer is reported, not raised
///
/// `validate` runs first, so a failure from airway's `install` after that can
/// only mean something else in this process installed first. That is not the
/// operator's mistake and must not fail their run — but it does mean these
/// values are not in force, so it is logged at `warn` with exactly that
/// wording.
pub async fn install_once(db: &DatabaseConnection) -> Result<(), AirwayError> {
    INSTALLED
        .get_or_try_init(|| async {
            let values = load(db).await?.unwrap_or_default();
            let config = values.to_global()?;
            // Belt and braces, for the reason airway gives at its own
            // `install_from_environment`: it makes "the install was refused"
            // mean exactly one thing below.
            config.validate().map_err(AirwayError::Engine)?;
            if global::install(config).is_err() {
                tracing::warn!(
                    "airway deployment config was already installed in this process — the \
                     `{TABLE}` values did NOT take effect and will not until a restart"
                );
            } else {
                tracing::info!(
                    configured = values != DeploymentValues::default(),
                    "installed airway deployment config for this process"
                );
            }
            Ok(())
        })
        .await
        .map(|_: &()| ())
}

#[cfg(test)]
#[path = "deployment_config_tests.rs"]
mod tests;
