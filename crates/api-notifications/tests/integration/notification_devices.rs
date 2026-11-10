//! DB-backed tests for device-token registration.
//!
//! These exist because the semantics under review are all *concurrency and
//! constraint* semantics, and the previous shape — `delete_many` then `insert`
//! — passed every test that only called it once. The bugs only appear against a
//! real `UNIQUE (platform, token)` index: a second registration raced into a
//! constraint violation, and a failed insert left the device deregistered
//! because its delete had already committed on its own.
//!
//! Run with:
//! `cargo nextest run -p oxy-api-notifications --test integration -E 'test(notification_devices)'`

use chrono::{Duration, Utc};
use entity::{device_tokens, users};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder,
};
use uuid::Uuid;

use oxy_api_notifications::devices::register::{Registration, prune_devices, upsert_device};
use oxy_api_notifications::devices::{self, DeviceError, DevicePatch};
use oxy_api_notifications::web_push_payload::{SubscriptionKeys, WebPushKeys};

use crate::common::{Schema, fresh_db};

/// `device_tokens.user_id` carries a real FK, so a bare uuid is rejected —
/// which is the point of running these against a live schema at all.
async fn seed_user(db: &DatabaseConnection) -> Uuid {
    let id = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(Some(format!("device-{id}@example.com"))),
        name: ActiveValue::Set("Device Test".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed user");
    id
}

/// Register `token` for `user` the way `POST /api/notifications/devices`
/// does; the device's id.
async fn register(
    db: &DatabaseConnection,
    user: Uuid,
    platform: &str,
    token: &str,
    name: Option<&str>,
    at: chrono::DateTime<chrono::FixedOffset>,
) -> Result<Uuid, sea_orm::DbErr> {
    let device = Registration {
        platform,
        token,
        name,
        app_version: None,
        keys: None,
    };
    upsert_device(db, user, &device, at).await
}

async fn rows_for(db: &DatabaseConnection, user: Uuid) -> Vec<device_tokens::Model> {
    device_tokens::Entity::find()
        .filter(device_tokens::Column::UserId.eq(user))
        .order_by_desc(device_tokens::Column::LastSeenAt)
        .all(db)
        .await
        .expect("query device tokens")
}

/// Re-registering the same device is the normal case — a client retry, an app
/// foregrounded twice — and must be idempotent rather than a constraint
/// violation. The delete-then-insert shape returned 500 here.
#[tokio::test]
async fn re_registering_the_same_token_is_idempotent() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let user = seed_user(&db).await;
    let t0 = Utc::now().fixed_offset();

    register(&db, user, "web", "tok-a", Some("Pixel"), t0)
        .await
        .expect("first registration");
    let later = t0 + Duration::minutes(5);
    register(&db, user, "web", "tok-a", Some("Pixel"), later)
        .await
        .expect("second registration must not violate the unique index");

    let rows = rows_for(&db, user).await;
    assert_eq!(rows.len(), 1, "a re-register wrote a second row");
    // Compared at MICROSECOND resolution, which is what `timestamptz` stores.
    // Asserting on the `DateTime` directly compares nanoseconds the database
    // truncated on the way in, so the test only passes where the platform clock
    // happens to be microsecond-aligned — green on macOS, red on Linux CI.
    let micros = |t: chrono::DateTime<chrono::FixedOffset>| t.timestamp_micros();
    // The column exists to prune devices that stopped refreshing, so it has to
    // actually move — before the upsert it was only ever a copy of created_at.
    assert_eq!(
        micros(rows[0].last_seen_at),
        micros(later),
        "last_seen_at did not advance"
    );
    assert_eq!(
        micros(rows[0].created_at),
        micros(t0),
        "created_at should not move"
    );
    assert!(
        rows[0].last_seen_at > rows[0].created_at,
        "the two timestamps did not separate"
    );
}

/// A token identifies a device install, not a person: a phone passed on, or an
/// app reinstalled under a different login, hands the same token to a second
/// account. Exactly one row must survive, owned by whoever registered last —
/// otherwise two users share it and one receives the other's notifications.
#[tokio::test]
async fn a_token_moves_to_whoever_registered_last() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let first = seed_user(&db).await;
    let second = seed_user(&db).await;
    let t0 = Utc::now().fixed_offset();

    register(&db, first, "apns", "shared-token", None, t0)
        .await
        .expect("first owner");
    register(&db, second, "apns", "shared-token", None, t0)
        .await
        .expect("second owner");

    assert!(
        rows_for(&db, first).await.is_empty(),
        "the previous owner still holds the token and will receive the new owner's pushes"
    );
    let now_owned = rows_for(&db, second).await;
    assert_eq!(now_owned.len(), 1);
    assert_eq!(now_owned[0].token, "shared-token");
}

