//! The usage report's three tables: the weekly rows, who each was sent to, and
//! who asked not to be emailed.
//!
//! The two inserts that matter are claims. `period_start` is unique, so one
//! report lands per week whichever node writes it; `(report_id, email)` is a
//! primary key, so one node at a time sends an address its copy, and nobody
//! sends it again once it has gone.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use entity::{
    custom_app_usage_report_deliveries as deliveries, custom_app_usage_reports as reports,
    staff_notification_preferences as preferences,
};
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr,
    EntityTrait, QueryFilter, QueryOrder, Statement,
};
use uuid::Uuid;

use super::model::Snapshot;

/// The `staff_notification_preferences.notification` value for this report.
pub const NOTIFICATION: &str = "custom_app_usage_report";

/// How far back reports are kept, counted from the newest. Twelve weeks is
/// about as long as the views they were counted from are.
const WEEKS_KEPT: i64 = 12;

#[derive(Clone, Debug)]
pub struct StoredReport {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub snapshot: Snapshot,
}

impl TryFrom<reports::Model> for StoredReport {
    type Error = DbErr;

    fn try_from(row: reports::Model) -> Result<Self, DbErr> {
        let snapshot = serde_json::from_value(row.snapshot).map_err(|e| {
            DbErr::Custom(format!("usage report {} could not be read: {e}", row.id))
        })?;
        Ok(Self {
            id: row.id,
            created_at: row.created_at.with_timezone(&Utc),
            snapshot,
        })
    }
}

/// The report for the week starting at `period_start`, if one was written.
pub async fn for_period(
    db: &DatabaseConnection,
    period_start: DateTime<Utc>,
) -> Result<Option<StoredReport>, DbErr> {
    reports::Entity::find()
        .filter(reports::Column::PeriodStart.eq(period_start))
        .one(db)
        .await?
        .map(StoredReport::try_from)
        .transpose()
}

/// The newest report.
pub async fn latest(db: &DatabaseConnection) -> Result<Option<StoredReport>, DbErr> {
    reports::Entity::find()
        .order_by_desc(reports::Column::PeriodStart)
        .one(db)
        .await?
        .map(StoredReport::try_from)
        .transpose()
}

/// Write the report for its week unless one is already there. `true` when this
/// call wrote it. Either way the caller reads the row back and delivers that
/// one, so two nodes never send different numbers.
pub async fn insert_if_absent(db: &DatabaseConnection, snapshot: &Snapshot) -> Result<bool, DbErr> {
    let json = serde_json::to_value(snapshot)
        .map_err(|e| DbErr::Custom(format!("usage report could not be stored: {e}")))?;
    let inserted = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO custom_app_usage_reports (id, period_start, period_end, snapshot) \
             VALUES ($1, $2, $3, $4) ON CONFLICT (period_start) DO NOTHING",
            [
                Uuid::new_v4().into(),
                snapshot.period.start.into(),
                snapshot.period.end.into(),
                json.into(),
            ],
        ))
        .await?;
    Ok(inserted.rows_affected() == 1)
}

/// Drop reports older than [`WEEKS_KEPT`] weeks before `newest_start`; their
/// delivery rows go with them.
pub async fn prune(db: &DatabaseConnection, newest_start: DateTime<Utc>) -> Result<u64, DbErr> {
    let keep_from = newest_start - chrono::Duration::weeks(WEEKS_KEPT - 1);
    let deleted = reports::Entity::delete_many()
        .filter(reports::Column::PeriodStart.lt(keep_from))
        .exec(db)
        .await?;
    Ok(deleted.rows_affected)
}

/// Every address this report was sent to, or is being sent to.
pub async fn claimed(db: &DatabaseConnection, report_id: Uuid) -> Result<HashSet<String>, DbErr> {
    let rows = deliveries::Entity::find()
        .filter(deliveries::Column::ReportId.eq(report_id))
        .all(db)
        .await?;
    Ok(rows.into_iter().map(|r| r.email).collect())
}

