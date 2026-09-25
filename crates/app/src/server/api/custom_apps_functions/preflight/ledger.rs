//! What the preflight has already reported: the refusals seen by the last
//! rollout it let through, in `app_preflight_refusals`. A refusal absent from
//! it is this release's doing; one present is the running binary's, and is
//! carried over rather than blocking.
//!
//! Keyed on the rule and the database, never on the host's message: a reworded
//! refusal is the same refusal, and must not block a rollout as new.

use std::collections::{HashMap, HashSet};

use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement, TransactionTrait,
};
use uuid::Uuid;

use super::{Findings, Refusal};

/// `(app_id, function_name, rule, database)` — the table's primary key.
pub type Entry = (Uuid, String, String, String);

pub fn entry(refusal: &Refusal) -> Entry {
    (
        refusal.app_id,
        refusal.function.clone(),
        refusal.rule.to_string(),
        refusal.database.clone(),
    )
}

/// The whole ledger. Small: one row per refusal an app carries.
pub async fn known(db: &impl ConnectionTrait) -> Result<HashSet<Entry>, DbErr> {
    let rows = db
        .query_all_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT app_id, function_name, rule, database FROM app_preflight_refusals",
        ))
        .await?;
    rows.into_iter()
        .map(|row| {
            Ok((
                row.try_get("", "app_id")?,
                row.try_get("", "function_name")?,
                row.try_get("", "rule")?,
                row.try_get("", "database")?,
            ))
        })
        .collect()
}

/// What recording `findings` changes, sorted: rows to add, and rows to drop
/// because their refusal is gone — the function was republished without it,
/// or left its app's live build. Two things keep their rows, because
/// forgetting a refusal makes it new again: a function this run could not
/// check (an unreadable workspace is not a fixed app), and an app with no live
/// build at all (unpublished for a rollback, it may come back as it was).
pub fn changes(findings: &Findings) -> (Vec<Entry>, Vec<Entry>) {
    let current: HashSet<Entry> = findings.judged.iter().map(|j| entry(&j.refusal)).collect();
    let live_apps: HashSet<Uuid> = findings
        .checked
        .iter()
        .chain(&findings.unchecked)
        .map(|(app, _)| *app)
        .collect();
    let mut insert: Vec<Entry> = current
        .iter()
        .filter(|e| !findings.known.contains(*e))
        .cloned()
        .collect();
    let mut delete: Vec<Entry> = findings
        .known
        .iter()
        .filter(|e| {
            !current.contains(*e)
                && live_apps.contains(&e.0)
                && !findings.unchecked.contains(&(e.0, e.1.clone()))
        })
        .cloned()
        .collect();
    insert.sort();
    delete.sort();
    (insert, delete)
}

/// Make the ledger what this run found, in one transaction. Called only for a
/// rollout the preflight lets through.
pub async fn record(db: &DatabaseConnection, findings: &Findings) -> Result<(), DbErr> {
    let (insert, delete) = changes(findings);
    if insert.is_empty() && delete.is_empty() {
        return Ok(());
    }
    let reasons: HashMap<Entry, &str> = findings
        .judged
        .iter()
        .map(|j| (entry(&j.refusal), j.refusal.reason.as_str()))
        .collect();
    let release = env!("CARGO_PKG_VERSION");
    let txn = db.begin().await?;
    for (app_id, function, rule, database) in delete {
        txn.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "DELETE FROM app_preflight_refusals \
             WHERE app_id = $1 AND function_name = $2 AND rule = $3 AND database = $4",
            [app_id.into(), function.into(), rule.into(), database.into()],
        ))
        .await?;
    }
    for key in insert {
        let reason = reasons.get(&key).copied().unwrap_or_default().to_string();
        let (app_id, function, rule, database) = key;
        txn.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO app_preflight_refusals \
               (app_id, function_name, rule, database, reason, first_seen_release) \
             VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
            [
                app_id.into(),
                function.into(),
                rule.into(),
                database.into(),
                reason.into(),
                release.into(),
            ],
        ))
        .await?;
    }
    txn.commit().await
}

#[cfg(test)]
mod tests {
    use super::super::{Judged, rule};
    use super::*;

    const APP: Uuid = Uuid::from_u128(1);

    fn judged(function: &str, reason: &str) -> Judged {
        Judged {
            refusal: Refusal {
                app_id: APP,
                app: "poke-house/warehouse".into(),
                function: function.into(),
                rule: rule::CUSTOMER_WAREHOUSE_WRITE,
                database: "clickhouse".into(),
                reason: reason.into(),
            },
            new: true,
            breaks_working: false,
        }
    }

    fn row(function: &str) -> Entry {
        (
            APP,
            function.into(),
            rule::CUSTOMER_WAREHOUSE_WRITE.into(),
            "clickhouse".into(),
        )
    }

    fn findings(
        judged: Vec<Judged>,
        checked: &[&str],
        unchecked: &[&str],
        known: &[Entry],
    ) -> Findings {
        let keys = |fs: &[&str]| fs.iter().map(|f| (APP, f.to_string())).collect();
        Findings {
            judged,
            checked: keys(checked),
            unchecked: keys(unchecked),
            known: known.iter().cloned().collect(),
        }
    }

    #[test]
    fn a_new_refusal_is_added_and_a_known_one_left_alone() {
        let f = findings(
            vec![
                judged("submit", "read-only"),
                judged("settings", "read-only"),
            ],
            &["submit", "settings"],
            &[],
            &[row("settings")],
        );
        assert_eq!(changes(&f), (vec![row("submit")], vec![]));
    }

    #[test]
    fn a_reworded_refusal_is_the_same_refusal() {
        // The host's message changed between releases; the rule and the
        // database did not. Keyed on prose, this would block as new.
        let f = findings(
            vec![judged("settings", "a clearer sentence than last release's")],
            &["settings"],
            &[],
            &[row("settings")],
        );
        assert_eq!(changes(&f), (vec![], vec![]));
    }

    #[test]
    fn a_refusal_that_is_gone_is_dropped() {
        // Republished with the declaration, or removed from the app's live
        // build: either way it must be new again if it comes back.
        let republished = findings(vec![], &["submit"], &[], &[row("submit")]);
        assert_eq!(changes(&republished), (vec![], vec![row("submit")]));
        let removed = findings(vec![], &["settings"], &[], &[row("submit")]);
        assert_eq!(changes(&removed), (vec![], vec![row("submit")]));
    }

    #[test]
    fn a_function_this_run_could_not_check_keeps_its_refusals() {
        let f = findings(vec![], &[], &["submit"], &[row("submit")]);
        assert_eq!(changes(&f), (vec![], vec![]));
    }

    #[test]
    fn an_app_with_no_live_build_keeps_its_refusals() {
        // Unpublished for a rollback: republishing the same build must not
        // make what it already carried read as new.
        let f = findings(vec![], &[], &[], &[row("submit")]);
        assert_eq!(changes(&f), (vec![], vec![]));
    }
}
