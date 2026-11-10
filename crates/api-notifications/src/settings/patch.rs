//! `PATCH /api/notifications/settings`: change the person-level settings and
//! any number of nodes, in one transaction, and answer with the new document.
//!
//! ```text
//! { global?: { paused_until?, quiet_hours?, timezone? },     // null clears
//!   nodes?: [{ app_id, scope? | kind?, channels?: { push?: true|false|null, … },
//!              min_level?: Level | null }] }                  // null inherits
//! ```
//!
//! - **`If-Match: <version>`** (optional): a document changed since then is
//!   refused with `412` and the current document, nothing written.
//! - **`400 { error }`**: `invalid_body` (malformed, an unknown field, a
//!   missing `app_id`), `too_many_nodes` (more than [`MAX_NODES`]; a node
//!   named twice is written once, the later value winning), `unknown_node` (an app this person cannot be told by,
//!   a path or kind the source does not have, both or neither of
//!   `scope`/`kind`), `channel_not_used` (no kind under the node is ever
//!   delivered on that channel), `critical_only` (off where every kind under
//!   the node is critical, or never muted), `bad_level`, `bad_time`
//!   (`HH:MM`, start ≠ end, an RFC 3339 `paused_until`), `bad_days`,
//!   `bad_timezone` (not IANA), `timezone_required` (quiet hours without
//!   one).

use chrono::{DateTime, Utc};
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, TransactionTrait};
use serde::{Deserialize, Deserializer};

use super::document::{NotificationSettings, document_for};
use super::model::GlobalSettings;
use super::patch_nodes::{self, NodePatch};
use super::quiet::{QuietHours, days_mask, parse_time, parse_tz};
use super::store;
use crate::catalog::model::Level;

