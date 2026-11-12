//! Tests for the operational tier: the schema-level singleton, the
//! configured-vs-installed drift report, and the entity ↔ airway key roster
//! pin.
//!
//! DB-backed tests skip (never fail) when `OXY_DATABASE_URL` is unset, per
//! `test_support::test_db` — grep a run for `skipping:` before believing a
//! PASS. They serialize on a keyed [`AdvisoryLock`] rather than
//! `#[serial_test::serial]`, which is process-local and therefore inert under
//! nextest's one-process-per-test execution; see `test_support.rs`.
//!
//! The lock matters more here than for the policy tier: this table has exactly
//! **one row**, so every test in this file writes the same row. Two of them in
//! parallel would not merely interleave, they would overwrite each other. And
//! this is not the only file that writes it — `crate::boot`'s tests do
//! too, which is why the key lives on `test_support` rather than here.

use agentic_airway::deployment_config::{COLUMNS, DeploymentValues, SINGLETON_ID};
use entity::airway_deployment_config;
use sea_orm::{ConnectionTrait, DatabaseConnection, EntityTrait, Iden, Iterable, Statement};

use super::{
    DeploymentWriteError, DriftReport, build_response, delete_deployment, load_row,
    upsert_deployment, values_from_row,
};
use crate::test_support::{self, AIRWAY_DEPLOYMENT_LOCK_KEY, AdvisoryLock, SKIP_MSG, test_db};

async fn lock() -> AdvisoryLock {
    let url = test_support::database_url().expect("OXY_DATABASE_URL set (test_db confirmed it)");
    AdvisoryLock::acquire(&url, AIRWAY_DEPLOYMENT_LOCK_KEY).await
}

/// Clear the singleton row so each test starts from "never configured".
async fn reset(db: &DatabaseConnection) {
    airway_deployment_config::Entity::delete_many()
        .exec(db)
        .await
        .expect("reset deployment config");
}

fn sample() -> DeploymentValues {
    DeploymentValues {
        timeout_secs: Some(90),
        max_retries: Some(7),
        user_agent: Some("oxy-airway/test".into()),
        retry_initial_delay_ms: Some(250),
        retry_max_delay_secs: Some(60),
        retry_backoff_factor: Some(1.5),
        cursor_lag_floor_secs: Some(120),
        tls_ca_cert: Some("/etc/pki/ca.pem".into()),
        tls_client_cert: Some("/etc/pki/client.pem".into()),
        tls_client_key_file: Some("/etc/pki/client.key".into()),
        tls_danger_accept_invalid_certs: Some(true),
    }
}

// The singleton is a schema property, not a convention

/// **The constraint actually holds.** A second row is refused by Postgres, and
/// so is a row under any id but `1` — the `PRIMARY KEY` closes the first door
/// and the `CHECK (id = 1)` closes the second. Driven with raw `INSERT`s
/// rather than through the upsert helper, because the helper is exactly the
/// code path that would make a broken constraint invisible.
#[tokio::test]
async fn the_singleton_constraint_is_enforced_by_the_schema() {
    let Some(db) = test_db().await else {
        println!("{SKIP_MSG}");
        return;
    };
    let _lock = lock().await;
    reset(&db).await;

    let insert = |sql: &'static str| {
        let db = db.clone();
        async move {
            db.execute_raw(Statement::from_string(db.get_database_backend(), sql))
                .await
        }
    };

    insert("INSERT INTO airway_deployment_config (id) VALUES (1)")
        .await
        .expect("the one row is insertable");

    let dup = insert("INSERT INTO airway_deployment_config (id) VALUES (1)").await;
    assert!(
        dup.is_err(),
        "a second row with id 1 was accepted — the PRIMARY KEY is missing"
    );

    let other_id = insert("INSERT INTO airway_deployment_config (id) VALUES (2)").await;
    assert!(
        other_id.is_err(),
        "a row with id 2 was accepted — `CHECK (id = 1)` is missing, so the table is \
         only a singleton by convention"
    );

    // And the default fills the id in, so a writer never has to know it.
    reset(&db).await;
    insert("INSERT INTO airway_deployment_config (timeout_secs) VALUES (30)")
        .await
        .expect("id defaults to 1");
    let row = load_row(&db).await.expect("load").expect("row");
    assert_eq!(row.id, SINGLETON_ID);

    reset(&db).await;
}

