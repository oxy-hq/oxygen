//! The org's home layout row (`org_home_layouts`): read, write with an
//! optimistic revision check, reset.
//!
//! The layout's `for`s are stored beside the document (`audiences`), never in
//! it, so a server older than `for` still reads the org's layout in full
//! (`oxy_app_core::widgets::layout::split_for_storage`). Callers see one
//! document either way.

use oxy_app_core::widgets::layout::{join_stored, split_for_storage};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DbErr, Statement};
use serde_json::Value;
use uuid::Uuid;

/// A stored layout, its `for`s joined back in, and how many writes it has had.
#[derive(Debug, Clone, PartialEq)]
pub struct Stored {
    pub layout: Value,
    pub revision: i64,
}

/// Why a write did not land.
#[derive(Debug)]
pub enum WriteError {
    /// The layout changed since the writer read revision `expected`.
    Stale {
        current: i64,
    },
    Db(DbErr),
}

impl From<DbErr> for WriteError {
    fn from(e: DbErr) -> Self {
        Self::Db(e)
    }
}

pub async fn load(db: &DatabaseConnection, org_id: Uuid) -> Result<Option<Stored>, DbErr> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT layout, audiences, revision FROM org_home_layouts WHERE org_id = $1",
            [org_id.into()],
        ))
        .await?;
    row.map(|r| {
        let audiences: Option<Value> = r.try_get("", "audiences")?;
        Ok(Stored {
            layout: join_stored(r.try_get("", "layout")?, audiences.as_ref()),
            revision: r.try_get("", "revision")?,
        })
    })
    .transpose()
}

/// Write `layout` for `org_id`. With `expected`, only over that revision (`0`:
/// only when there is none yet); without, over whatever is there. One
/// statement each way, so two writers cannot both win. Answers the new
/// revision.
pub async fn save(
    db: &DatabaseConnection,
    org_id: Uuid,
    layout: &Value,
    expected: Option<i64>,
    by: Uuid,
) -> Result<i64, WriteError> {
    let (bare, audiences) = split_for_storage(layout);
    let written = match expected {
        None => {
            db.query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO org_home_layouts (org_id, layout, audiences, revision, updated_by, updated_at) \
                 VALUES ($1, $2, $4, 1, $3, now()) \
                 ON CONFLICT (org_id) DO UPDATE SET layout = EXCLUDED.layout, \
                   audiences = EXCLUDED.audiences, \
                   revision = org_home_layouts.revision + 1, \
                   updated_by = EXCLUDED.updated_by, updated_at = now() \
                 RETURNING revision",
                [org_id.into(), bare.clone().into(), by.into(), audiences.clone().into()],
            ))
            .await?
        }
        Some(0) => {
            db.query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO org_home_layouts (org_id, layout, audiences, revision, updated_by, updated_at) \
                 VALUES ($1, $2, $4, 1, $3, now()) ON CONFLICT (org_id) DO NOTHING RETURNING revision",
                [org_id.into(), bare.clone().into(), by.into(), audiences.clone().into()],
            ))
            .await?
        }
        Some(revision) => {
            db.query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE org_home_layouts SET layout = $2, audiences = $5, revision = revision + 1, \
                   updated_by = $3, updated_at = now() \
                 WHERE org_id = $1 AND revision = $4 RETURNING revision",
                [
                    org_id.into(),
                    bare.clone().into(),
                    by.into(),
                    revision.into(),
                    audiences.clone().into(),
                ],
            ))
            .await?
        }
    };
    match written {
        Some(row) => Ok(row.try_get("", "revision")?),
        None => {
            let current = load(db, org_id).await?.map(|s| s.revision).unwrap_or(0);
            Err(WriteError::Stale { current })
        }
    }
}

/// Back to the default home. Answers whether there was a layout to clear.
pub async fn reset(db: &DatabaseConnection, org_id: Uuid) -> Result<bool, DbErr> {
    let done = db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM org_home_layouts WHERE org_id = $1",
            [org_id.into()],
        ))
        .await?;
    Ok(done.rows_affected() > 0)
}
