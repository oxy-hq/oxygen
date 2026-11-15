//! The assignment graph's cross-tenant gates.
//!
//! `POST /api/work` takes `org_id`, `assignee_user_id`, `assignee_role_id`,
//! `supervisor_id` and `location_id` **from the request body**, and `/work` is
//! mounted outside `/orgs/{org_id}` on purpose — nesting it would put
//! `org_middleware` in front, which rejects the frontline workers the graph
//! exists to route work to. So the gate `org_middleware` would have provided is
//! made by hand, and these are the tests that it actually is.
//!
//! This surface has shipped a cross-tenant write once, and the first fix for it
//! covered four of the five ids: `supervisor_id` went through unchecked, and
//! `Scope::SupervisedByMe` has no org predicate of its own. That is the case
//! `a_supervisor_from_another_org_is_refused` exists for.
//!
//! Run with:
//! `cargo nextest run -p oxy-api-operating-graph --test integration -E 'test(work_item_gates)'`

use axum::http::StatusCode;
use chrono::Utc;
use entity::{locations, org_members, org_roles, organizations, users};
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection};
use uuid::Uuid;

use oxy_api_operating_graph::work::dto::CreateWorkItem;
use oxy_api_operating_graph::work::handlers::{gate_create, has_standing_in_org};

use crate::common::{Schema, fresh_db};

/// One org, one member of it, one location and one role inside it.
struct Org {
    id: Uuid,
    member: Uuid,
    location: Uuid,
    role: Uuid,
}

async fn seed_org(db: &DatabaseConnection, label: &str) -> Org {
    let org = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org),
        name: ActiveValue::Set(format!("{label} Co")),
        slug: ActiveValue::Set(format!("{label}-{org}")),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed org");

    let member = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(member),
        email: ActiveValue::Set(Some(format!("{label}-{member}@example.com"))),
        name: ActiveValue::Set(format!("{label} Member")),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed user");
    org_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(org),
        user_id: ActiveValue::Set(member),
        role: ActiveValue::Set(org_members::OrgRole::Member),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed membership");

    let now = Utc::now().fixed_offset();
    let location = Uuid::new_v4();
    locations::ActiveModel {
        id: ActiveValue::Set(location),
        org_id: ActiveValue::Set(org),
        name: ActiveValue::Set(format!("{label} Store")),
        status: ActiveValue::Set("open".into()),
        timezone: ActiveValue::Set("UTC".into()),
        external_id: ActiveValue::Set(None),
        parent_id: ActiveValue::Set(None),
        kind: ActiveValue::Set(None),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(db)
    .await
    .expect("seed location");

    let role = Uuid::new_v4();
    org_roles::ActiveModel {
        id: ActiveValue::Set(role),
        org_id: ActiveValue::Set(org),
        name: ActiveValue::Set("Shift Lead".into()),
        scope: ActiveValue::Set("location".into()),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(db)
    .await
    .expect("seed role");

    Org {
        id: org,
        member,
        location,
        role,
    }
}

fn item(org: Uuid) -> CreateWorkItem {
    CreateWorkItem {
        org_id: org,
        title: "Closing checklist".into(),
        body: None,
        location_id: None,
        assignee_user_id: None,
        assignee_role_id: None,
        supervisor_id: None,
        due_at: None,
        priority: 0,
        source_kind: None,
        source_id: None,
    }
}

/// The baseline: everything in one org is accepted. Without this the refusal
/// tests below could all be passing for the wrong reason.
#[tokio::test]
async fn a_wholly_in_org_item_is_accepted() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let a = seed_org(&db, "acme").await;

    let mut body = item(a.id);
    body.assignee_user_id = Some(a.member);
    body.supervisor_id = Some(a.member);
    body.assignee_role_id = Some(a.role);
    body.location_id = Some(a.location);

    assert_eq!(gate_create(&db, a.member, &body).await, Ok(()));
}

/// An org the caller has no standing in answers 404, not 403 — a refusal must
/// not confirm the org exists.
#[tokio::test]
async fn a_foreign_org_is_not_confirmed_to_exist() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let a = seed_org(&db, "acme").await;
    let b = seed_org(&db, "beta").await;

    assert_eq!(
        gate_create(&db, a.member, &item(b.id)).await,
        Err(StatusCode::NOT_FOUND)
    );
}

