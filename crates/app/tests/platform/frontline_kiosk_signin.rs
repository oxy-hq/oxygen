//! Sign-in on a store's kiosk, against a real database.
//!
//! `frontline_roster` pins WHO a kiosk's name picker shows. This pins that the
//! same people — and only they — can sign in on it. A right PIN typed on the
//! Clovis tablet by someone rostered only at Santa Rosa is refused, and refused
//! the way a wrong PIN is: the same status, headers and body bytes, and the same
//! attempt charged against the same lockout budget. Anything less would turn
//! the tablet into a way to learn which stores a person works at.
//!
//! One kiosk is the exception on the charge, and only that: a store with NOBODY
//! rostered refuses every attempt alike, so it answers the same bytes without
//! reading or charging anybody's credential.
//!
//! Every PIN here is real (the Argon2 hash `enroll_worker` writes). The refusal
//! under test happens only AFTER a PIN has verified, so a fixture whose PIN
//! could never match would pass for the wrong reason.
//!
//! Run with:
//! `cargo nextest run -p oxy-app --test platform -E 'test(frontline_kiosk_signin)'`

use axum::Json;
use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use entity::{org_frontline_members, org_role_members, org_roles, user_credentials, users};
use oxy_app::server::api::frontline::{LoginRequest, login};
use oxy_auth::frontline::{KIND_PIN, PinPolicy, hash_pin};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait,
    QueryFilter,
};
use uuid::Uuid;

use crate::common::test_db;
use crate::frontline_roster::{Store, kiosk_at, picker, seed_store};

const PIN: &str = "4821";
const WRONG_PIN: &str = "0000";

/// Everything a caller at the counter can observe about one sign-in.
#[derive(Debug, PartialEq)]
struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
}

/// `POST /api/frontline/login` from the tablet whose kiosk cookie is `kiosk`.
async fn sign_in(kiosk: &HeaderMap, store: &Store, identifier: &str, pin: &str) -> Answer {
    let response = login(
        kiosk.clone(),
        Json(LoginRequest {
            org: store.slug.clone(),
            identifier: identifier.to_string(),
            pin: pin.to_string(),
        }),
    )
    .await
    .into_response();
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status,
        headers: parts.headers,
        body: axum::body::to_bytes(body, 64 * 1024)
            .await
            .expect("login body"),
    }
}

/// Where a worker is rostered.
enum At {
    Store(Uuid),
    /// A franchisor-scope position: `org_role_members.location_id IS NULL`.
    OrgWide,
    /// No assignment row at all.
    Nowhere,
}