/// The upsert really is an upsert: writing twice leaves one row, and the
/// second write's values win.
#[tokio::test]
async fn a_second_save_replaces_rather_than_inserting() {
    let Some(db) = test_db().await else {
        println!("{SKIP_MSG}");
        return;
    };
    let _lock = lock().await;
    reset(&db).await;

    upsert_deployment(&db, &sample()).await.expect("first save");
    let changed = DeploymentValues {
        timeout_secs: Some(45),
        ..sample()
    };
    upsert_deployment(&db, &changed).await.expect("second save");

    let all = airway_deployment_config::Entity::find()
        .all(&db)
        .await
        .expect("list");
    assert_eq!(all.len(), 1, "the upsert inserted a second row");
    assert_eq!(all[0].timeout_secs, Some(45));

    reset(&db).await;
}

/// A stored row round-trips through the entity unchanged, in the units the
/// column names state — and an all-`NULL` row comes back as all-`None`, not as
/// zeros.
#[tokio::test]
async fn a_stored_row_round_trips_and_nulls_stay_none() {
    let Some(db) = test_db().await else {
        println!("{SKIP_MSG}");
        return;
    };
    let _lock = lock().await;
    reset(&db).await;

    upsert_deployment(&db, &sample()).await.expect("save");
    let row = load_row(&db).await.expect("load").expect("row");
    assert_eq!(values_from_row(&row).expect("widen"), sample());

    upsert_deployment(&db, &DeploymentValues::default())
        .await
        .expect("save cleared");
    let row = load_row(&db).await.expect("load").expect("row");
    let values = values_from_row(&row).expect("widen");
    assert_eq!(
        values,
        DeploymentValues::default(),
        "a cleared setting came back as a value — absence is not zero"
    );
    // A cleared row is still a row; that is the distinction `configured_row_exists`
    // exists to carry.
    let resp = build_response(Some(&row), None).expect("response");
    assert!(resp.configured_row_exists);

    reset(&db).await;
}

/// Clearing removes the row entirely, which is a different state from saving
/// every field as `null` — an operator reading "never configured" should be
/// reading the truth.
#[tokio::test]
async fn delete_removes_the_row_and_is_idempotent() {
    let Some(db) = test_db().await else {
        println!("{SKIP_MSG}");
        return;
    };
    let _lock = lock().await;
    reset(&db).await;

    upsert_deployment(&db, &sample()).await.expect("save");
    delete_deployment(&db).await.expect("delete");
    assert!(load_row(&db).await.expect("load").is_none());
    delete_deployment(&db)
        .await
        .expect("delete again is a no-op");

    let resp = build_response(None, None).expect("response");
    assert!(!resp.configured_row_exists);
    assert_eq!(resp.configured, DeploymentValues::default());
}

/// A value airway refuses never reaches the table. Checked on the write path
/// so the row the worker installs from is one airway has already accepted.
#[tokio::test]
async fn an_invalid_value_is_refused_before_it_is_stored() {
    let Some(db) = test_db().await else {
        println!("{SKIP_MSG}");
        return;
    };
    let _lock = lock().await;
    reset(&db).await;

    let err = upsert_deployment(
        &db,
        &DeploymentValues {
            timeout_secs: Some(0),
            ..Default::default()
        },
    )
    .await
    .expect_err("a zero deadline must be refused");
    assert!(
        err.to_string().contains("timeout_secs"),
        "the diagnostic must name the key: {err}"
    );
    assert!(
        load_row(&db).await.expect("load").is_none(),
        "the refused value was written anyway"
    );
}

