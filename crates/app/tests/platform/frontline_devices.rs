//! The kiosk binding, against a real database.
//!
//! What the module promises and what each case removes: a bound device
//! resolves from its cookie; a forged secret, a stale token, a second use of
//! the same link, and a revoked device each resolve to NOTHING — one `None`,
//! because the login handler answers every one of them with the same 401 a
//! wrong PIN gets. And a device is its org's: the handler compares
//! `device.org_id` to the org the PIN was typed for, so the fixture proves the
//! org is on the row rather than assumed.

use axum::http::{HeaderMap, HeaderValue, header};
use entity::{org_kiosk_devices, organizations, users};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait,
    QueryFilter, Statement, TransactionTrait,
};
use uuid::Uuid;

use crate::common::{Schema, fresh_db};
use oxy_app::server::api::frontline_devices::{
    DEFAULT_IDLE_TIMEOUT_SECONDS, DeviceError, DeviceUpdate, KIOSK_COOKIE_NAME, NewDevice,
    bind_with_token, bound_device, create, reissue_link, revoke, update,
};

async fn seed_org(db: &DatabaseConnection) -> Uuid {
    let org = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org),
        name: ActiveValue::Set("Poke".into()),
        slug: ActiveValue::Set(format!("poke-{}", &org.simple().to_string()[..8])),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed org");
    org
}

async fn seed_admin(db: &DatabaseConnection) -> Uuid {
    let id = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(Some(format!("admin-{}@example.com", id.simple()))),
        name: ActiveValue::Set("Admin".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed admin");
    id
}

fn with_cookie(value: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(
        header::COOKIE,
        HeaderValue::from_str(&format!(
            "oxy_session=irrelevant; {KIOSK_COOKIE_NAME}={value}"
        ))
        .unwrap(),
    );
    h
}

#[tokio::test]
async fn a_kiosk_binds_once_and_then_resolves_from_its_cookie() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let org = seed_org(&db).await;
    let admin = seed_admin(&db).await;

    let (row, token) = create(
        &db,
        org,
        NewDevice {
            name: "Front counter",
            created_by: Some(admin),
            ..Default::default()
        },
    )
    .await
    .expect("create");
    assert!(row.bound_at.is_none() && row.secret_hash.is_none());
    assert!(
        row.enrol_token_hash.as_deref() != Some(token.as_str()),
        "the token itself must never be stored"
    );

    // Before binding, no cookie resolves — the enrol token is not a credential.
    assert!(
        bound_device(&db, &with_cookie(&format!("{}.{token}", row.id)))
            .await
            .is_none()
    );

    let (bound, cookie) = bind_with_token(&db, &token).await.expect("bind");
    assert_eq!(bound.id, row.id);
    assert!(bound.bound_at.is_some() && bound.enrol_token_hash.is_none());

    let device = bound_device(&db, &with_cookie(&cookie))
        .await
        .expect("a bound device resolves from its cookie");
    assert_eq!(device.org_id, org, "the org travels with the device");
    assert_eq!(device.name, "Front counter");
    // A kiosk enrolled without an idle timeout carries the default rather than
    // nothing: the column is nullable so old rows need no backfill, and the
    // number the app arms its timer with must always be present.
    assert_eq!(device.idle_timeout_seconds, DEFAULT_IDLE_TIMEOUT_SECONDS);

    // And one enrolled WITH a timeout carries that number back out — the write
    // path's read path, through a real column round-trip.
    let (_, brief_token) = create(
        &db,
        org,
        NewDevice {
            name: "Drive-thru",
            idle_timeout_seconds: Some(60),
            ..Default::default()
        },
    )
    .await
    .expect("create");
    let (_, brief_cookie) = bind_with_token(&db, &brief_token).await.expect("bind");
    assert_eq!(
        bound_device(&db, &with_cookie(&brief_cookie))
            .await
            .expect("the second kiosk resolves")
            .idle_timeout_seconds,
        60
    );

    // Single use: the same link opened on a second tablet binds nothing.
    assert!(matches!(
        bind_with_token(&db, &token).await,
        Err(DeviceError::NoSuchToken)
    ));

    // A forged secret for a real device id resolves to nothing.
    let forged = format!("{}.{}", row.id, "0".repeat(64));
    assert!(bound_device(&db, &with_cookie(&forged)).await.is_none());
    // So does a cookie that is not even the shape of one.
    assert!(bound_device(&db, &with_cookie("garbage")).await.is_none());

    // Revoke: the row stays, the cookie stops working, a second revoke is a no-op.
    assert!(revoke(&db, org, row.id).await.expect("revoke"));
    assert!(!revoke(&db, org, row.id).await.expect("revoke again"));
    assert!(bound_device(&db, &with_cookie(&cookie)).await.is_none());
    assert!(
        org_kiosk_devices::Entity::find_by_id(row.id)
            .one(&db)
            .await
            .expect("query")
            .is_some(),
        "revocation keeps the row — it is the audit trail"
    );
}