/// Take this address's send of this report. `false` when it is already taken —
/// by another node, or by an earlier pass.
pub async fn claim_delivery(
    db: &DatabaseConnection,
    report_id: Uuid,
    email: &str,
) -> Result<bool, DbErr> {
    let claimed = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO custom_app_usage_report_deliveries (report_id, email) \
             VALUES ($1, $2) ON CONFLICT DO NOTHING",
            [report_id.into(), normalize(email).into()],
        ))
        .await?;
    Ok(claimed.rows_affected() == 1)
}

/// The provider accepted the message: the claim is now a record.
pub async fn mark_sent(db: &DatabaseConnection, report_id: Uuid, email: &str) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE custom_app_usage_report_deliveries SET sent_at = now() \
         WHERE report_id = $1 AND email = $2",
        [report_id.into(), normalize(email).into()],
    ))
    .await?;
    Ok(())
}

/// The send failed: give the claim back so the next pass may try again. A
/// claim that became a record is never released.
pub async fn release_delivery(
    db: &DatabaseConnection,
    report_id: Uuid,
    email: &str,
) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "DELETE FROM custom_app_usage_report_deliveries \
         WHERE report_id = $1 AND email = $2 AND sent_at IS NULL",
        [report_id.into(), normalize(email).into()],
    ))
    .await?;
    Ok(())
}

/// One address's stored answer, and who gave it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preference {
    pub enabled: bool,
    /// The address that last changed it: the person, or an admin for them.
    pub updated_by: Option<String>,
    pub updated_at: DateTime<Utc>,
}

/// Every stored answer for this report's email, by address. An address with
/// no entry never answered, which means send.
pub async fn preferences(db: &DatabaseConnection) -> Result<HashMap<String, Preference>, DbErr> {
    let rows = preferences::Entity::find()
        .filter(preferences::Column::Notification.eq(NOTIFICATION))
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let preference = Preference {
                enabled: r.enabled,
                updated_by: r.updated_by,
                updated_at: r.updated_at.with_timezone(&Utc),
            };
            (r.email, preference)
        })
        .collect())
}

/// Does this address want the report emailed? Yes unless it was turned off.
pub async fn wants_email(db: &DatabaseConnection, email: &str) -> Result<bool, DbErr> {
    let row = preferences::Entity::find_by_id((normalize(email), NOTIFICATION.to_string()))
        .one(db)
        .await?;
    Ok(row.is_none_or(|r| r.enabled))
}

/// Turn the email on or off for `email`. `by` is who did it — the person
/// themselves, or an admin acting for them.
pub async fn set_wants_email(
    db: &DatabaseConnection,
    email: &str,
    enabled: bool,
    by: &str,
) -> Result<(), DbErr> {
    let row = preferences::ActiveModel {
        email: Set(normalize(email)),
        notification: Set(NOTIFICATION.to_string()),
        enabled: Set(enabled),
        updated_at: Set(Utc::now().into()),
        updated_by: Set(Some(normalize(by))),
    };
    preferences::Entity::insert(row)
        .on_conflict(
            OnConflict::columns([
                preferences::Column::Email,
                preferences::Column::Notification,
            ])
            .update_columns([
                preferences::Column::Enabled,
                preferences::Column::UpdatedAt,
                preferences::Column::UpdatedBy,
            ])
            .to_owned(),
        )
        .exec_without_returning(db)
        .await?;
    Ok(())
}

/// Addresses are compared as `app_admins` stores them.
pub(super) fn normalize(email: &str) -> String {
    email.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_compared_trimmed_and_lowercased() {
        assert_eq!(normalize(" Root@Oxy.Tech "), "root@oxy.tech");
    }

    #[test]
    fn a_stored_row_that_is_not_a_report_is_an_error_and_not_an_empty_report() {
        let row = reports::Model {
            id: Uuid::from_u128(1),
            period_start: Utc::now().into(),
            period_end: Utc::now().into(),
            snapshot: serde_json::json!({ "orgs": "not a list" }),
            created_at: Utc::now().into(),
        };
        let err = StoredReport::try_from(row).unwrap_err();
        assert!(err.to_string().contains("could not be read"), "{err}");
    }
}
