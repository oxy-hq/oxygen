//! The kiosk name picker, against a real database.
//!
//! `frontline::narrow_to_location`'s unit tests pin the RULE — a tablet at a
//! place shows the people assigned to that place. What they cannot say is which
//! rows the reads handed that rule, and the bug this file exists for lived
//! exactly there: the 200-row cap on the credential read was applied org-wide
//! and the store filter ran on whatever survived it. In a tenant past 200 PIN
//! credentials, a store whose crew sort late lost them — no error, no empty
//! list, the worker simply not on their own tablet.
//!
//! So the fixture is deliberately lopsided: 205 credentials at one store whose
//! identifiers sort FIRST, and the store under test's crew sorting LAST. Before
//! the fix that picker came back empty.

use axum::extract::Query;
use axum::http::{HeaderMap, HeaderValue, header};
use entity::{
    locations, org_frontline_members, org_role_members, org_roles, organizations, user_credentials,
    users,
};
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection, EntityTrait};
use uuid::Uuid;

use crate::common::test_db;
use oxy_app::server::api::frontline::{RosterQuery, roster};
use oxy_app::server::api::frontline_devices::{
    KIOSK_COOKIE_NAME, NewDevice, bind_with_token, create,
};

/// The picker's own cap, and the reason the fixture is 205 names wide.
const ROSTER_LIMIT: usize = 200;

/// One tenant with two stores and a store-scoped role — shared with
/// `frontline_kiosk_signin`, which asks the same question of the sign-in.
pub(crate) struct Store {
    pub(crate) org: Uuid,
    pub(crate) slug: String,
    pub(crate) clovis: Uuid,
    pub(crate) santa_rosa: Uuid,
    pub(crate) role: Uuid,
}

pub(crate) async fn seed_store(db: &DatabaseConnection) -> Store {
    let org = Uuid::new_v4();
    let slug = format!("poke-{}", &org.simple().to_string()[..8]);
    organizations::ActiveModel {
        id: ActiveValue::Set(org),
        name: ActiveValue::Set("Poke".into()),
        slug: ActiveValue::Set(slug.clone()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed org");

    let now = chrono::Utc::now().fixed_offset();
    let place = |name: &str| {
        let id = Uuid::new_v4();
        (
            id,
            locations::ActiveModel {
                id: ActiveValue::Set(id),
                org_id: ActiveValue::Set(org),
                name: ActiveValue::Set(name.into()),
                status: ActiveValue::Set("open".into()),
                timezone: ActiveValue::Set("UTC".into()),
                external_id: ActiveValue::Set(None),
                parent_id: ActiveValue::Set(None),
                kind: ActiveValue::Set(Some("store".into())),
                created_at: ActiveValue::Set(now),
                updated_at: ActiveValue::Set(now),
            },
        )
    };
    let (clovis, clovis_row) = place("Clovis");
    let (santa_rosa, santa_rosa_row) = place("Santa Rosa");
    locations::Entity::insert_many([clovis_row, santa_rosa_row])
        .exec(db)
        .await
        .expect("seed locations");

    let role = Uuid::new_v4();
    org_roles::ActiveModel {
        id: ActiveValue::Set(role),
        org_id: ActiveValue::Set(org),
        name: ActiveValue::Set("Crew".into()),
        scope: ActiveValue::Set("location".into()),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(db)
    .await
    .expect("seed role");

    Store {
        org,
        slug,
        clovis,
        santa_rosa,
        role,
    }
}

/// `(identifier, name)` pairs, owned, because the big fixture is generated.
fn people(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(id, name)| ((*id).to_string(), (*name).to_string()))
        .collect()
}

/// Identifiers that sort before every store name in these fixtures.
fn filler(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|i| (format!("aaa-{i:04}"), format!("Filler {i}")))
        .collect()
}

/// Workers with an active standing, a PIN credential and an assignment at one
/// place. Batched, because the size of the fixture is the point of it.
/// Returns the new user ids, in the order given.
async fn seed_crew(
    db: &DatabaseConnection,
    store: &Store,
    at: Uuid,
    crew: &[(String, String)],
) -> Vec<Uuid> {
    let ids: Vec<Uuid> = crew.iter().map(|_| Uuid::new_v4()).collect();
    let now = chrono::Utc::now().fixed_offset();

    users::Entity::insert_many(ids.iter().zip(crew).map(|(id, (_, name))| {
        users::ActiveModel {
            id: ActiveValue::Set(*id),
            // No email: having none is what a frontline worker is.
            email: ActiveValue::Set(None),
            name: ActiveValue::Set(name.clone()),
            picture: ActiveValue::Set(None),
            email_verified: ActiveValue::Set(false),
            ..Default::default()
        }
    }))
    .exec(db)
    .await
    .expect("seed users");

    org_frontline_members::Entity::insert_many(ids.iter().map(|id| {
        org_frontline_members::ActiveModel {
            org_id: ActiveValue::Set(store.org),
            user_id: ActiveValue::Set(*id),
            status: ActiveValue::Set(org_frontline_members::STATUS_ACTIVE.into()),
            ..Default::default()
        }
    }))
    .exec(db)
    .await
    .expect("seed standing");

    user_credentials::Entity::insert_many(ids.iter().zip(crew).map(|(id, (identifier, _))| {
        user_credentials::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            user_id: ActiveValue::Set(*id),
            kind: ActiveValue::Set("pin".into()),
            org_id: ActiveValue::Set(Some(store.org)),
            identifier: ActiveValue::Set(identifier.clone()),
            // Never verified here, and the schema refuses a PIN without one.
            secret_hash: ActiveValue::Set(Some("$argon2id$not-a-real-hash".into())),
            ..Default::default()
        }
    }))
    .exec(db)
    .await
    .expect("seed credentials");

    org_role_members::Entity::insert_many(ids.iter().map(|id| org_role_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(store.org),
        role_id: ActiveValue::Set(store.role),
        user_id: ActiveValue::Set(*id),
        location_id: ActiveValue::Set(Some(at)),
        supervisor_id: ActiveValue::Set(None),
        created_at: ActiveValue::Set(now),
    }))
    .exec(db)
    .await
    .expect("seed assignments");

    ids
}

