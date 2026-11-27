//! `humantime` (de)serialization for `Option<Duration>`: the `3d` / `30m` wire
//! format, for `#[serde(with = "oxy_shared::duration_opt")]`.
//!
//! `.monitor.yml` (`oxy-metric-monitoring`) and `reconcile.yml`
//! (`oxy-workspace-health`) both write `freshness` this way and must parse the
//! same strings the same way; two copies would drift, so the one copy lives
//! here, below both.
//!
//! Note humantime's `m` is minutes and `M` is months, and its month is an
//! average 30.44 days: express anything above a week in days or weeks.

use serde::{Deserialize, Deserializer, Serializer};
use std::time::Duration;

pub fn deserialize<'de, D>(d: D) -> Result<Option<Duration>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<String>::deserialize(d)? {
        None => Ok(None),
        Some(s) => humantime::parse_duration(&s)
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}

pub fn serialize<S>(v: &Option<Duration>, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match v {
        None => s.serialize_none(),
        Some(d) => s.serialize_str(&humantime::format_duration(*d).to_string()),
    }
}