#[tokio::test]
async fn an_expired_or_foreign_link_binds_nothing_and_a_foreign_org_cannot_revoke() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let org = seed_org(&db).await;
    let other_org = seed_org(&db).await;

    // Expired: push the deadline into the past and the link is dead.
    let (row, token) = create(
        &db,
        org,
        NewDevice {
            name: "Back office",
            ..Default::default()
        },
    )
    .await
    .expect("create");
    org_kiosk_devices::ActiveModel {
        id: ActiveValue::Set(row.id),
        enrol_expires_at: ActiveValue::Set(Some(
            (chrono::Utc::now() - chrono::Duration::hours(1)).into(),
        )),
        ..Default::default()
    }
    .update(&db)
    .await
    .expect("expire");
    assert!(matches!(
        bind_with_token(&db, &token).await,
        Err(DeviceError::NoSuchToken)
    ));

    // A token nobody issued.
    assert!(matches!(
        bind_with_token(&db, "not-a-token").await,
        Err(DeviceError::NoSuchToken)
    ));

    // Another org cannot revoke this org's device — the org filter is the fence.
    let (mine, token) = create(
        &db,
        org,
        NewDevice {
            name: "Counter",
            ..Default::default()
        },
    )
    .await
    .expect("create");
    bind_with_token(&db, &token).await.expect("bind");
    assert!(matches!(
        revoke(&db, other_org, mine.id).await,
        Err(DeviceError::NotFound)
    ));

    // Bad inputs are refused before any row exists.
    assert!(matches!(
        create(
            &db,
            org,
            NewDevice {
                name: "   ",
                ..Default::default()
            }
        )
        .await,
        Err(DeviceError::BadName)
    ));
    assert!(matches!(
        create(
            &db,
            org,
            NewDevice {
                name: "Counter",
                return_to: Some("https://evil.example.com/"),
                ..Default::default()
            }
        )
        .await,
        Err(DeviceError::BadReturnTo)
    ));
    // An idle timeout nobody could work under is refused before a row exists,
    // rather than stored and left to sign a worker out mid-tap.
    assert!(matches!(
        create(
            &db,
            org,
            NewDevice {
                name: "Counter",
                idle_timeout_seconds: Some(0),
                ..Default::default()
            }
        )
        .await,
        Err(DeviceError::BadIdleTimeout)
    ));
}

/// A lost or expired link is replaced, not recovered: the new token binds, the
/// old one is dead the moment the new one exists, and once a tablet has bound
/// (or the kiosk is revoked) there is no link to hand out at all.
#[tokio::test]
async fn a_new_enrol_link_kills_the_old_one_and_only_while_unbound() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let org = seed_org(&db).await;
    let other_org = seed_org(&db).await;

    let (row, lost) = create(
        &db,
        org,
        NewDevice {
            name: "Front counter",
            ..Default::default()
        },
    )
    .await
    .expect("create");
    // Expire the first link so the reissue also proves the deadline restarts.
    org_kiosk_devices::ActiveModel {
        id: ActiveValue::Set(row.id),
        enrol_expires_at: ActiveValue::Set(Some(
            (chrono::Utc::now() - chrono::Duration::hours(1)).into(),
        )),
        ..Default::default()
    }
    .update(&db)
    .await
    .expect("expire");

    // The org filter is the fence: another org's admin gets nothing.
    assert!(matches!(
        reissue_link(&db, other_org, row.id).await,
        Err(DeviceError::NotFound)
    ));

    let (fresh, token) = reissue_link(&db, org, row.id).await.expect("reissue");
    assert_ne!(token, lost);
    assert!(
        fresh
            .enrol_expires_at
            .is_some_and(|t| t > chrono::Utc::now() + chrono::Duration::hours(23)),
        "a new link gets a full day"
    );

    // Two reissues in a row: only the latest link is live.
    let (_, latest) = reissue_link(&db, org, row.id).await.expect("reissue again");
    for dead in [&lost, &token] {
        assert!(matches!(
            bind_with_token(&db, dead).await,
            Err(DeviceError::NoSuchToken)
        ));
    }
    bind_with_token(&db, &latest)
        .await
        .expect("latest link binds");

    // Bound: moving the kiosk is revoke-and-enrol, never a new link.
    assert!(matches!(
        reissue_link(&db, org, row.id).await,
        Err(DeviceError::NotPending)
    ));

    // Revoked before it ever bound: also no link.
    let (waiting, _) = create(
        &db,
        org,
        NewDevice {
            name: "Drive-thru",
            ..Default::default()
        },
    )
    .await
    .expect("create");
    assert!(revoke(&db, org, waiting.id).await.expect("revoke"));
    assert!(matches!(
        reissue_link(&db, org, waiting.id).await,
        Err(DeviceError::NotPending)
    ));
}

