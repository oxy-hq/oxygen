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
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection, EntityTrait};
use uuid::Uuid;

use crate::common::{Schema, fresh_db};
use oxy_app::server::api::frontline_devices::{
    DEFAULT_IDLE_TIMEOUT_SECONDS, DeviceError, KIOSK_COOKIE_NAME, NewDevice, bind_with_token,
    bound_device, create, reissue_link, revoke,
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