/// The original hole: work addressed at somebody in another tenant, which
/// `Scope::AssignedToMe` then surfaces with no org predicate of its own.
#[tokio::test]
async fn an_assignee_from_another_org_is_refused() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let a = seed_org(&db, "acme").await;
    let b = seed_org(&db, "beta").await;

    let mut body = item(a.id);
    body.assignee_user_id = Some(b.member);
    assert_eq!(
        gate_create(&db, a.member, &body).await,
        Err(StatusCode::BAD_REQUEST)
    );
}

/// The edge the FIRST fix missed. `supervisor_id` went through unchecked while
/// the other four ids were gated, and `Scope::SupervisedByMe` filters on it
/// with no org predicate — so an arbitrary uuid landed authored content in a
/// stranger's queue, in any tenant.
#[tokio::test]
async fn a_supervisor_from_another_org_is_refused() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let a = seed_org(&db, "acme").await;
    let b = seed_org(&db, "beta").await;

    let mut body = item(a.id);
    body.supervisor_id = Some(b.member);
    assert_eq!(
        gate_create(&db, a.member, &body).await,
        Err(StatusCode::BAD_REQUEST)
    );
}

/// A role id routes work just as effectively as a user id.
#[tokio::test]
async fn a_role_from_another_org_is_refused() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let a = seed_org(&db, "acme").await;
    let b = seed_org(&db, "beta").await;

    let mut body = item(a.id);
    body.assignee_role_id = Some(b.role);
    assert_eq!(
        gate_create(&db, a.member, &body).await,
        Err(StatusCode::BAD_REQUEST)
    );
}

/// A location from another org would leak the item into that org's location
/// view.
#[tokio::test]
async fn a_location_from_another_org_is_refused() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let a = seed_org(&db, "acme").await;
    let b = seed_org(&db, "beta").await;

    let mut body = item(a.id);
    body.location_id = Some(b.location);
    assert_eq!(
        gate_create(&db, a.member, &body).await,
        Err(StatusCode::BAD_REQUEST)
    );
}

/// A uuid that names nothing is refused too — the FK proves existence at write
/// time, but the gate must not wave through an id it could not resolve.
#[tokio::test]
async fn ids_that_name_nothing_are_refused() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let a = seed_org(&db, "acme").await;

    for mutate in [
        |b: &mut CreateWorkItem| b.assignee_user_id = Some(Uuid::new_v4()),
        |b: &mut CreateWorkItem| b.supervisor_id = Some(Uuid::new_v4()),
        |b: &mut CreateWorkItem| b.assignee_role_id = Some(Uuid::new_v4()),
        |b: &mut CreateWorkItem| b.location_id = Some(Uuid::new_v4()),
    ] {
        let mut body = item(a.id);
        mutate(&mut body);
        assert_eq!(
            gate_create(&db, a.member, &body).await,
            Err(StatusCode::BAD_REQUEST)
        );
    }
}

/// Standing is org membership OR a tenant-defined role held there — a frontline
/// worker holds no `org_members` row by design, and locking them out would
/// defeat the graph's purpose.
#[tokio::test]
async fn a_role_holder_has_standing_without_a_membership_row() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let a = seed_org(&db, "acme").await;

    let worker = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(worker),
        email: ActiveValue::Set(None),
        name: ActiveValue::Set("Shift Lead".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(false),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed worker");

    assert!(
        !has_standing_in_org(&db, worker, a.id).await.expect("query"),
        "a bare user should have no standing yet"
    );

    entity::org_role_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(a.id),
        role_id: ActiveValue::Set(a.role),
        user_id: ActiveValue::Set(worker),
        location_id: ActiveValue::Set(Some(a.location)),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("grant role");

    assert!(
        has_standing_in_org(&db, worker, a.id).await.expect("query"),
        "a role holder was denied standing, which locks frontline workers out"
    );

    // And that standing is enough to be assigned work.
    let mut body = item(a.id);
    body.assignee_user_id = Some(worker);
    assert_eq!(gate_create(&db, a.member, &body).await, Ok(()));
}