/// The store tunes the tablet after the first shift, and the tablet stays
/// enrolled.
///
/// This is the case the module could not answer before: the idle sign-out was
/// writable only at enrolment, so moving it meant revoking a counter tablet and
/// walking a new link out to it. The assertion that matters is the LAST one of
/// each pair — the same cookie, still resolving, now carrying the new number.
#[tokio::test]
async fn an_enrolled_kiosk_changes_in_place_and_clears_back_to_the_default() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let org = seed_org(&db).await;

    let (row, token) = create(
        &db,
        org,
        NewDevice {
            name: "Front counter",
            idle_timeout_seconds: Some(60),
            ..Default::default()
        },
    )
    .await
    .expect("create");
    let (_, cookie) = bind_with_token(&db, &token).await.expect("bind");
    assert_eq!(
        bound_device(&db, &with_cookie(&cookie))
            .await
            .expect("bound")
            .idle_timeout_seconds,
        60
    );

    let (before, after) = update(
        &db,
        org,
        row.id,
        DeviceUpdate {
            idle_timeout_seconds: Some(Some(900)),
            ..Default::default()
        },
    )
    .await
    .expect("update");
    assert_eq!(before.idle_timeout_seconds, Some(60), "the row as it was");
    assert_eq!(after.idle_timeout_seconds, Some(900), "and as it is");
    assert_eq!(
        bound_device(&db, &with_cookie(&cookie))
            .await
            .expect("the tablet is still enrolled — that is the whole point")
            .idle_timeout_seconds,
        900
    );

    // Back to the default: the column must go to NULL rather than to a frozen
    // copy of today's 1800, or this kiosk would sit out the next change of the
    // default exactly as one enrolled with an explicit number does.
    let (_, cleared) = update(
        &db,
        org,
        row.id,
        DeviceUpdate {
            idle_timeout_seconds: Some(None),
            ..Default::default()
        },
    )
    .await
    .expect("clear");
    assert_eq!(
        cleared.idle_timeout_seconds, None,
        "clearing must store NULL, not the default's number"
    );
    assert_eq!(
        bound_device(&db, &with_cookie(&cookie))
            .await
            .expect("bound")
            .idle_timeout_seconds,
        DEFAULT_IDLE_TIMEOUT_SECONDS
    );

    // A rename is trimmed like enrolment's, and touches nothing else.
    let (_, renamed) = update(
        &db,
        org,
        row.id,
        DeviceUpdate {
            name: Some("  Counter 1  "),
            ..Default::default()
        },
    )
    .await
    .expect("rename");
    assert_eq!(renamed.name, "Counter 1");
    assert_eq!(
        renamed.idle_timeout_seconds, None,
        "a rename must not disturb the timeout it said nothing about"
    );

    // A body naming no field writes nothing and is not an error: a client
    // submitting an untouched form must not manufacture an audit entry.
    let (b, a) = update(&db, org, row.id, DeviceUpdate::default())
        .await
        .expect("an empty patch is a no-op, not a 400");
    assert_eq!(
        (b.name, b.idle_timeout_seconds),
        (a.name, a.idle_timeout_seconds)
    );
}