/// The same token on two platforms is two devices, not one — the unique index
/// is on the pair, and collapsing them would silently drop a registration.
#[tokio::test]
async fn platform_is_part_of_the_identity() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let user = seed_user(&db).await;
    let t0 = Utc::now().fixed_offset();

    register(&db, user, "web", "same", None, t0).await.unwrap();
    register(&db, user, "fcm", "same", None, t0).await.unwrap();

    assert_eq!(rows_for(&db, user).await.len(), 2);
}

/// `notify` loads every one of a user's rows on every notification, so the set
/// has to stay bounded. The prune drops the least recently seen.
#[tokio::test]
async fn pruning_keeps_the_most_recently_seen_and_bounds_the_set() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let user = seed_user(&db).await;
    let base = Utc::now().fixed_offset();

    // 25 devices, oldest first, so the newest are unambiguous.
    for i in 0..25i64 {
        register(
            &db,
            user,
            "web",
            &format!("tok-{i:02}"),
            None,
            base + Duration::minutes(i),
        )
        .await
        .unwrap();
    }
    assert_eq!(rows_for(&db, user).await.len(), 25, "fixture did not land");

    prune_devices(&db, user).await;

    let kept = rows_for(&db, user).await;
    assert_eq!(kept.len(), 20, "the set is still unbounded");
    let kept_tokens: Vec<&str> = kept.iter().map(|r| r.token.as_str()).collect();
    assert!(
        kept_tokens.contains(&"tok-24"),
        "the most recently seen device was pruned"
    );
    assert!(
        !kept_tokens.contains(&"tok-00"),
        "the least recently seen device survived"
    );

    // Idempotent: a second prune at the bound is a no-op, not a slow drain.
    prune_devices(&db, user).await;
    assert_eq!(rows_for(&db, user).await.len(), 20);
}

/// One user's registrations must never prune another's.
#[tokio::test]
async fn pruning_is_scoped_to_one_user() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let noisy = seed_user(&db).await;
    let quiet = seed_user(&db).await;
    let base = Utc::now().fixed_offset();

    for i in 0..25i64 {
        register(
            &db,
            noisy,
            "web",
            &format!("noisy-{i:02}"),
            None,
            base + Duration::minutes(i),
        )
        .await
        .unwrap();
    }
    register(&db, quiet, "web", "quiet-1", None, base)
        .await
        .unwrap();

    prune_devices(&db, noisy).await;

    assert_eq!(rows_for(&db, quiet).await.len(), 1);
}

fn device_patch(v: serde_json::Value) -> DevicePatch {
    serde_json::from_value(v).expect("a device patch")
}

/// A phone passed to someone else keeps its id, and nothing of the previous
/// owner's: switched off, floored or OS-blocked for them is not for the new
/// owner.
#[tokio::test]
async fn a_device_that_changes_hands_starts_over() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let (first, second) = (seed_user(&db).await, seed_user(&db).await);
    let t0 = Utc::now().fixed_offset();
    let id = register(&db, first, "apns", "handed-on", Some("Ana's iPhone"), t0)
        .await
        .unwrap();
    let patch = device_patch(
        serde_json::json!({ "enabled": false, "min_level": "critical",
        "os_permission": "denied", "os_disabled": ["oxygen"], "app_version": "1.0" }),
    );
    devices::update(&db, first, id, patch).await.expect("patch");

    let again = register(&db, second, "apns", "handed-on", Some("iPhone"), t0)
        .await
        .unwrap();
    assert_eq!(again, id, "the same install is the same device");
    let row = &rows_for(&db, second).await[0];
    assert!(row.enabled);
    assert_eq!(
        (row.min_level.as_deref(), row.os_permission.as_deref()),
        (None, None)
    );
    assert_eq!(
        (row.os_disabled.as_ref(), row.app_version.as_deref()),
        (None, None)
    );
    assert_eq!(row.device_name.as_deref(), Some("iPhone"));
}

/// The owner re-registering (every launch) keeps the name they gave the
/// device and its settings; a newer app version replaces the old one.
#[tokio::test]
async fn re_registering_keeps_what_the_owner_set() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let user = seed_user(&db).await;
    let t0 = Utc::now().fixed_offset();
    let id = register(&db, user, "fcm", "pixel", Some("Pixel 9"), t0)
        .await
        .unwrap();
    let patch =
        device_patch(serde_json::json!({ "name": "Work phone", "min_level": "time_sensitive" }));
    devices::update(&db, user, id, patch).await.expect("rename");
    let device = Registration {
        platform: "fcm",
        token: "pixel",
        name: Some("Pixel 9"),
        app_version: Some("2.0.1"),
        keys: None,
    };
    upsert_device(&db, user, &device, t0 + Duration::minutes(1))
        .await
        .unwrap();
    let row = &rows_for(&db, user).await[0];
    assert_eq!(row.device_name.as_deref(), Some("Work phone"));
    assert_eq!(row.min_level.as_deref(), Some("time_sensitive"));
    assert_eq!(row.app_version.as_deref(), Some("2.0.1"));
}