/// A value the column cannot hold is refused by name, not wrapped into one it
/// can.
///
/// `9223372036854775808` is a valid `u64` and valid JSON, so it clears serde
/// and clears airway (which has no opinion about Postgres' integer widths). A
/// bare `as` turned it into `i64::MIN` — a *negative* timeout, which is not the
/// value anyone sent. That is the same class as "absence read as zero": a wrong
/// value wearing a valid shape. The `CHECK (>= 0)` would then reject it as a
/// `500 db_err`, blaming the database for the caller's input.
///
/// Asserted through the real write path, and against the table, because the
/// half that matters is that nothing was stored.
#[tokio::test]
async fn a_value_too_wide_for_its_column_is_refused_rather_than_wrapped() {
    let Some(db) = test_db().await else {
        println!("{SKIP_MSG}");
        return;
    };
    let _lock = lock().await;
    reset(&db).await;

    // `BIGINT` columns against a `u64`: one past `i64::MAX`. `max_retries` is
    // `INTEGER`, so its ceiling is `i32::MAX` — a value that fits every other
    // column still does not fit that one.
    let past_i64 = u64::try_from(i64::MAX).expect("i64::MAX is non-negative") + 1;
    let past_i32 = u32::try_from(i32::MAX).expect("i32::MAX fits u32") + 1;
    let cases = [
        (
            "timeout_secs",
            DeploymentValues {
                timeout_secs: Some(past_i64),
                ..Default::default()
            },
        ),
        (
            "retry_max_delay_secs",
            DeploymentValues {
                retry_max_delay_secs: Some(past_i64),
                ..Default::default()
            },
        ),
        (
            "max_retries",
            DeploymentValues {
                max_retries: Some(past_i32),
                ..Default::default()
            },
        ),
    ];
    for (field, values) in cases {
        let err = upsert_deployment(&db, &values)
            .await
            .expect_err("a value past the column's width must be refused");
        // Matched on the variant, not on the message: airway validates first,
        // so a message-only assertion would still pass if airway happened to
        // reject the value for an unrelated reason and this check were absent.
        assert!(
            matches!(&err, DeploymentWriteError::OutOfRange { field: f, .. } if *f == field),
            "`{field}` must be refused as out of range, got: {err:?}"
        );
        assert!(
            err.to_string().contains(field),
            "the diagnostic must name the field: {err}"
        );
    }

    assert!(
        load_row(&db).await.expect("load").is_none(),
        "a refused value was written anyway"
    );

    // And the boundary itself is storable, so the refusal is a width check and
    // not an off-by-one that costs an operator a legitimate setting.
    upsert_deployment(
        &db,
        &DeploymentValues {
            max_retries: Some(u32::try_from(i32::MAX).expect("i32::MAX fits u32")),
            ..Default::default()
        },
    )
    .await
    .expect("i32::MAX retries is storable");

    reset(&db).await;
}

/// **A process that predates the column cannot silently clear it.**
///
/// The rolling-deploy hazard, stated: the migration adds
/// `cursor_lag_floor_secs` before the last replica of the previous image is
/// gone, so for a few minutes a build whose `DeploymentValues` has no such
/// field is still serving `PUT /deployment-config`. If that save were a
/// full-row overwrite it would erase a floor a newer replica had just
/// configured — no error, and nothing in the row afterwards to say it happened.
///
/// It is not, because [`upsert_deployment`]'s `OnConflict::update_columns` list
/// is **enumerated**: a build overwrites exactly the columns it knows and
/// leaves the rest alone. Driven with the raw statement a pre-#111 build would
/// generate rather than through the helper — the helper's column list is the
/// thing under test, so using it would assert nothing. Whoever swaps this for
/// `Entity::update`, which SeaORM writes for every column in the model, finds
/// out here rather than in production.
///
/// The `timeout_secs` assertion is what keeps the test honest: without it, a
/// statement that silently failed to apply would pass on the floor check alone.
#[tokio::test]
async fn an_older_writer_cannot_clear_the_cursor_lag_floor() {
    let Some(db) = test_db().await else {
        println!("{SKIP_MSG}");
        return;
    };
    let _lock = lock().await;
    reset(&db).await;

    // A current build configures a floor.
    upsert_deployment(&db, &sample())
        .await
        .expect("current build saves the whole row");
    assert_eq!(
        load_row(&db)
            .await
            .expect("load")
            .expect("row")
            .cursor_lag_floor_secs,
        Some(120)
    );

    // The previous image saves the ten columns it has. Column-for-column what
    // SeaORM emits for an `ActiveModel` without `cursor_lag_floor_secs` and an
    // `update_columns` list that does not mention it.
    db.execute_raw(Statement::from_string(
        db.get_database_backend(),
        r#"
        INSERT INTO airway_deployment_config
            (id, timeout_secs, max_retries, user_agent, retry_initial_delay_ms,
             retry_max_delay_secs, retry_backoff_factor, tls_ca_cert, tls_client_cert,
             tls_client_key_file, tls_danger_accept_invalid_certs, updated_at)
        VALUES (1, 45, 2, 'oxy-airway/previous-image', 100, 30, 2.0,
                NULL, NULL, NULL, NULL, NOW())
        ON CONFLICT (id) DO UPDATE SET
            timeout_secs = EXCLUDED.timeout_secs,
            max_retries = EXCLUDED.max_retries,
            user_agent = EXCLUDED.user_agent,
            retry_initial_delay_ms = EXCLUDED.retry_initial_delay_ms,
            retry_max_delay_secs = EXCLUDED.retry_max_delay_secs,
            retry_backoff_factor = EXCLUDED.retry_backoff_factor,
            tls_ca_cert = EXCLUDED.tls_ca_cert,
            tls_client_cert = EXCLUDED.tls_client_cert,
            tls_client_key_file = EXCLUDED.tls_client_key_file,
            tls_danger_accept_invalid_certs = EXCLUDED.tls_danger_accept_invalid_certs,
            updated_at = EXCLUDED.updated_at
        "#
        .to_string(),
    ))
    .await
    .expect("the previous image's upsert applies");

    let row = load_row(&db).await.expect("load").expect("row");
    assert_eq!(
        row.timeout_secs,
        Some(45),
        "the old write must actually have landed, or this test proves nothing"
    );
    assert_eq!(
        row.cursor_lag_floor_secs,
        Some(120),
        "a build with no `cursor_lag_floor_secs` cleared a configured floor — the upsert has \
         become a full-row overwrite"
    );

    reset(&db).await;
}

