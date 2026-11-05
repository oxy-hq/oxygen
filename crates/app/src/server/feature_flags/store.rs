//! Sea-ORM helpers for the `feature_flags` table, and the mapping between a row and
//! a flag's kill switch + rules.
//!
//! A row with no `targeting` is a plain global flag: `enabled` is its value. A row
//! with `targeting` keeps its kill switch inside that document and writes `enabled`
//! as the registry default, so a binary that predates targeting (one revert back)
//! reads the flag as untouched rather than on for every org.
//!
//! That older binary can still flip the column, which it believes is the switch. The
//! document records the `updated_at` it was written with (`written_at`); a row updated
//! after that was written by a binary that cannot see the rules, so its column is the
//! kill switch an admin chose last, and wins on roll-forward. Time, not value: an
//! older binary that turns a flag on and then off again leaves the column as it was.

use chrono::{DateTime, FixedOffset, Utc};
use entity::feature_flag::{ActiveModel, Column, Entity, Model};
use sea_orm::ActiveValue::Set;
use sea_orm::sea_query::OnConflict;
use sea_orm::{DatabaseConnection, DbErr, EntityTrait};
use serde::{Deserialize, Serialize};

use super::registry::FlagDef;
use super::targeting::Targeting;

/// The `targeting` column's document: the rules plus the kill switch.
#[derive(Serialize, Deserialize)]
struct StoredTargeting {
    enabled: bool,
    /// The row's `updated_at` as this document was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    written_at: Option<DateTime<FixedOffset>>,
    #[serde(flatten)]
    targeting: Targeting,
}

/// A row read back as what an admin set.
pub struct Decoded {
    pub enabled: bool,
    /// `Err` is the reason the stored rules are unreadable.
    pub targeting: Result<Option<Targeting>, String>,
}

pub fn decode(row: &Model) -> Decoded {
    let Some(json) = &row.targeting else {
        return Decoded {
            enabled: row.enabled,
            targeting: Ok(None),
        };
    };
    match serde_json::from_value::<StoredTargeting>(json.clone()) {
        Ok(stored) => Decoded {
            enabled: kill_switch(row, &stored),
            targeting: Ok(Some(stored.targeting)),
        },
        // Unreadable rules serve off (the cache marks them invalid), so the switch
        // reads off too: what the admin page shows, and what a PATCH that clears the
        // rules without naming `enabled` keeps. Never on for every org by accident.
        Err(e) => Decoded {
            enabled: false,
            targeting: Err(format!("stored targeting is unreadable: {e}")),
        },
    }
}

/// The document's switch, unless a binary that predates targeting has written the row
/// since the document was: then the column is the newer choice.
fn kill_switch(row: &Model, stored: &StoredTargeting) -> bool {
    match stored.written_at {
        Some(written_at) if row.updated_at > written_at => {
            tracing::warn!(
                flag = %row.key,
                enabled = row.enabled,
                "feature flag switch changed by an older binary; serving its value"
            );
            row.enabled
        }
        _ => stored.enabled,
    }
}

pub async fn fetch_all(db: &DatabaseConnection) -> Result<Vec<Model>, DbErr> {
    Entity::find().all(db).await
}

pub async fn fetch(db: &DatabaseConnection, key: &str) -> Result<Option<Model>, DbErr> {
    Entity::find_by_id(key.to_string()).one(db).await
}

/// Write a flag's kill switch and rules. `targeting: None` makes it a plain global
/// flag again.
pub async fn write(
    db: &DatabaseConnection,
    def: &FlagDef,
    enabled: bool,
    targeting: Option<&Targeting>,
) -> Result<Model, DbErr> {
    let now = now();
    let Some(targeting) = targeting else {
        return put(db, def.key, enabled, None, now).await;
    };
    let stored = StoredTargeting {
        enabled,
        written_at: Some(now),
        targeting: targeting.clone(),
    };
    let json = serde_json::to_value(stored)
        .map_err(|e| DbErr::Custom(format!("feature flag targeting: {e}")))?;
    put(db, def.key, def.default_enabled, Some(json), now).await
}

/// Now, to the microsecond Postgres keeps, so the `written_at` in a document equals the
/// `updated_at` it was written with rather than reading back a few nanoseconds later.
fn now() -> DateTime<FixedOffset> {
    let micros = Utc::now().timestamp_micros();
    DateTime::from_timestamp_micros(micros)
        .expect("now is in range")
        .fixed_offset()
}

