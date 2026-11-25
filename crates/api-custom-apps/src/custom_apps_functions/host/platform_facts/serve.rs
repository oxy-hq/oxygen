//! What the host op (`host.rs` `platform_facts`) asks and answers: its two
//! gates, the requested span, the build behind the cache, and the one message
//! a failure may carry into the isolate.
//!
//! This module is the one reader of the deployment's internal orgs
//! (`oxy_app_core::internal_orgs`): the gate hands the list it checked on to
//! the build, so nothing beside it reads the environment
//! (`crates/guards/tests/integration/authz_boundaries.rs` bans the call elsewhere).

use std::sync::OnceLock;
use std::time::Instant;

use chrono::{DateTime, Utc};
use oxy_app_core::internal_orgs::{self, INTERNAL_ORGS_ENV};
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::cache::{TTL, TtlCache};
use super::{DEFAULT_WEEKS, MAX_WEEKS, build_platform_facts, clamp_weeks, facts_enabled_for};

/// What a function sees when the build fails. Fixed text: the database's own
/// error can name tables, columns and values, and none of that is the app's to
/// read. The detail goes to the log.
pub const UNAVAILABLE: &str = "PlatformFactsUnavailable: could not read platform facts";

/// What a function sees when `opts.weeks` is not a number: the app's own
/// argument error, so its label is on the caller-error list
/// (`host_call_attrs::CALLER_ERROR_LABELS`) and it never pages.
pub const BAD_WEEKS: &str = "PlatformFactsBadRequest: `weeks` must be a number";

/// What a function without the `platform.facts` capability sees: the first
/// of the two gates, asked by the broker from the op's registry row
/// (`FunctionCapabilities::refuse`).
pub const CAPABILITY_MISSING: &str = "PlatformCapabilityMissing: this function has not declared the `platform.facts` \
     capability (add \"platform\": { \"facts\": true } to its oxy-app.json entry)";

/// The gates against this deployment's internal orgs ([`platform_facts_gate`]),
/// and on a pass the list itself, which the build takes as its staff.
pub fn admit(granted: bool, org_id: Uuid) -> Result<&'static [Uuid], String> {
    let internal = internal_orgs::internal_orgs();
    platform_facts_gate(granted, org_id, internal)?;
    Ok(internal)
}

/// The second gate alone — the host's op asks it once the broker has asked
/// the first: `org_id` among the deployment's internal orgs, and on a pass
/// the list itself.
pub fn admit_org(org_id: Uuid) -> Result<&'static [Uuid], String> {
    admit(true, org_id)
}

/// The two fail-closed gates in front of `ctx.platform.facts()`, in order:
/// the manifest's `platform.facts` capability, then `org_id` (the host's)
/// among `internal`, the deployment's internal orgs (`OXY_INTERNAL_ORGS`).
/// The capability is asked first, so an app in a customer org learns nothing
/// about the list from a manifest that never asked. Both labels are a
/// contract: the internal app that reads the facts tells "not declared" from
/// "not enabled here" by them.
pub fn platform_facts_gate(granted: bool, org_id: Uuid, internal: &[Uuid]) -> Result<(), String> {
    if !granted {
        return Err(CAPABILITY_MISSING.to_string());
    }
    if !facts_enabled_for(org_id, internal) {
        return Err(format!(
            "PlatformFactsNotEnabled: platform facts are not allowed for this app's org, which \
             is not an internal org on this deployment (its id is not in {INTERNAL_ORGS_ENV})"
        ));
    }
    Ok(())
}

/// `opts.weeks`, held to `1..=MAX_WEEKS`; [`DEFAULT_WEEKS`] when absent or
/// `null`. A fraction is cut to the whole week below it. Anything else is
/// [`BAD_WEEKS`].
pub fn requested_weeks(opts: &serde_json::Value) -> Result<u32, String> {
    match opts.get("weeks") {
        None | Some(serde_json::Value::Null) => Ok(DEFAULT_WEEKS),
        Some(weeks) => weeks
            .as_f64()
            .filter(|w| w.is_finite())
            .map(|w| clamp_weeks(w.clamp(0.0, f64::from(MAX_WEEKS)) as u32))
            .ok_or_else(|| BAD_WEEKS.to_string()),
    }
}

/// The facts as the isolate receives them, built now. Any failure is
/// [`UNAVAILABLE`]; what actually failed is logged.
pub async fn load_reply(
    db: &DatabaseConnection,
    internal: &[Uuid],
    weeks: u32,
    now: DateTime<Utc>,
) -> Result<serde_json::Value, String> {
    let facts = build_platform_facts(db, internal, weeks, now)
        .await
        .map_err(|e| unavailable(&e))?;
    serde_json::to_value(facts).map_err(|e| unavailable(&e))
}

/// [`load_reply`] behind the process-wide cache, one entry per list of
/// internal orgs and span. The list is the same on every call in production
/// (it is the environment's); keying on it keeps a stored answer from ever
/// reaching a caller whose list marks different orgs `internal`.
pub async fn cached_reply(
    db: &DatabaseConnection,
    internal: &[Uuid],
    weeks: u32,
) -> Result<serde_json::Value, String> {
    static CACHE: OnceLock<TtlCache<(Vec<Uuid>, u32), serde_json::Value>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| TtlCache::new(TTL));
    let key = (internal.to_vec(), weeks);
    let reply = cache
        .get_or_load(key, Instant::now(), || {
            load_reply(db, internal, weeks, Utc::now())
        })
        .await?;
    Ok((*reply).clone())
}

fn unavailable(detail: &dyn std::fmt::Display) -> String {
    tracing::warn!(error = %detail, "ctx.platform.facts: could not read platform facts");
    UNAVAILABLE.to_string()
}