/// `PATCH` and `DELETE /api/notifications/devices/{id}` touch only the
/// caller's own devices: anyone else's is 404, as is one that does not exist.
#[tokio::test]
async fn a_device_is_changed_or_removed_only_by_its_owner() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let (owner, other) = (seed_user(&db).await, seed_user(&db).await);
    // An hour ago, so "seen again" is past it at any clock resolution.
    let t0 = Utc::now().fixed_offset() - Duration::hours(1);
    let id = register(&db, owner, "web", "https://push.example/a", None, t0)
        .await
        .unwrap();
    let patch = || device_patch(serde_json::json!({ "enabled": false }));
    assert_eq!(
        devices::update(&db, other, id, patch()).await,
        Err(DeviceError::NotFound)
    );
    assert_eq!(
        devices::remove(&db, other, id).await,
        Err(DeviceError::NotFound)
    );
    assert_eq!(
        devices::update(&db, owner, Uuid::new_v4(), patch()).await,
        Err(DeviceError::NotFound)
    );
    assert!(devices::list_for(&db, other).await.unwrap().is_empty());
    assert!(
        rows_for(&db, owner).await[0].enabled,
        "untouched by the outsider"
    );

    let updated = devices::update(&db, owner, id, patch())
        .await
        .expect("mine");
    assert!(!updated.enabled);
    let seen = chrono::DateTime::parse_from_rfc3339(&updated.last_seen_at).unwrap();
    assert!(seen > t0, "a patch is the device saying it is here");
    let listed = devices::list_for(&db, owner).await.unwrap();
    assert_eq!((listed.len(), listed[0].platform), (1, "web"));
    devices::remove(&db, owner, id).await.expect("removed");
    assert!(rows_for(&db, owner).await.is_empty());
    assert_eq!(
        devices::remove(&db, owner, id).await,
        Err(DeviceError::NotFound)
    );
}

/// Keys shaped as a browser's — a 0x04 point and a 16-byte secret — distinct
/// per `seed`, so a replacement is observable.
fn web_keys(seed: u8) -> SubscriptionKeys {
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let mut point = vec![0x04u8];
    point.extend(std::iter::repeat_n(seed, 64));
    SubscriptionKeys::parse(&WebPushKeys {
        p256dh: URL_SAFE_NO_PAD.encode(point),
        auth: URL_SAFE_NO_PAD.encode([seed; 16]),
    })
    .expect("browser-shaped keys")
}

/// The keys a row holds, as stored.
async fn keys_of(db: &DatabaseConnection, id: Uuid) -> (Option<String>, Option<String>) {
    let row = device_tokens::Entity::find_by_id(id)
        .one(db)
        .await
        .expect("query")
        .expect("the device");
    (row.web_push_p256dh, row.web_push_auth)
}

/// A browser's subscription keys are what let a push carry a payload, so a
/// registration stores them; a re-registration that sends none (an older tab)
/// keeps them rather than downgrading the device to a bodiless wake; new ones
/// replace them; and they stay with the subscription when its endpoint moves
/// to another account, whose settings start over.
#[tokio::test]
async fn a_web_registration_stores_its_keys_and_a_keyless_one_keeps_them() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let user = seed_user(&db).await;
    let t0 = Utc::now().fixed_offset();
    let endpoint = "https://push.example/sub";
    let stored = |k: &SubscriptionKeys| {
        let (p, a) = k.to_stored();
        (Some(p), Some(a))
    };
    let with = |keys| Registration {
        platform: "web",
        token: endpoint,
        name: Some("Chrome on Mac"),
        app_version: None,
        keys,
    };

    let first = web_keys(1);
    let id = upsert_device(&db, user, &with(Some(&first)), t0)
        .await
        .expect("register with keys");
    assert_eq!(keys_of(&db, id).await, stored(&first));

    let again = register(&db, user, "web", endpoint, None, t0 + Duration::minutes(1))
        .await
        .expect("re-register without keys");
    assert_eq!(again, id, "the same device");
    assert_eq!(
        keys_of(&db, id).await,
        stored(&first),
        "a keyless re-registration erased the keys"
    );

    let second = web_keys(2);
    upsert_device(&db, user, &with(Some(&second)), t0 + Duration::minutes(2))
        .await
        .expect("re-register with new keys");
    assert_eq!(keys_of(&db, id).await, stored(&second));

    let other = seed_user(&db).await;
    register(&db, other, "web", endpoint, None, t0 + Duration::minutes(3))
        .await
        .expect("the endpoint moves to another account");
    assert_eq!(keys_of(&db, id).await, stored(&second));
}