/// An enrolled, bound kiosk at `at`, as the cookie header a request carries.
pub(crate) async fn kiosk_at(db: &DatabaseConnection, org: Uuid, at: Option<Uuid>) -> HeaderMap {
    let (_row, token) = create(
        db,
        org,
        NewDevice {
            name: "Front counter",
            location_id: at,
            ..Default::default()
        },
    )
    .await
    .expect("create kiosk");
    let (_bound, cookie) = bind_with_token(db, &token).await.expect("bind kiosk");
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        HeaderValue::from_str(&format!("{KIOSK_COOKIE_NAME}={cookie}")).expect("cookie header"),
    );
    headers
}

/// The names the picker would show, in the order it would show them.
pub(crate) async fn picker(headers: HeaderMap, slug: &str) -> Vec<String> {
    let response = roster(
        headers,
        Query(RosterQuery {
            org: slug.to_string(),
        }),
    )
    .await;
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("roster body");
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("roster json");
    body["staff"]
        .as_array()
        .expect("staff is a list and never absent — a kiosk reads body.staff")
        .iter()
        .map(|entry| entry["name"].as_str().expect("name").to_string())
        .collect()
}

#[tokio::test]
async fn the_row_cap_is_per_store_so_a_big_tenant_keeps_a_small_stores_crew() {
    let db = test_db().await;
    let store = seed_store(&db).await;

    // Santa Rosa is a big store in a big tenant: more PIN credentials than the
    // picker's whole cap, every one of them sorting first.
    seed_crew(&db, &store, store.santa_rosa, &filler(ROSTER_LIMIT + 5)).await;

    // Clovis has three people whose identifiers sort last — which used to be
    // the only thing deciding whether they existed.
    seed_crew(
        &db,
        &store,
        store.clovis,
        &people(&[
            ("zzz-maria", "Maria"),
            ("zzz-devon", "Devon"),
            ("zzz-cy", "Cy"),
        ]),
    )
    .await;

    let mut names = picker(
        kiosk_at(&db, store.org, Some(store.clovis)).await,
        &store.slug,
    )
    .await;
    names.sort();
    assert_eq!(
        names,
        ["Cy", "Devon", "Maria"],
        "the store's own crew must reach its own tablet however the rest of the \
         tenant's credentials sort"
    );
}

/// The other half of the same read: narrowing first must not have widened
/// anything. A Santa Rosa worker is still not on the Clovis tablet, and the cap
/// still bounds the picker — it is a screen, not a dataset.
#[tokio::test]
async fn a_kiosk_shows_its_own_store_and_no_more_than_the_cap() {
    let db = test_db().await;
    let store = seed_store(&db).await;
    seed_crew(&db, &store, store.clovis, &filler(ROSTER_LIMIT + 5)).await;
    seed_crew(
        &db,
        &store,
        store.santa_rosa,
        &people(&[("zzz-devon", "Devon")]),
    )
    .await;

    let names = picker(
        kiosk_at(&db, store.org, Some(store.clovis)).await,
        &store.slug,
    )
    .await;
    assert_eq!(
        names.len(),
        ROSTER_LIMIT,
        "the cap still bounds one store's own picker"
    );
    assert!(
        !names.iter().any(|name| name == "Devon"),
        "a worker rostered at another store is not on this tablet: {names:?}"
    );
}

/// A suspended worker never appears — and that has to survive the narrowing
/// being done by a different query than before.
#[tokio::test]
async fn a_suspended_worker_is_not_on_the_picker() {
    let db = test_db().await;
    let store = seed_store(&db).await;
    let crew = seed_crew(
        &db,
        &store,
        store.clovis,
        &people(&[("aaa-maria", "Maria"), ("aaa-devon", "Devon")]),
    )
    .await;

    // Suspend Devon the way the standing route does: the row stays, the status
    // changes.
    org_frontline_members::ActiveModel {
        org_id: ActiveValue::Set(store.org),
        user_id: ActiveValue::Set(crew[1]),
        status: ActiveValue::Set(org_frontline_members::STATUS_SUSPENDED.into()),
        ..Default::default()
    }
    .update(&db)
    .await
    .expect("suspend Devon");

    let names = picker(
        kiosk_at(&db, store.org, Some(store.clovis)).await,
        &store.slug,
    )
    .await;
    assert_eq!(names, ["Maria"]);
}

/// A kiosk enrolled without a place keeps the org-wide list — there is no store
/// to narrow to, and a picker nobody can load is a kiosk nobody can use.
#[tokio::test]
async fn a_kiosk_with_no_place_still_shows_the_org() {
    let db = test_db().await;
    let store = seed_store(&db).await;
    seed_crew(
        &db,
        &store,
        store.clovis,
        &people(&[("aaa-maria", "Maria")]),
    )
    .await;
    seed_crew(
        &db,
        &store,
        store.santa_rosa,
        &people(&[("bbb-devon", "Devon")]),
    )
    .await;

    let names = picker(kiosk_at(&db, store.org, None).await, &store.slug).await;
    assert_eq!(
        names,
        ["Maria", "Devon"],
        "with no location there is nothing to narrow to, so both stores show"
    );
}