// Drift

/// No install in this process is **unknown**, never `in_sync`. This is the
/// multi-process case: a `serve` replica answering the request installed
/// nothing, and reporting that as agreement is precisely the lie the
/// `installed_scope` field exists to prevent.
#[test]
fn an_uninstalled_process_reports_unknown_not_in_sync() {
    let report = DriftReport::compare(&sample(), None);
    assert_eq!(report.status, "unknown");
    assert_eq!(report.reason, Some("not_installed_in_this_process"));
    assert!(report.fields.is_empty());

    // Including when nothing is configured either — "both empty" is not
    // observable agreement if we never observed the process.
    let report = DriftReport::compare(&DeploymentValues::default(), None);
    assert_eq!(report.status, "unknown");
}

#[test]
fn matching_configured_and_installed_are_in_sync() {
    let installed = sample().effective().expect("valid");
    let report = DriftReport::compare(&sample(), Some(&installed));
    assert_eq!(report.status, "in_sync");
    assert!(report.fields.is_empty());
    assert_eq!(report.reason, None);
}

/// Drift names the settings, so the banner can say which restart is owed.
#[test]
fn a_changed_setting_is_reported_by_name() {
    let mut installed = sample().effective().expect("valid");
    installed.timeout_secs = Some(30);
    installed.user_agent = Some("stale/0.1".into());

    let report = DriftReport::compare(&sample(), Some(&installed));
    assert_eq!(report.status, "drifted");
    assert_eq!(report.fields, vec!["timeout_secs", "user_agent"]);
}

/// Configuring a setting the running process does not have is drift — this is
/// the ordinary "saved but not restarted" case, and the whole reason the
/// region exists.
#[test]
fn saving_a_setting_into_a_process_running_defaults_is_drift() {
    let report = DriftReport::compare(&sample(), Some(&DeploymentValues::default()));
    assert_eq!(report.status, "drifted");
    assert_eq!(
        report.fields, COLUMNS,
        "every configured setting differs from a process running airway's built-ins"
    );
}

/// A row airway refuses is `unknown`, not `drifted`. The operator's problem is
/// that the next restart will fail, not that two values differ.
#[test]
fn an_invalid_configured_row_is_unknown_rather_than_drifted() {
    let invalid = DeploymentValues {
        retry_backoff_factor: Some(0.5),
        ..Default::default()
    };
    let report = DriftReport::compare(&invalid, Some(&DeploymentValues::default()));
    assert_eq!(report.status, "unknown");
    assert_eq!(report.reason, Some("configured_values_invalid"));
}