/// Set a plain global flag on or off, clearing any rules. Takes a bare key, so a
/// test can write a row for a flag the registry no longer declares.
pub async fn upsert(db: &DatabaseConnection, key: &str, enabled: bool) -> Result<Model, DbErr> {
    put(db, key, enabled, None, now()).await
}

async fn put(
    db: &DatabaseConnection,
    key: &str,
    enabled: bool,
    targeting: Option<serde_json::Value>,
    now: DateTime<FixedOffset>,
) -> Result<Model, DbErr> {
    let active = ActiveModel {
        key: Set(key.to_string()),
        enabled: Set(enabled),
        updated_at: Set(now),
        targeting: Set(targeting),
    };
    Entity::insert(active)
        .on_conflict(
            OnConflict::column(Column::Key)
                .update_columns([Column::Enabled, Column::UpdatedAt, Column::Targeting])
                .to_owned(),
        )
        .exec_with_returning(db)
        .await
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::server::feature_flags::targeting::Fallthrough;

    fn at(secs: i64) -> DateTime<FixedOffset> {
        DateTime::from_timestamp(1_800_000_000 + secs, 0)
            .unwrap()
            .fixed_offset()
    }

    /// A row last updated at `at(updated)`.
    fn row_at(enabled: bool, updated: i64, targeting: Option<serde_json::Value>) -> Model {
        Model {
            key: "k".into(),
            enabled,
            updated_at: at(updated),
            targeting,
        }
    }

    fn row(enabled: bool, targeting: Option<serde_json::Value>) -> Model {
        row_at(enabled, 0, targeting)
    }

    #[test]
    fn a_row_without_targeting_is_its_enabled_column() {
        let d = decode(&row(true, None));
        assert!(d.enabled);
        assert_eq!(d.targeting, Ok(None));
    }

    #[test]
    fn a_targeted_row_reads_its_kill_switch_from_the_document_not_the_column() {
        let targeting = Targeting {
            users: BTreeMap::new(),
            orgs: BTreeMap::new(),
            fallthrough: Fallthrough::Variant {
                variant: "on".into(),
            },
            salt: None,
        };
        let json = serde_json::to_value(StoredTargeting {
            enabled: false,
            written_at: Some(at(0)),
            targeting: targeting.clone(),
        })
        .unwrap();
        let d = decode(&row(true, Some(json)));
        assert!(!d.enabled);
        assert_eq!(d.targeting, Ok(Some(targeting)));
    }

    fn on_for_everyone() -> Targeting {
        Targeting {
            users: BTreeMap::new(),
            orgs: BTreeMap::new(),
            fallthrough: Fallthrough::Variant {
                variant: "on".into(),
            },
            salt: None,
        }
    }

    /// A document with the switch `enabled`, written at `at(0)`.
    fn written(enabled: bool) -> serde_json::Value {
        serde_json::to_value(StoredTargeting {
            enabled,
            written_at: Some(at(0)),
            targeting: on_for_everyone(),
        })
        .unwrap()
    }

    #[test]
    fn a_switch_an_older_binary_flipped_wins_on_roll_forward() {
        // Untouched since (the column is the registry default): the document's switch.
        assert!(decode(&row_at(false, 0, Some(written(true)))).enabled);
        assert!(!decode(&row_at(false, 0, Some(written(false)))).enabled);
        // An older binary, which sees only the column, turned the flag on.
        assert!(decode(&row_at(true, 60, Some(written(false)))).enabled);
        // ...or turned it on and then off again during an incident: the column reads
        // as it was written, but the admin's last choice was off.
        assert!(!decode(&row_at(false, 120, Some(written(true)))).enabled);
    }

    #[test]
    fn a_written_document_carries_the_rows_updated_at() {
        let now = now();
        assert_eq!(now.timestamp_subsec_nanos() % 1_000, 0);
        let json = serde_json::to_value(StoredTargeting {
            enabled: true,
            written_at: Some(now),
            targeting: on_for_everyone(),
        })
        .unwrap();
        let back: StoredTargeting = serde_json::from_value(json).unwrap();
        assert_eq!(back.written_at, Some(now));
        assert!(
            decode(&Model {
                updated_at: now,
                ..row(false, Some(serde_json::to_value(back).unwrap()))
            })
            .enabled
        );
    }

    #[test]
    fn an_unreadable_document_is_reported_and_reads_off() {
        let d = decode(&row(
            true,
            Some(serde_json::json!({ "enabled": true, "orgs": 7 })),
        ));
        assert!(d.targeting.is_err());
        assert!(
            !d.enabled,
            "clearing unreadable rules must not leave the flag on"
        );
    }
}
