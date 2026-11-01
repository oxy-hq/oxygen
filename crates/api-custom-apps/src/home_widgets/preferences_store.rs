//! A person's own choices about the home (`home_user_preferences`): read,
//! write. One row per person per org, written lazily; no row is the default.

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DbErr, Statement};
use serde_json::Value;
use uuid::Uuid;

use super::preferences::Preferences;

/// The person's stored choices in `org_id`, or the default (nothing hidden,
/// Today shown) when they have made none.
pub async fn load(
    db: &DatabaseConnection,
    user_id: Uuid,
    org_id: Uuid,
) -> Result<Preferences, DbErr> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT hidden, today_hidden FROM home_user_preferences \
             WHERE user_id = $1 AND org_id = $2",
            [user_id.into(), org_id.into()],
        ))
        .await?;
    let Some(row) = row else {
        return Ok(Preferences::default());
    };
    let hidden: Value = row.try_get("", "hidden")?;
    Ok(Preferences {
        hidden: strings(&hidden),
        today_hidden: row.try_get("", "today_hidden")?,
    })
}

/// Write `prefs` over whatever the person had. One statement, so two tabs
/// saving at once leave one of them, never a mix.
pub async fn save(
    db: &DatabaseConnection,
    user_id: Uuid,
    org_id: Uuid,
    prefs: &Preferences,
) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO home_user_preferences (user_id, org_id, hidden, today_hidden, updated_at) \
         VALUES ($1, $2, $3, $4, now()) \
         ON CONFLICT (user_id, org_id) DO UPDATE SET hidden = EXCLUDED.hidden, \
           today_hidden = EXCLUDED.today_hidden, updated_at = now()",
        [
            user_id.into(),
            org_id.into(),
            Value::from(prefs.hidden.clone()).into(),
            prefs.today_hidden.into(),
        ],
    ))
    .await?;
    Ok(())
}

/// A stored `hidden` array, read leniently: anything that is not a string is
/// skipped, so a hand-edited row never fails a home load.
fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|list| {
            list.iter()
                .filter_map(|k| k.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::strings;

    #[test]
    fn a_stored_array_reads_leniently() {
        assert_eq!(strings(&json!(["a/b", 7, null, "c/d#x"])), ["a/b", "c/d#x"]);
        assert!(strings(&json!({ "a": 1 })).is_empty());
    }
}