// The two column rosters cannot drift apart

/// `agentic-airway` may not depend on `entity` (see that crate's `CLAUDE.md`),
/// so it reads this table with a hand-written `SELECT` over
/// `deployment_config::COLUMNS` while the admin API reads it through the
/// entity. Nothing but this test sits between those two lists, and `oxy-app`
/// is the only crate that can see both.
///
/// A column added to the entity and not to `COLUMNS` is a setting the worker
/// silently never installs; one added to `COLUMNS` and not the entity is a
/// `SELECT` that fails at runtime on the worker and nowhere else.
#[test]
fn entity_columns_match_the_airway_key_roster() {
    let mut entity_columns: Vec<String> = airway_deployment_config::Column::iter()
        .map(|c| c.to_string())
        .filter(|c| c != "id" && c != "updated_at")
        .collect();
    entity_columns.sort();

    let mut declared: Vec<String> = COLUMNS.iter().map(|c| (*c).to_string()).collect();
    declared.sort();

    assert_eq!(
        entity_columns, declared,
        "`entity::airway_deployment_config` and \
         `agentic_airway::deployment_config::COLUMNS` have diverged — one of the two \
         readers of this table is now wrong"
    );
}

/// The four knobs that were proposed and rejected have no column, because they
/// have no reader in airway. Pinned by name so re-adding one is a deliberate
/// act with a failing test attached, not a quiet PR.
#[test]
fn inert_knobs_have_no_column() {
    let entity_columns: Vec<String> = airway_deployment_config::Column::iter()
        .map(|c| c.to_string())
        .collect();
    for inert in [
        "max_rewind",
        "cursor_lag_floor",
        "allow_unversioned_writes",
        "partition_repull_budget",
    ] {
        assert!(
            !entity_columns.iter().any(|c| c == inert),
            "`{inert}` has zero occurrences in airway's `src/`, so a column for it would \
             be accepted, stored and ignored"
        );
    }
}

/// **`cursor_lag_floor_secs` has two ways to say "nothing" and only one of them
/// is a value.** `NULL` is *no floor*; `0` is a floor of zero, which raises no
/// resource's lag and is therefore a setting that does not settle anything.
/// airway refuses the second so the two cannot be spelled the same way, and
/// this pins that the refusal reaches the *write path* rather than only the
/// install — a `0` accepted here would sit in the table until the next restart
/// and then fail every worker's boot install.
///
/// Written as one test because the two halves are the same claim: the column
/// must accept absence and refuse zero, and either half alone is satisfiable
/// by a mistake (refuse both, or accept both).
#[tokio::test]
async fn a_zero_cursor_lag_floor_is_refused_while_no_floor_is_stored() {
    let Some(db) = test_db().await else {
        println!("{SKIP_MSG}");
        return;
    };
    let _lock = lock().await;
    reset(&db).await;

    let err = upsert_deployment(
        &db,
        &DeploymentValues {
            cursor_lag_floor_secs: Some(0),
            ..Default::default()
        },
    )
    .await
    .expect_err("a zero floor must be refused, not stored");
    assert!(
        err.to_string().contains("cursor_lag_floor_secs"),
        "the diagnostic must name the key: {err}"
    );
    assert!(
        load_row(&db).await.expect("load").is_none(),
        "the refused floor was written anyway"
    );

    // Absence is storable, and comes back as absence — not as the `0` the
    // write path just refused.
    upsert_deployment(&db, &DeploymentValues::default())
        .await
        .expect("no floor is a valid deployment");
    let row = load_row(&db).await.expect("load").expect("row");
    assert_eq!(
        row.cursor_lag_floor_secs, None,
        "an unset floor materialised as a stored value"
    );
    assert_eq!(
        values_from_row(&row).expect("widen").cursor_lag_floor_secs,
        None
    );

    // And a real floor round-trips in the unit the column name states.
    upsert_deployment(
        &db,
        &DeploymentValues {
            cursor_lag_floor_secs: Some(900),
            ..Default::default()
        },
    )
    .await
    .expect("a positive floor saves");
    let row = load_row(&db).await.expect("load").expect("row");
    assert_eq!(row.cursor_lag_floor_secs, Some(900));

    reset(&db).await;
}