/// One active worker holding a real PIN credential, rostered as `at` says.
///
/// `hash` is computed once per test and shared: Argon2 in a debug build is the
/// slow part of this file, and it is the VERIFY that is under test, not the
/// enrolment.
async fn worker(
    db: &DatabaseConnection,
    store: &Store,
    hash: &str,
    identifier: &str,
    name: &str,
    at: At,
) -> Uuid {
    let id = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(None),
        name: ActiveValue::Set(name.into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(false),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed user");
    org_frontline_members::ActiveModel {
        org_id: ActiveValue::Set(store.org),
        user_id: ActiveValue::Set(id),
        status: ActiveValue::Set(org_frontline_members::STATUS_ACTIVE.into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed standing");
    user_credentials::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        user_id: ActiveValue::Set(id),
        kind: ActiveValue::Set(KIND_PIN.into()),
        org_id: ActiveValue::Set(Some(store.org)),
        identifier: ActiveValue::Set(identifier.into()),
        secret_hash: ActiveValue::Set(Some(hash.into())),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed credential");

    let (role, location) = match at {
        At::Store(place) => (store.role, Some(place)),
        At::OrgWide => (franchisor_role(db, store.org).await, None),
        At::Nowhere => return id,
    };
    org_role_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(store.org),
        role_id: ActiveValue::Set(role),
        user_id: ActiveValue::Set(id),
        location_id: ActiveValue::Set(location),
        supervisor_id: ActiveValue::Set(None),
        created_at: ActiveValue::Set(chrono::Utc::now().fixed_offset()),
    }
    .insert(db)
    .await
    .expect("seed assignment");
    id
}

/// A role held across the org — the Account Manager who works every store.
async fn franchisor_role(db: &DatabaseConnection, org: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    let now = chrono::Utc::now().fixed_offset();
    org_roles::ActiveModel {
        id: ActiveValue::Set(id),
        org_id: ActiveValue::Set(org),
        // Unique per org, and a test may seed more than one.
        name: ActiveValue::Set(format!("Account Manager {}", id.simple())),
        scope: ActiveValue::Set("franchisor".into()),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(db)
    .await
    .expect("seed franchisor role");
    id
}

async fn credential(db: &DatabaseConnection, user: Uuid) -> user_credentials::Model {
    user_credentials::Entity::find()
        .filter(user_credentials::Column::UserId.eq(user))
        .one(db)
        .await
        .expect("load credential")
        .expect("credential exists")
}

/// The case the store check must not break: the tablet's own crew.
#[tokio::test]
async fn a_worker_rostered_at_the_kiosks_store_signs_in() {
    let db = test_db().await;
    let store = seed_store(&db).await;
    let hash = hash_pin(PIN).expect("hash");
    let maria = worker(
        &db,
        &store,
        &hash,
        "maria",
        "Maria",
        At::Store(store.clovis),
    )
    .await;
    let clovis = kiosk_at(&db, store.org, Some(store.clovis)).await;

    assert_eq!(picker(clovis.clone(), &store.slug).await, ["Maria"]);
    let answer = sign_in(&clovis, &store, "maria", PIN).await;
    assert_eq!(answer.status, StatusCode::OK, "{answer:?}");
    let body: serde_json::Value = serde_json::from_slice(&answer.body).expect("login json");
    assert_eq!(body["name"], "Maria");
    assert!(
        answer.headers.contains_key(header::SET_COOKIE),
        "a sign-in that sets no session cookie is not a sign-in: {answer:?}"
    );
    assert_eq!(credential(&db, maria).await.failed_attempts, 0);
}

/// THE case: the right PIN, the wrong store.
///
/// Refused — and indistinguishable from a wrong PIN on every channel a caller
/// at the counter has: status, headers, body bytes, and the attempt it costs.
#[tokio::test]
async fn a_right_pin_from_another_store_is_refused_exactly_as_a_wrong_pin_is() {
    let db = test_db().await;
    let store = seed_store(&db).await;
    let hash = hash_pin(PIN).expect("hash");
    worker(
        &db,
        &store,
        &hash,
        "maria",
        "Maria",
        At::Store(store.clovis),
    )
    .await;
    let devon = worker(
        &db,
        &store,
        &hash,
        "devon",
        "Devon",
        At::Store(store.santa_rosa),
    )
    .await;
    let clovis = kiosk_at(&db, store.org, Some(store.clovis)).await;
    let santa_rosa = kiosk_at(&db, store.org, Some(store.santa_rosa)).await;

    // The PIN IS right: at his own store it signs him in. Without this the
    // refusal below could be a wrong PIN and the test would prove nothing.
    let at_home = sign_in(&santa_rosa, &store, "devon", PIN).await;
    assert_eq!(at_home.status, StatusCode::OK, "{at_home:?}");
    let stamped = credential(&db, devon).await.last_used_at;

    let not_his_store = sign_in(&clovis, &store, "devon", PIN).await;
    assert_eq!(
        not_his_store.status,
        StatusCode::UNAUTHORIZED,
        "a right PIN typed on another store's tablet must not open a shift there"
    );
    // Charged like a wrong PIN, and not stamped like a success.
    let after = credential(&db, devon).await;
    assert_eq!(
        after.failed_attempts, 1,
        "the refusal must cost the attempt a wrong PIN costs"
    );
    assert_eq!(
        after.last_used_at, stamped,
        "a refused sign-in must not be recorded as a use of the credential"
    );

    let his_wrong_pin = sign_in(&clovis, &store, "devon", WRONG_PIN).await;
    assert_eq!(
        credential(&db, devon).await.failed_attempts,
        2,
        "a wrong PIN charges one attempt, the same one the refusal above did"
    );
    let a_local_wrong_pin = sign_in(&clovis, &store, "maria", WRONG_PIN).await;

    assert_eq!(
        not_his_store, his_wrong_pin,
        "the store refusal must be byte-identical to his own wrong PIN"
    );
    assert_eq!(
        not_his_store, a_local_wrong_pin,
        "and to a wrong PIN from this store's own crew"
    );
    // Guard: `Answer` equality can tell two sign-ins apart at all.
    assert_ne!(not_his_store, at_home);
}

/// The same budget, not a parallel one: refusals at another store lock the
/// credential exactly as that many wrong PINs would — at the worker's own store
/// too. Otherwise the lockout would be a second channel telling the two apart.
#[tokio::test]
async fn refusals_on_another_stores_tablet_spend_the_lockout_budget_a_wrong_pin_does() {
    let db = test_db().await;
    let store = seed_store(&db).await;
    let hash = hash_pin(PIN).expect("hash");
    // Clovis has crew of its own. A store with nobody rostered charges nobody
    // (`a_store_with_nobody_rostered_refuses_every_pin_alike_and_charges_nobody`),
    // so without her this would be that case, not another store's tablet.
    worker(
        &db,
        &store,
        &hash,
        "maria",
        "Maria",
        At::Store(store.clovis),
    )
    .await;
    let devon = worker(
        &db,
        &store,
        &hash,
        "devon",
        "Devon",
        At::Store(store.santa_rosa),
    )
    .await;
    let clovis = kiosk_at(&db, store.org, Some(store.clovis)).await;
    let santa_rosa = kiosk_at(&db, store.org, Some(store.santa_rosa)).await;

    for attempt in 1..=PinPolicy::default().max_attempts {
        let answer = sign_in(&clovis, &store, "devon", PIN).await;
        assert_eq!(
            answer.status,
            StatusCode::UNAUTHORIZED,
            "attempt {attempt} on another store's tablet: {answer:?}"
        );
    }
    assert!(
        credential(&db, devon).await.locked_until.is_some(),
        "a full budget of refusals must arm the lockout, as wrong PINs do"
    );
    assert_eq!(
        sign_in(&santa_rosa, &store, "devon", PIN).await.status,
        StatusCode::UNAUTHORIZED,
        "locked out at his own store too, exactly as that many wrong PINs leave him"
    );
}

/// Rename a table in this test's own database: how a test stages one read
/// failing while everything around it still works.
async fn rename_table(db: &DatabaseConnection, from: &str, to: &str) {
    db.execute_unprepared(&format!("ALTER TABLE {from} RENAME TO {to}"))
        .await
        .unwrap_or_else(|e| panic!("rename {from} to {to}: {e}"));
}

/// A roster read that FAILED is not a roster read that found nobody.
///
/// Both keep the door shut, but only one of them is anything the worker did.
/// Charged like a wrong PIN, a server-side blip on the scope read made every
/// right PIN at that store a failed attempt, and five of them left the crew
/// locked out for the lockout window AFTER the database had recovered —
/// clearable only by an admin resetting each PIN. The failed read answers 503,
/// which is what tells a tablet to back off, and costs the credential nothing.
///
/// The outage is staged by taking `org_role_members` away: it is the one table
/// the scope read touches and nothing else on the sign-in path reads, so the org
/// lookup, the kiosk binding and the credential all still work — the shape of a
/// statement timeout on that one query.
#[tokio::test]
async fn a_failed_roster_read_refuses_the_shift_without_spending_the_lockout_budget() {
    let db = test_db().await;
    let store = seed_store(&db).await;
    let hash = hash_pin(PIN).expect("hash");
    let maria = worker(
        &db,
        &store,
        &hash,
        "maria",
        "Maria",
        At::Store(store.clovis),
    )
    .await;
    let clovis = kiosk_at(&db, store.org, Some(store.clovis)).await;
    // Her PIN, her store: without the outage she signs in.
    assert_eq!(picker(clovis.clone(), &store.slug).await, ["Maria"]);
    let before = sign_in(&clovis, &store, "maria", PIN).await;
    assert_eq!(before.status, StatusCode::OK, "{before:?}");

    rename_table(&db, "org_role_members", "org_role_members_offline").await;
    assert!(
        picker(clovis.clone(), &store.slug).await.is_empty(),
        "the picker still fails closed on a failed read — never the tenant's crew"
    );
    let mut during = Vec::new();
    for _ in 0..PinPolicy::default().max_attempts {
        during.push(sign_in(&clovis, &store, "maria", PIN).await.status);
    }
    let charged = credential(&db, maria).await;
    assert_eq!(
        (charged.failed_attempts, charged.locked_until.is_some()),
        (0, false),
        "a failed roster read must not be charged to the worker typing the right PIN \
         (statuses during the outage: {during:?})"
    );
    assert!(
        during.iter().all(|s| *s == StatusCode::SERVICE_UNAVAILABLE),
        "a failed read is ours, and says so: {during:?}"
    );

    rename_table(&db, "org_role_members_offline", "org_role_members").await;
    let after = sign_in(&clovis, &store, "maria", PIN).await;
    assert_eq!(
        after.status,
        StatusCode::OK,
        "once the database is back, so is the shift: {after:?}"
    );
}

/// A store's tablet with nobody rostered at that store — enrolled before the
/// crew import ran, a real pre-go-live state — signs in nobody, and must not
/// make anybody pay for trying.
///
/// Its picker is empty, so the only way to type at it is a hand-made request
/// (the login page says nobody is set up here instead of offering an ID box).
/// Charged, each right PIN typed there was a failed attempt, and five of them
/// locked the worker out org-wide — at the store they do work at, too. The
/// refusal belongs to the KIOSK, not to the worker: every attempt at such a
/// tablet is refused alike whatever identifier and PIN are typed, so answering
/// it without reading or charging any credential tells a caller nothing about
/// any person. It still pays the verify's cost and still answers the wrong-PIN
/// bytes.
#[tokio::test]
async fn a_store_with_nobody_rostered_refuses_every_pin_alike_and_charges_nobody() {
    let db = test_db().await;
    let store = seed_store(&db).await;
    let hash = hash_pin(PIN).expect("hash");
    // Not assigned anywhere yet: the crew import has not run.
    let maria = worker(&db, &store, &hash, "maria", "Maria", At::Nowhere).await;
    // Assigned at another store, where the same PIN works.
    let devon = worker(
        &db,
        &store,
        &hash,
        "devon",
        "Devon",
        At::Store(store.santa_rosa),
    )
    .await;
    let clovis = kiosk_at(&db, store.org, Some(store.clovis)).await;
    let santa_rosa = kiosk_at(&db, store.org, Some(store.santa_rosa)).await;
    assert!(
        picker(clovis.clone(), &store.slug).await.is_empty(),
        "the fixture is a store with nobody rostered"
    );

    // Her right PIN, one more time than the lockout budget allows.
    let mut right_pins = Vec::new();
    for _ in 0..=PinPolicy::default().max_attempts {
        right_pins.push(sign_in(&clovis, &store, "maria", PIN).await);
    }
    let charged = credential(&db, maria).await;
    assert_eq!(
        (charged.failed_attempts, charged.locked_until.is_some()),
        (0, false),
        "a kiosk nobody is rostered at must not charge the worker whose right PIN it refuses"
    );

    let wrong_pin = sign_in(&clovis, &store, "maria", WRONG_PIN).await;
    let no_such_worker = sign_in(&clovis, &store, "nobody-by-this-name", PIN).await;
    assert_eq!(wrong_pin.status, StatusCode::UNAUTHORIZED, "{wrong_pin:?}");
    for (attempt, answer) in right_pins.iter().enumerate() {
        assert_eq!(
            *answer, wrong_pin,
            "right PIN #{attempt} must be refused with a wrong PIN's exact bytes"
        );
    }
    assert_eq!(
        no_such_worker, wrong_pin,
        "and so must an identifier nobody holds — the refusal says nothing about who exists"
    );
    assert_eq!(
        credential(&db, maria).await.failed_attempts,
        0,
        "not even a wrong PIN is charged there: nothing about the PIN was asked"
    );

    // The reviewer's scenario: a whole budget of right PINs on the empty
    // store's tablet, then the worker's own store.
    for _ in 0..PinPolicy::default().max_attempts {
        sign_in(&clovis, &store, "devon", PIN).await;
    }
    let his = credential(&db, devon).await;
    assert_eq!(
        (his.failed_attempts, his.locked_until.is_some()),
        (0, false),
        "trying the empty store's tablet must not lock him out of his own"
    );
    let at_home = sign_in(&santa_rosa, &store, "devon", PIN).await;
    assert_eq!(at_home.status, StatusCode::OK, "{at_home:?}");
    // Guard: `Answer` equality can tell a sign-in from a refusal at all.
    assert_ne!(at_home, wrong_pin);
}

/// The roster has no allowance for org-wide positions — an Account Manager is
/// on no store's picker — so neither does sign-in. On a kiosk with no place
/// the picker is the org's, which includes them, and so does the sign-in.
#[tokio::test]
async fn an_org_wide_position_signs_in_only_where_the_picker_shows_it() {
    let db = test_db().await;
    let store = seed_store(&db).await;
    let hash = hash_pin(PIN).expect("hash");
    worker(
        &db,
        &store,
        &hash,
        "maria",
        "Maria",
        At::Store(store.clovis),
    )
    .await;
    worker(&db, &store, &hash, "amy", "Amy", At::OrgWide).await;
    let clovis = kiosk_at(&db, store.org, Some(store.clovis)).await;
    let unbound = kiosk_at(&db, store.org, None).await;

    assert_eq!(picker(clovis.clone(), &store.slug).await, ["Maria"]);
    let refused = sign_in(&clovis, &store, "amy", PIN).await;
    assert_eq!(refused.status, StatusCode::UNAUTHORIZED, "{refused:?}");
    assert_eq!(
        refused,
        sign_in(&clovis, &store, "amy", WRONG_PIN).await,
        "an org-wide position's right PIN must read as a wrong one on a store's tablet"
    );

    assert!(
        picker(unbound.clone(), &store.slug)
            .await
            .contains(&"Amy".to_string())
    );
    let answer = sign_in(&unbound, &store, "amy", PIN).await;
    assert_eq!(answer.status, StatusCode::OK, "{answer:?}");
}

/// A kiosk enrolled without a place keeps the org-wide picker, and so the
/// org-wide sign-in: another store's worker and a worker rostered nowhere both
/// sign in, because both are on its picker.
#[tokio::test]
async fn a_kiosk_with_no_place_signs_in_the_org_its_picker_shows() {
    let db = test_db().await;
    let store = seed_store(&db).await;
    let hash = hash_pin(PIN).expect("hash");
    worker(
        &db,
        &store,
        &hash,
        "maria",
        "Maria",
        At::Store(store.clovis),
    )
    .await;
    worker(
        &db,
        &store,
        &hash,
        "devon",
        "Devon",
        At::Store(store.santa_rosa),
    )
    .await;
    worker(&db, &store, &hash, "ghost", "Ghost", At::Nowhere).await;
    let unbound = kiosk_at(&db, store.org, None).await;

    let mut shown = picker(unbound.clone(), &store.slug).await;
    shown.sort();
    assert_eq!(shown, ["Devon", "Ghost", "Maria"]);
    for identifier in ["maria", "devon", "ghost"] {
        let answer = sign_in(&unbound, &store, identifier, PIN).await;
        assert_eq!(answer.status, StatusCode::OK, "{identifier}: {answer:?}");
    }
}

/// The property the other cases are instances of: on every kiosk, a worker
/// signs in exactly when their name is on that kiosk's picker. One rule, read
/// by both, so the two cannot drift — and this is what notices if they do.
#[tokio::test]
async fn sign_in_admits_exactly_the_names_on_the_kiosks_picker() {
    let db = test_db().await;
    let store = seed_store(&db).await;
    let hash = hash_pin(PIN).expect("hash");
    worker(
        &db,
        &store,
        &hash,
        "maria",
        "Maria",
        At::Store(store.clovis),
    )
    .await;
    worker(
        &db,
        &store,
        &hash,
        "devon",
        "Devon",
        At::Store(store.santa_rosa),
    )
    .await;
    worker(&db, &store, &hash, "amy", "Amy", At::OrgWide).await;
    worker(&db, &store, &hash, "ghost", "Ghost", At::Nowhere).await;
    let crew = [
        ("maria", "Maria"),
        ("devon", "Devon"),
        ("amy", "Amy"),
        ("ghost", "Ghost"),
    ];

    let mut disagreements = Vec::new();
    let mut admitted = 0;
    for (label, place) in [
        ("Clovis", Some(store.clovis)),
        ("Santa Rosa", Some(store.santa_rosa)),
        ("no-place", None),
    ] {
        let kiosk = kiosk_at(&db, store.org, place).await;
        let shown = picker(kiosk.clone(), &store.slug).await;
        for (identifier, name) in crew {
            let on_picker = shown.iter().any(|n| n == name);
            let signs_in = sign_in(&kiosk, &store, identifier, PIN).await.status == StatusCode::OK;
            admitted += usize::from(signs_in);
            if on_picker != signs_in {
                disagreements.push(format!(
                    "{name} on the {label} kiosk: on the picker = {on_picker}, signs in = {signs_in}"
                ));
            }
        }
    }
    assert!(
        disagreements.is_empty(),
        "sign-in and the picker disagree about who belongs on a tablet:\n{}",
        disagreements.join("\n")
    );
    // Guard: Maria at Clovis, Devon at Santa Rosa, all four with no place.
    // A matrix where nobody signs in would agree with an empty picker too.
    assert_eq!(
        admitted, 6,
        "the fixture must admit somebody for agreement to mean anything"
    );
}