/// Most nodes one patch may change. A settings page changes one or a few;
/// every node is a few statements under the person's lock.
pub const MAX_NODES: usize = 200;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsPatch {
    #[serde(default)]
    pub global: Option<GlobalPatch>,
    #[serde(default)]
    pub nodes: Vec<NodePatch>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalPatch {
    #[serde(default, deserialize_with = "present")]
    pub paused_until: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    pub quiet_hours: Option<Option<QuietHoursInput>>,
    #[serde(default, deserialize_with = "present")]
    pub timezone: Option<Option<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuietHoursInput {
    pub start: String,
    pub end: String,
    pub days: Vec<u8>,
    pub allow: String,
}

/// A field that may be absent (`None`), `null` (`Some(None)`) or a value.
pub(crate) fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// Why a patch was not applied.
#[derive(Debug)]
pub enum PatchError {
    /// `400 { error }`.
    BadRequest(&'static str),
    /// `412`: the document changed since the `If-Match` version.
    Stale(Box<NotificationSettings>),
    Db(DbErr),
}

impl From<DbErr> for PatchError {
    fn from(e: DbErr) -> Self {
        PatchError::Db(e)
    }
}

/// Whether an `If-Match` value names `version`: as sent (`7`), quoted
/// (`"7"`), weak (`W/"7"`), or `*`.
pub fn matches(if_match: &str, version: i64) -> bool {
    let want = version.to_string();
    if_match.split(',').map(str::trim).any(|tag| {
        let tag = tag.strip_prefix("W/").unwrap_or(tag);
        tag == "*" || tag.trim_matches('"') == want
    })
}

/// Apply `body` for `user` and answer with the new document.
pub async fn apply(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    if_match: Option<&str>,
    body: &[u8],
) -> Result<NotificationSettings, PatchError> {
    let patch = parse(body)?;
    // Everything that reads beyond the person's own nodes happens before the
    // lock: the transaction below holds one connection, briefly.
    let changes = patch_nodes::prepare(db, user, &patch.nodes).await?;
    let txn = db.begin().await?;
    let version = store::lock(&txn, user.id).await?;
    if if_match.is_some_and(|tag| !matches(tag, version)) {
        txn.rollback().await?;
        return Err(PatchError::Stale(Box::new(document_for(db, user).await?)));
    }
    if let Some(global) = patch.global {
        apply_global(&txn, user, global).await?;
    }
    patch_nodes::write(&txn, user.id, &changes).await?;
    store::bump(&txn, user.id).await?;
    txn.commit().await?;
    Ok(document_for(db, user).await?)
}

/// The body, parsed and bounded: at most [`MAX_NODES`] nodes.
pub fn parse(body: &[u8]) -> Result<SettingsPatch, PatchError> {
    let patch: SettingsPatch =
        serde_json::from_slice(body).map_err(|_| PatchError::BadRequest("invalid_body"))?;
    if patch.nodes.len() > MAX_NODES {
        return Err(PatchError::BadRequest("too_many_nodes"));
    }
    Ok(patch)
}

async fn apply_global(
    txn: &impl ConnectionTrait,
    user: &AuthenticatedUser,
    patch: GlobalPatch,
) -> Result<(), PatchError> {
    let (current, _) = store::person(txn, user.id).await?;
    let next = merge_global(current, patch)?;
    store::write_global(txn, user.id, &next).await?;
    Ok(())
}

/// `current` with `patch` applied, checked. Pure.
pub fn merge_global(
    mut current: GlobalSettings,
    patch: GlobalPatch,
) -> Result<GlobalSettings, PatchError> {
    let bad = PatchError::BadRequest;
    if let Some(paused_until) = patch.paused_until {
        current.paused_until = paused_until
            .map(|s| DateTime::parse_from_rfc3339(&s).map(|t| t.with_timezone(&Utc)))
            .transpose()
            .map_err(|_| bad("bad_time"))?;
    }
    if let Some(timezone) = patch.timezone {
        if timezone.as_deref().is_some_and(|tz| parse_tz(tz).is_none()) {
            return Err(bad("bad_timezone"));
        }
        current.timezone = timezone;
    }
    if let Some(quiet) = patch.quiet_hours {
        current.quiet_hours = quiet.map(quiet_hours).transpose()?;
    }
    if current.quiet_hours.is_some() && current.timezone.is_none() {
        return Err(bad("timezone_required"));
    }
    Ok(current)
}

fn quiet_hours(input: QuietHoursInput) -> Result<QuietHours, PatchError> {
    let bad = PatchError::BadRequest;
    let (Some(start), Some(end)) = (parse_time(&input.start), parse_time(&input.end)) else {
        return Err(bad("bad_time"));
    };
    if start == end {
        return Err(bad("bad_time"));
    }
    let days = days_mask(&input.days)
        .filter(|mask| *mask != 0)
        .ok_or(bad("bad_days"))?;
    let allow = match Level::parse(&input.allow) {
        Some(level @ (Level::Critical | Level::TimeSensitive)) => level,
        _ => return Err(bad("bad_level")),
    };
    Ok(QuietHours {
        start,
        end,
        days,
        allow,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn global(json: serde_json::Value) -> GlobalPatch {
        serde_json::from_value(json).expect("a global patch")
    }

    fn refusal(r: Result<GlobalSettings, PatchError>) -> &'static str {
        match r {
            Err(PatchError::BadRequest(code)) => code,
            other => panic!("expected a 400, got {other:?}"),
        }
    }

    #[test]
    fn absent_null_and_a_value_are_three_different_things() {
        let p = global(serde_json::json!({ "timezone": null }));
        assert_eq!(p.timezone, Some(None));
        assert!(p.paused_until.is_none() && p.quiet_hours.is_none());
        let current = GlobalSettings {
            timezone: Some("UTC".into()),
            paused_until: Some(Utc::now()),
            ..GlobalSettings::default()
        };
        let next = merge_global(current.clone(), global(serde_json::json!({}))).unwrap();
        assert_eq!(next, current, "an empty patch changes nothing");
        let next = merge_global(current, p).unwrap();
        assert_eq!(next.timezone, None);
        assert!(
            next.paused_until.is_some(),
            "an absent field keeps its value"
        );
    }

    #[test]
    fn quiet_hours_need_a_timezone_and_well_formed_times() {
        let quiet = serde_json::json!({ "start": "22:00", "end": "07:00", "days": [5, 6],
                                        "allow": "time_sensitive" });
        assert_eq!(
            refusal(merge_global(
                GlobalSettings::default(),
                global(serde_json::json!({ "quiet_hours": quiet }))
            )),
            "timezone_required"
        );
        let ok = merge_global(
            GlobalSettings::default(),
            global(serde_json::json!({ "quiet_hours": quiet, "timezone": "Europe/Stockholm" })),
        )
        .unwrap();
        assert_eq!(ok.quiet_hours.unwrap().allow, Level::TimeSensitive);
        // Clearing the timezone under quiet hours is refused too.
        assert_eq!(
            refusal(merge_global(
                ok.clone(),
                global(serde_json::json!({ "timezone": null }))
            )),
            "timezone_required"
        );
        let with = |field: &str, value: serde_json::Value| {
            let mut q = quiet.clone();
            q[field] = value;
            merge_global(ok.clone(), global(serde_json::json!({ "quiet_hours": q })))
        };
        assert_eq!(refusal(with("start", "7:00".into())), "bad_time");
        assert_eq!(refusal(with("end", "24:00".into())), "bad_time");
        assert_eq!(
            refusal(with("end", "22:00".into())),
            "bad_time",
            "start = end"
        );
        assert_eq!(refusal(with("days", serde_json::json!([7]))), "bad_days");
        assert_eq!(refusal(with("days", serde_json::json!([]))), "bad_days");
        assert_eq!(refusal(with("allow", "active".into())), "bad_level");
        assert_eq!(
            refusal(merge_global(
                ok.clone(),
                global(serde_json::json!({ "timezone": "Mars/Olympus" }))
            )),
            "bad_timezone"
        );
        assert_eq!(
            refusal(merge_global(
                ok,
                global(serde_json::json!({ "paused_until": "tomorrow" }))
            )),
            "bad_time"
        );
    }

    #[test]
    fn a_patch_is_bounded_in_nodes() {
        let body = |n: usize| {
            let node =
                serde_json::json!({ "app_id": null, "scope": "", "channels": { "push": null } });
            serde_json::json!({ "nodes": vec![node; n] }).to_string()
        };
        assert_eq!(
            parse(body(MAX_NODES).as_bytes()).unwrap().nodes.len(),
            MAX_NODES
        );
        assert!(matches!(
            parse(body(MAX_NODES + 1).as_bytes()),
            Err(PatchError::BadRequest("too_many_nodes"))
        ));
    }

    #[test]
    fn unknown_fields_are_refused_not_ignored() {
        assert!(
            serde_json::from_value::<SettingsPatch>(serde_json::json!({ "globl": {} })).is_err()
        );
        assert!(
            serde_json::from_value::<GlobalPatch>(serde_json::json!({ "paused": null })).is_err()
        );
    }

    #[test]
    fn if_match_takes_the_version_bare_quoted_weak_or_any() {
        for tag in ["7", "\"7\"", "W/\"7\"", "*", "\"3\", \"7\""] {
            assert!(matches(tag, 7), "{tag}");
        }
        for tag in ["8", "\"70\"", ""] {
            assert!(!matches(tag, 7), "{tag}");
        }
    }
}