/// Until some statement in this database is waiting on a row lock.
///
/// How the race below is staged without a sleep that merely hopes: the other
/// admin's write commits only once this update is observably blocked on it.
async fn until_a_statement_waits_on_a_lock(db: &DatabaseConnection) {
    for _ in 0..500 {
        let row = db
            .query_one_raw(Statement::from_string(
                db.get_database_backend(),
                "SELECT count(*)::bigint AS waiting FROM pg_stat_activity \
                 WHERE datname = current_database() AND wait_event_type = 'Lock'",
            ))
            .await
            .expect("read pg_stat_activity")
            .expect("count(*) answers a row");
        let waiting: i64 = row.try_get("", "waiting").expect("waiting column");
        if waiting > 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("the update never waited on the other admin's row — the race was not staged");
}

/// The before/after pair an update answers is THIS update's change, and nobody
/// else's.
///
/// `update_device` hands the pair to the audit trail as "who changed this
/// tablet, and from what". As three separate statements — read, write, read —
/// a second admin's edit committing between the first read and the write sat
/// inside the pair and was signed with this admin's name. Staged exactly: the
/// other admin's rename is written and held uncommitted, this timeout change
/// starts, and the rename commits only once the change is seen waiting on it.
#[tokio::test]
async fn an_update_answers_only_its_own_change_when_another_admin_edits_the_same_kiosk() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let org = seed_org(&db).await;
    let (row, _) = create(
        &db,
        org,
        NewDevice {
            name: "Front counter",
            idle_timeout_seconds: Some(600),
            ..Default::default()
        },
    )
    .await
    .expect("create");

    let other_admin = db.begin().await.expect("begin the other admin's edit");
    org_kiosk_devices::Entity::update_many()
        .col_expr(
            org_kiosk_devices::Column::Name,
            sea_orm::sea_query::Expr::value("Counter 1"),
        )
        .filter(org_kiosk_devices::Column::Id.eq(row.id))
        .exec(&other_admin)
        .await
        .expect("the other admin's rename");

    let (mine, ()) = tokio::join!(
        update(
            &db,
            org,
            row.id,
            DeviceUpdate {
                idle_timeout_seconds: Some(Some(900)),
                ..Default::default()
            },
        ),
        async {
            until_a_statement_waits_on_a_lock(&db).await;
            other_admin
                .commit()
                .await
                .expect("commit the other admin's rename");
        }
    );
    let (before, after) = mine.expect("update");

    assert_eq!(
        (before.name.as_str(), after.name.as_str()),
        ("Counter 1", "Counter 1"),
        "the other admin's rename is inside this update's before/after pair, so the \
         trail would sign it with this admin's name"
    );
    assert_eq!(
        (before.idle_timeout_seconds, after.idle_timeout_seconds),
        (Some(600), Some(900)),
        "and the pair still carries the change this update did make"
    );
}

/// An update refuses exactly what enrolment refuses, and refuses it *before*
/// writing anything — the row an admin is told about is the row they have.
#[tokio::test]
async fn an_update_refuses_what_enrolment_refuses_and_leaves_the_row_untouched() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let org = seed_org(&db).await;
    let other_org = seed_org(&db).await;

    let (row, _) = create(
        &db,
        org,
        NewDevice {
            name: "Front counter",
            idle_timeout_seconds: Some(600),
            ..Default::default()
        },
    )
    .await
    .expect("create");

    // Zero is ambiguous, half a minute signs a worker out mid-tap, and past the
    // shift length the timer could never fire. Same window as `create`.
    for bad in [0u32, 1, 29, 12 * 3600 + 1, u32::MAX] {
        assert!(
            matches!(
                update(
                    &db,
                    org,
                    row.id,
                    DeviceUpdate {
                        idle_timeout_seconds: Some(Some(bad)),
                        ..Default::default()
                    }
                )
                .await,
                Err(DeviceError::BadIdleTimeout)
            ),
            "{bad} should not be storable through an update either"
        );
    }
    assert!(matches!(
        update(
            &db,
            org,
            row.id,
            DeviceUpdate {
                name: Some("   "),
                ..Default::default()
            }
        )
        .await,
        Err(DeviceError::BadName)
    ));

    // A good name beside an impossible timeout writes NEITHER. A partial write
    // here would leave the admin reading a 400 about a row that had already
    // half-changed.
    assert!(matches!(
        update(
            &db,
            org,
            row.id,
            DeviceUpdate {
                name: Some("Renamed"),
                idle_timeout_seconds: Some(Some(0)),
            }
        )
        .await,
        Err(DeviceError::BadIdleTimeout)
    ));

    // Another org's admin cannot reach this device — the same fence `revoke`
    // puts up, and not found rather than not allowed.
    assert!(matches!(
        update(
            &db,
            other_org,
            row.id,
            DeviceUpdate {
                idle_timeout_seconds: Some(Some(900)),
                ..Default::default()
            }
        )
        .await,
        Err(DeviceError::NotFound)
    ));

    let untouched = org_kiosk_devices::Entity::find_by_id(row.id)
        .one(&db)
        .await
        .expect("query")
        .expect("row");
    assert_eq!(untouched.name, "Front counter");
    assert_eq!(untouched.idle_timeout_seconds, Some(600));

    // Revoked: the row is the record of which tablet a shift was signed in on,
    // and neither number can apply again. Bringing it back is enrolling it.
    assert!(revoke(&db, org, row.id).await.expect("revoke"));
    assert!(matches!(
        update(
            &db,
            org,
            row.id,
            DeviceUpdate {
                name: Some("Recycled"),
                ..Default::default()
            }
        )
        .await,
        Err(DeviceError::Revoked)
    ));
    assert_eq!(
        org_kiosk_devices::Entity::find_by_id(row.id)
            .one(&db)
            .await
            .expect("query")
            .expect("row")
            .name,
        "Front counter",
        "a revoked kiosk's label must stay what the shift was signed in on"
    );
}
