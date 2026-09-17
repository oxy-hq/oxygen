//! The operating graph against a real database: who may hold a position
//! where, the hierarchy, what each integration calls a place, and the two
//! `ctx.org` reads a function gets.
//!
//! `org_role_members` shipped with readers and no writer; every rule a writer
//! has to enforce lives in `assignments::assign`, and each case here removes
//! one ingredient and asserts the answer flips.

use entity::{
    app_members, apps, locations, org_frontline_members, org_members, org_roles, organizations,
    users,
};
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection};
use uuid::Uuid;

use crate::common::{Schema, fresh_db};
use oxy_app::server::api::custom_apps_functions::host::{
    org_assignments_directory, org_places_directory,
};
use oxy_app::server::api::operating_graph::assignments::{self, AssignError};
use oxy_app::server::api::operating_graph::dto::{
    AssignmentSpec, AssignmentsQuery, UpdateLocation,
};
use oxy_app::server::api::operating_graph::locations as places;
use oxy_app::server::api::operating_graph::positions;

struct Fx {
    org: Uuid,
    other_org: Uuid,
    app: Uuid,
    store: Uuid,
    other_store: Uuid,
    shift_lead: Uuid,
    area_manager: Uuid,
}

async fn org(db: &DatabaseConnection) -> Uuid {
    let id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(id),
        name: ActiveValue::Set("Poke".into()),
        slug: ActiveValue::Set(format!("poke-{}", &id.simple().to_string()[..8])),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed org");
    id
}

async fn user(db: &DatabaseConnection, name: &str, email: Option<String>) -> Uuid {
    let id = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(email),
        name: ActiveValue::Set(name.into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(false),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed user");
    id
}

async fn member(db: &DatabaseConnection, org: Uuid, name: &str) -> Uuid {
    let id = user(
        db,
        name,
        Some(format!("{}@example.com", Uuid::new_v4().simple())),
    )
    .await;
    org_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(org),
        user_id: ActiveValue::Set(id),
        role: ActiveValue::Set(entity::org_members::OrgRole::Member),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed membership");
    id
}

async fn worker(db: &DatabaseConnection, org: Uuid, name: &str, status: &str) -> Uuid {
    let id = user(db, name, None).await;
    org_frontline_members::ActiveModel {
        org_id: ActiveValue::Set(org),
        user_id: ActiveValue::Set(id),
        status: ActiveValue::Set(status.into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("enrol worker");
    id
}

async fn location(db: &DatabaseConnection, org: Uuid, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    let now = chrono::Utc::now().fixed_offset();
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
    }
    .insert(db)
    .await
    .expect("seed location");
    id
}

async fn role(db: &DatabaseConnection, org: Uuid, name: &str, scope: &str) -> Uuid {
    let id = Uuid::new_v4();
    let now = chrono::Utc::now().fixed_offset();
    org_roles::ActiveModel {
        id: ActiveValue::Set(id),
        org_id: ActiveValue::Set(org),
        name: ActiveValue::Set(name.into()),
        scope: ActiveValue::Set(scope.into()),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(db)
    .await
    .expect("seed role");
    id
}

async fn seed(db: &DatabaseConnection) -> Fx {
    let org_id = org(db).await;
    let other_org = org(db).await;
    let workspace = Uuid::new_v4();
    entity::workspaces::ActiveModel {
        id: ActiveValue::Set(workspace),
        org_id: ActiveValue::Set(Some(org_id)),
        name: ActiveValue::Set("workspace".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    let app = Uuid::new_v4();
    apps::ActiveModel {
        id: ActiveValue::Set(app),
        org_id: ActiveValue::Set(org_id),
        project_id: ActiveValue::Set(workspace),
        slug: ActiveValue::Set("store-ops".into()),
        name: ActiveValue::Set("Store Ops".into()),
        branch: ActiveValue::Set("main".into()),
        source_repo: ActiveValue::Set("git@example.com:poke/app.git".into()),
        status: ActiveValue::Set("active".into()),
        source_type: ActiveValue::Set("git".into()),
        source_config: ActiveValue::Set(serde_json::json!({})),
        visibility: ActiveValue::Set("org".into()),
        published_at: ActiveValue::Set(Some(chrono::Utc::now().fixed_offset())),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed app");
    Fx {
        org: org_id,
        other_org,
        app,
        store: location(db, org_id, "Clovis").await,
        other_store: location(db, other_org, "Elsewhere").await,
        shift_lead: role(db, org_id, "Shift Lead", "location").await,
        area_manager: role(db, org_id, "Area Manager", "franchisor").await,
    }
}

fn at(role_id: Uuid, location_id: Option<Uuid>) -> AssignmentSpec {
    AssignmentSpec {
        role_id,
        location_id,
        supervisor_id: None,
    }
}

#[tokio::test]
async fn a_member_or_an_active_worker_can_hold_a_position_and_nobody_else() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let fx = seed(&db).await;

    let maria = member(&db, fx.org, "Maria").await;
    let first = assignments::assign(&db, fx.org, maria, &at(fx.shift_lead, Some(fx.store)))
        .await
        .expect("a member is rostered");
    assert!(first.created);

    // Idempotent: the same position at the same place is the same row.
    let again = assignments::assign(&db, fx.org, maria, &at(fx.shift_lead, Some(fx.store)))
        .await
        .expect("re-rostering is not an error");
    assert!(
        !again.created,
        "a second identical write must not duplicate"
    );
    assert_eq!(again.row.id, first.row.id);

    let nia = worker(&db, fx.org, "Nia", "active").await;
    assert!(
        assignments::assign(&db, fx.org, nia, &at(fx.shift_lead, Some(fx.store)))
            .await
            .expect("an active worker is rostered")
            .created
    );

    let suspended = worker(&db, fx.org, "Sam", "suspended").await;
    assert!(matches!(
        assignments::assign(&db, fx.org, suspended, &at(fx.shift_lead, Some(fx.store))).await,
        Err(AssignError::NoStanding)
    ));

    let stranger = user(&db, "Stranger", Some("s@example.com".into())).await;
    assert!(matches!(
        assignments::assign(&db, fx.org, stranger, &at(fx.shift_lead, Some(fx.store))).await,
        Err(AssignError::NoStanding)
    ));

    // The other org's member has no standing here either.
    let foreign = member(&db, fx.other_org, "Foreign").await;
    assert!(matches!(
        assignments::assign(&db, fx.org, foreign, &at(fx.shift_lead, Some(fx.store))).await,
        Err(AssignError::NoStanding)
    ));
}

#[tokio::test]
async fn the_scope_rule_and_the_org_boundary_are_checked_before_the_person() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let fx = seed(&db).await;

    // A location-scope position needs a place; an org-wide one refuses one.
    assert!(matches!(
        assignments::validate_targets(&db, fx.org, &at(fx.shift_lead, None)).await,
        Err(AssignError::LocationRequired)
    ));
    assert!(matches!(
        assignments::validate_targets(&db, fx.org, &at(fx.area_manager, Some(fx.store))).await,
        Err(AssignError::LocationForbidden)
    ));
    // Another org's place, another org's position: not found, not confirmed.
    assert!(matches!(
        assignments::validate_targets(&db, fx.org, &at(fx.shift_lead, Some(fx.other_store))).await,
        Err(AssignError::NoSuchLocation)
    ));
    let foreign_role = role(&db, fx.other_org, "Shift Lead", "location").await;
    assert!(matches!(
        assignments::validate_targets(&db, fx.org, &at(foreign_role, Some(fx.store))).await,
        Err(AssignError::NoSuchRole)
    ));
    // A supervisor must have standing too.
    let ghost = Uuid::new_v4();
    let spec = AssignmentSpec {
        role_id: fx.shift_lead,
        location_id: Some(fx.store),
        supervisor_id: Some(ghost),
    };
    assert!(matches!(
        assignments::validate_targets(&db, fx.org, &spec).await,
        Err(AssignError::NoSuchSupervisor)
    ));
    // And the happy shapes pass.
    assignments::validate_targets(&db, fx.org, &at(fx.shift_lead, Some(fx.store)))
        .await
        .expect("a position at its place");
    assignments::validate_targets(&db, fx.org, &at(fx.area_manager, None))
        .await
        .expect("an org-wide position with no place");
}

#[tokio::test]
async fn the_roster_reads_back_with_names_and_a_removed_row_is_gone() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let fx = seed(&db).await;
    let boss = member(&db, fx.org, "Boss").await;
    let nia = worker(&db, fx.org, "Nia", "active").await;
    let spec = AssignmentSpec {
        role_id: fx.shift_lead,
        location_id: Some(fx.store),
        supervisor_id: Some(boss),
    };
    let held = assignments::assign(&db, fx.org, nia, &spec)
        .await
        .expect("rostered");
    assignments::assign(&db, fx.org, boss, &at(fx.area_manager, None))
        .await
        .expect("org-wide");

    let rows = assignments::rows(&db, fx.org, &AssignmentsQuery::default())
        .await
        .expect("rows");
    assert_eq!(rows.len(), 2);
    let nia_row = rows.iter().find(|r| r.user_id == nia).expect("nia's row");
    assert_eq!(nia_row.user_name, "Nia");
    assert_eq!(nia_row.user_kind, "frontline");
    assert_eq!(nia_row.role_name, "Shift Lead");
    assert_eq!(nia_row.location_name.as_deref(), Some("Clovis"));
    assert_eq!(nia_row.supervisor_name.as_deref(), Some("Boss"));
    let boss_row = rows.iter().find(|r| r.user_id == boss).expect("boss's row");
    assert_eq!(boss_row.user_kind, "member");
    assert!(boss_row.location_id.is_none());

    // Narrowed to the store: only the store's row.
    let at_store = assignments::rows(
        &db,
        fx.org,
        &AssignmentsQuery {
            user_id: None,
            location_id: Some(fx.store),
        },
    )
    .await
    .expect("rows");
    assert_eq!(at_store.len(), 1);

    // Removing from the other org finds nothing; from this org, it is gone.
    assert!(
        assignments::remove(&db, fx.other_org, held.row.id)
            .await
            .expect("query")
            .is_none()
    );
    assert!(
        assignments::remove(&db, fx.org, held.row.id)
            .await
            .expect("query")
            .is_some()
    );
    let by_user = assignments::by_user(&db, fx.org).await.expect("grouped");
    assert!(!by_user.contains_key(&nia));
    assert_eq!(by_user.get(&boss).map(Vec::len), Some(1));
}

#[tokio::test]
async fn the_hierarchy_refuses_a_loop_and_a_foreign_parent() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let fx = seed(&db).await;
    let region = location(&db, fx.org, "West").await;
    let district = location(&db, fx.org, "Central Valley").await;

    let patch = |parent: Option<Uuid>| UpdateLocation {
        parent_id: Some(parent),
        ..Default::default()
    };
    places::update_location(&db, fx.org, district, patch(Some(region)))
        .await
        .expect("district under region");
    places::update_location(&db, fx.org, fx.store, patch(Some(district)))
        .await
        .expect("store under district");

    // Closing the loop from the top is refused; so is being one's own parent.
    assert!(matches!(
        places::update_location(&db, fx.org, region, patch(Some(fx.store))).await,
        Err(places::LocationError::Cycle)
    ));
    assert!(matches!(
        places::update_location(&db, fx.org, region, patch(Some(region))).await,
        Err(places::LocationError::Cycle)
    ));
    // Another org's place is not a parent here.
    assert!(matches!(
        places::update_location(&db, fx.org, region, patch(Some(fx.other_store))).await,
        Err(places::LocationError::NoSuchParent)
    ));
    // Detaching works, and the kind is a lowercase word.
    let detached = places::update_location(
        &db,
        fx.org,
        fx.store,
        UpdateLocation {
            parent_id: Some(None),
            kind: Some(Some("  Store ".into())),
            ..Default::default()
        },
    )
    .await
    .expect("detach");
    assert!(detached.parent_id.is_none());
    assert_eq!(detached.kind.as_deref(), Some("store"));
    // A bad zone and an unknown status are refused before the database sees them.
    assert!(matches!(
        places::update_location(
            &db,
            fx.org,
            fx.store,
            UpdateLocation {
                timezone: Some("Mars/Olympus".into()),
                ..Default::default()
            }
        )
        .await,
        Err(places::LocationError::BadTimezone)
    ));
    assert!(matches!(
        places::update_location(
            &db,
            fx.org,
            fx.store,
            UpdateLocation {
                status: Some("closed".into()),
                ..Default::default()
            }
        )
        .await,
        Err(places::LocationError::BadStatus)
    ));
}

#[tokio::test]
async fn the_tenants_own_id_is_patchable_and_one_per_org() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let fx = seed(&db).await;
    let second = location(&db, fx.org, "Fresno").await;
    let patch = |value: Option<&str>| UpdateLocation {
        external_id: Some(value.map(str::to_string)),
        ..Default::default()
    };

    // Stored trimmed, and read back on the row every client sees.
    let row = places::update_location(&db, fx.org, fx.store, patch(Some("  santa-clara ")))
        .await
        .expect("set");
    assert_eq!(row.external_id.as_deref(), Some("santa-clara"));
    let rows = places::location_rows(&db, fx.org).await.expect("rows");
    let store = rows.iter().find(|r| r.id == fx.store).expect("store row");
    assert_eq!(store.external_id.as_deref(), Some("santa-clara"));

    // Another place of the org may not take it; the same place may re-send it;
    // another org's registry is that org's business.
    assert!(matches!(
        places::update_location(&db, fx.org, second, patch(Some("santa-clara"))).await,
        Err(places::LocationError::ExternalIdTaken)
    ));
    places::update_location(&db, fx.org, fx.store, patch(Some("santa-clara")))
        .await
        .expect("its own id is not a collision");
    places::update_location(
        &db,
        fx.other_org,
        fx.other_store,
        patch(Some("santa-clara")),
    )
    .await
    .expect("another org may reuse the id");

    // Blank clears it, as null does; an absent field leaves it alone.
    let cleared = places::update_location(&db, fx.org, fx.store, patch(Some("   ")))
        .await
        .expect("blank clears");
    assert!(cleared.external_id.is_none());
    places::update_location(&db, fx.org, fx.store, patch(Some("santa-clara")))
        .await
        .expect("set again");
    let renamed = places::update_location(
        &db,
        fx.org,
        fx.store,
        UpdateLocation {
            name: Some("Clovis North".into()),
            ..Default::default()
        },
    )
    .await
    .expect("rename");
    assert_eq!(renamed.external_id.as_deref(), Some("santa-clara"));
    let nulled = places::update_location(&db, fx.org, fx.store, patch(None))
        .await
        .expect("null clears");
    assert!(nulled.external_id.is_none());
    // Freed, the id may move to the other place.
    places::update_location(&db, fx.org, second, patch(Some("santa-clara")))
        .await
        .expect("freed id moves");
}

#[tokio::test]
async fn an_external_id_names_one_place_per_system_per_org() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let fx = seed(&db).await;
    let second = location(&db, fx.org, "Fresno").await;

    places::set_external_id(&db, fx.org, fx.store, "toast", " guid-1 ", None)
        .await
        .expect("mapped");
    // Re-mapping is an upsert, not a conflict.
    let remapped = places::set_external_id(&db, fx.org, fx.store, "toast", "guid-2", None)
        .await
        .expect("re-mapped");
    assert_eq!(remapped.external_id, "guid-2");
    // The same id on another place of the org is refused.
    assert!(matches!(
        places::set_external_id(&db, fx.org, second, "toast", "guid-2", None).await,
        Err(places::LocationError::ExternalIdTaken)
    ));
    // But the same id in another org's registry is that org's business.
    places::set_external_id(&db, fx.other_org, fx.other_store, "toast", "guid-2", None)
        .await
        .expect("another org may reuse the id");
    // Shape rules.
    assert!(matches!(
        places::set_external_id(&db, fx.org, second, "Toast", "x", None).await,
        Err(places::LocationError::BadSystem)
    ));
    assert!(matches!(
        places::set_external_id(&db, fx.org, second, "toast", "   ", None).await,
        Err(places::LocationError::BadExternalId)
    ));
    assert!(matches!(
        places::set_external_id(&db, fx.org, fx.other_store, "toast", "x", None).await,
        Err(places::LocationError::NotFound)
    ));

    // The registry read carries the map, and a removed id is gone.
    let rows = places::location_rows(&db, fx.org).await.expect("rows");
    let clovis = rows.iter().find(|r| r.id == fx.store).expect("clovis");
    assert_eq!(
        clovis.external_ids.get("toast").map(String::as_str),
        Some("guid-2")
    );
    assert!(
        places::remove_external_id(&db, fx.org, fx.store, "toast")
            .await
            .expect("removed")
    );
    assert!(
        !places::remove_external_id(&db, fx.org, fx.store, "toast")
            .await
            .expect("nothing to remove")
    );
}

#[tokio::test]
async fn a_held_position_cannot_be_deleted_and_a_rename_keeps_names_unique() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let fx = seed(&db).await;
    let maria = member(&db, fx.org, "Maria").await;
    assignments::assign(&db, fx.org, maria, &at(fx.shift_lead, Some(fx.store)))
        .await
        .expect("rostered");

    assert!(matches!(
        positions::delete_role(&db, fx.org, fx.shift_lead).await,
        Err(positions::RoleError::Held(1))
    ));
    positions::delete_role(&db, fx.org, fx.area_manager)
        .await
        .expect("an unheld position goes");
    assert!(matches!(
        positions::rename_role(&db, fx.org, fx.shift_lead, "   ").await,
        Err(positions::RoleError::BadName)
    ));
    let cook = role(&db, fx.org, "Cook", "location").await;
    assert!(matches!(
        positions::rename_role(&db, fx.org, cook, "Shift Lead").await,
        Err(positions::RoleError::NameTaken)
    ));
    assert!(matches!(
        positions::rename_role(&db, fx.other_org, cook, "Chef").await,
        Err(positions::RoleError::NotFound)
    ));
    assert_eq!(
        positions::rename_role(&db, fx.org, cook, "Line Cook")
            .await
            .expect("renamed")
            .name,
        "Line Cook"
    );
}

#[tokio::test]
async fn a_function_reads_the_registry_whole_and_the_roster_by_audience() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let fx = seed(&db).await;
    let maria = member(&db, fx.org, "Maria").await;
    let granted = worker(&db, fx.org, "Nia", "active").await;
    let ungranted = worker(&db, fx.org, "Sam", "active").await;
    app_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        app_id: ActiveValue::Set(fx.app),
        user_id: ActiveValue::Set(granted),
        role: ActiveValue::Set("member".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("grant");
    for who in [maria, granted, ungranted] {
        assignments::assign(&db, fx.org, who, &at(fx.shift_lead, Some(fx.store)))
            .await
            .expect("rostered");
    }
    places::set_external_id(&db, fx.org, fx.store, "toast", "guid-1", None)
        .await
        .expect("mapped");

    // places(): the whole registry, this org's only, with the map.
    let v = org_places_directory(&db, fx.org).await.expect("places");
    assert_eq!(v["total"], 1);
    assert_eq!(v["places"][0]["name"], "Clovis");
    assert_eq!(v["places"][0]["kind"], "store");
    assert_eq!(v["places"][0]["external_ids"]["toast"], "guid-1");

    // assignments(): the member and the granted worker; not the ungranted one.
    let v = org_assignments_directory(&db, fx.org, fx.app)
        .await
        .expect("assignments");
    let names: Vec<&str> = v["assignments"]
        .as_array()
        .expect("array")
        .iter()
        .map(|a| a["user_name"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(v["total"], 2, "{names:?}");
    assert!(names.contains(&"Maria") && names.contains(&"Nia"));
    assert!(
        !names.contains(&"Sam"),
        "an ungranted worker is not this app's to name"
    );
}

#[tokio::test]
async fn reach_follows_the_assignments_and_the_standing() {
    use oxy_app::server::api::operating_graph::reach::{Caller, Reach, load_reach};
    let (db, _url) = fresh_db(Schema::Central).await;
    let fx = seed(&db).await;
    let office = member(&db, fx.org, "Office").await;
    let nia = worker(&db, fx.org, "Nia", "active").await;
    let boss = member(&db, fx.org, "Boss").await;
    let as_member = Caller {
        system: false,
        app_admin: false,
        member: true,
    };
    let as_worker = Caller {
        member: false,
        ..as_member
    };

    // Unassigned: the office reaches everywhere, the crew nowhere.
    assert_eq!(
        load_reach(&db, fx.org, office, as_member)
            .await
            .expect("reach"),
        Reach::everywhere("org-member", vec![])
    );
    assert_eq!(
        load_reach(&db, fx.org, nia, as_worker)
            .await
            .expect("reach"),
        Reach::nowhere()
    );

    // Assigned at Clovis: exactly Clovis, for either standing.
    assignments::assign(&db, fx.org, nia, &at(fx.shift_lead, Some(fx.store)))
        .await
        .expect("rostered");
    assignments::assign(&db, fx.org, office, &at(fx.shift_lead, Some(fx.store)))
        .await
        .expect("rostered");
    for (who, caller) in [(nia, as_worker), (office, as_member)] {
        let r = load_reach(&db, fx.org, who, caller).await.expect("reach");
        assert!(!r.everywhere);
        assert_eq!(r.locations, vec![fx.store]);
    }

    // An org-wide position reaches everywhere and still names the store.
    assignments::assign(&db, fx.org, boss, &at(fx.area_manager, None))
        .await
        .expect("org-wide");
    assignments::assign(&db, fx.org, boss, &at(fx.shift_lead, Some(fx.store)))
        .await
        .expect("also at the store");
    let r = load_reach(&db, fx.org, boss, as_member)
        .await
        .expect("reach");
    assert_eq!(r.via, Some("org-wide-position"));
    assert_eq!(r.locations, vec![fx.store]);

    // Another org's assignments are not this org's reach.
    assert_eq!(
        load_reach(&db, fx.other_org, nia, as_worker)
            .await
            .expect("reach"),
        Reach::nowhere()
    );
    // The system reads nothing and reaches everywhere.
    let system = Caller {
        system: true,
        ..as_worker
    };
    assert_eq!(
        load_reach(&db, fx.org, nia, system).await.expect("reach"),
        Reach::everywhere("system", vec![])
    );
}

#[tokio::test]
async fn a_bound_view_resolves_keys_to_places_and_scopes_a_query_to_reach() {
    use oxy_app::server::api::operating_graph::binding::{
        NO_REACH_SENTINEL, ScopeError, apply_reach_scope, external_ids_for_locations,
        locations_by_external_ids,
    };
    use oxy_app::server::api::operating_graph::reach::Reach;
    let (db, _url) = fresh_db(Schema::Central).await;
    let fx = seed(&db).await;
    let fresno = location(&db, fx.org, "Fresno").await;
    places::set_external_id(&db, fx.org, fx.store, "toast", "guid-clovis", None)
        .await
        .expect("mapped");
    places::set_external_id(&db, fx.org, fresno, "toast", "guid-fresno", None)
        .await
        .expect("mapped");

    // key → place, unknown keys absent, another org's registry invisible.
    let found = locations_by_external_ids(
        &db,
        fx.org,
        "toast",
        &["guid-clovis".into(), "guid-nobody".into()],
    )
    .await
    .expect("lookup");
    assert_eq!(found.len(), 1);
    assert_eq!(found["guid-clovis"].name, "Clovis");
    assert_eq!(found["guid-clovis"].kind.as_deref(), Some("store"));
    assert!(
        locations_by_external_ids(&db, fx.other_org, "toast", &["guid-clovis".into()])
            .await
            .expect("lookup")
            .is_empty()
    );

    // places → keys, sorted; a place with no key in the system contributes nothing.
    let unmapped = location(&db, fx.org, "Reedley").await;
    assert_eq!(
        external_ids_for_locations(&db, fx.org, "toast", &[fresno, fx.store, unmapped])
            .await
            .expect("keys"),
        vec!["guid-clovis".to_string(), "guid-fresno".to_string()]
    );

    // A layer with one bound view and one plain view.
    let sales = oxy_airlayer_compat::parse_view_yaml(
        "name: sales\ntable: sales\nentities:\n  - name: store\n    type: primary\n    key: restaurant_id\n    binding: { registry: locations, system: toast }\nmeasures:\n  - name: total\n    type: sum\n    sql: amount\n",
    )
    .expect("bound view");
    let weather = oxy_airlayer_compat::parse_view_yaml(
        "name: weather\ntable: weather\nmeasures:\n  - name: temp\n    type: average\n    sql: temp\n",
    )
    .expect("plain view");
    let layer = oxy_airlayer_compat::SemanticLayer {
        views: vec![sales, weather],
        topics: None,
        motifs: None,
        saved_queries: None,
        metadata: None,
    };
    let query = |members: &[&str]| -> agentic_semantic::config::SemanticQueryConfig {
        serde_json::from_value(serde_json::json!({ "topic": "sales", "measures": members }))
            .unwrap()
    };

    // Scoped to Clovis: one `in` filter on the bound key with Clovis's GUID.
    let scoped = Reach {
        everywhere: false,
        via: None,
        locations: vec![fx.store],
    };
    let mut q = query(&["sales.total"]);
    apply_reach_scope(&db, fx.org, &layer, &scoped, &mut q)
        .await
        .expect("scoped");
    assert_eq!(q.filters.len(), 1);
    assert_eq!(q.filters[0].field, "sales.restaurant_id");
    let json = serde_json::to_value(&q.filters[0]).unwrap();
    assert_eq!(json["op"], "in");
    assert_eq!(json["values"], serde_json::json!(["guid-clovis"]));

    // Scoped to an unmapped place: a list that names nothing.
    let nowhere_mapped = Reach {
        everywhere: false,
        via: None,
        locations: vec![unmapped],
    };
    let mut q = query(&["sales.total"]);
    apply_reach_scope(&db, fx.org, &layer, &nowhere_mapped, &mut q)
        .await
        .expect("scoped");
    let json = serde_json::to_value(&q.filters[0]).unwrap();
    assert_eq!(json["values"], serde_json::json!([NO_REACH_SENTINEL]));

    // Everywhere: untouched. Naming no bound view: refused, for everyone.
    let everywhere = Reach::everywhere("org-member", vec![]);
    let mut q = query(&["sales.total"]);
    apply_reach_scope(&db, fx.org, &layer, &everywhere, &mut q)
        .await
        .expect("everywhere");
    assert!(q.filters.is_empty());
    for reach in [&everywhere, &scoped] {
        let mut q = query(&["weather.temp"]);
        assert!(matches!(
            apply_reach_scope(&db, fx.org, &layer, reach, &mut q).await,
            Err(ScopeError::NoBoundView)
        ));
    }
}

#[tokio::test]
async fn review_round_two_a_taken_name_a_changed_supervisor_and_a_supervisor_outside_the_audience()
{
    let (db, _url) = fresh_db(Schema::Central).await;
    let fx = seed(&db).await;

    // A taken name answers 409-shaped, not 500-shaped.
    let fresno = location(&db, fx.org, "Fresno").await;
    assert!(matches!(
        places::update_location(
            &db,
            fx.org,
            fresno,
            UpdateLocation {
                name: Some("Clovis".into()),
                ..Default::default()
            }
        )
        .await,
        Err(places::LocationError::NameTaken)
    ));

    // Re-posting the same position with a different supervisor updates it,
    // and re-posting it identically moves nothing.
    let boss = member(&db, fx.org, "Boss").await;
    let other_boss = member(&db, fx.org, "Other Boss").await;
    let nia = worker(&db, fx.org, "Nia", "active").await;
    let spec = AssignmentSpec {
        role_id: fx.shift_lead,
        location_id: Some(fx.store),
        supervisor_id: Some(boss),
    };
    let first = assignments::assign(&db, fx.org, nia, &spec)
        .await
        .expect("rostered");
    assert!(first.created && !first.updated);
    let moved = assignments::assign(
        &db,
        fx.org,
        nia,
        &AssignmentSpec {
            supervisor_id: Some(other_boss),
            ..spec.clone()
        },
    )
    .await
    .expect("supervisor changed");
    assert!(!moved.created && moved.updated);
    assert_eq!(moved.row.id, first.row.id);
    assert_eq!(moved.row.supervisor_id, Some(other_boss));
    let same = assignments::assign(
        &db,
        fx.org,
        nia,
        &AssignmentSpec {
            supervisor_id: Some(other_boss),
            ..spec.clone()
        },
    )
    .await
    .expect("identical re-post");
    assert!(!same.created && !same.updated);

    // `ctx.org.assignments()` withholds a supervisor the app may not name.
    // Other Boss is a member (in the audience); a granted worker's row names
    // them. Then a supervisor who is an ungranted WORKER is withheld.
    app_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        app_id: ActiveValue::Set(fx.app),
        user_id: ActiveValue::Set(nia),
        role: ActiveValue::Set("member".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("grant");
    let v = org_assignments_directory(&db, fx.org, fx.app)
        .await
        .expect("assignments");
    let row = &v["assignments"][0];
    assert_eq!(row["user_name"], "Nia");
    assert_eq!(row["supervisor_name"], "Other Boss");
    let lead = worker(&db, fx.org, "Lead Nobody Granted", "active").await;
    assignments::assign(
        &db,
        fx.org,
        nia,
        &AssignmentSpec {
            supervisor_id: Some(lead),
            ..spec.clone()
        },
    )
    .await
    .expect("supervisor changed to an ungranted worker");
    let v = org_assignments_directory(&db, fx.org, fx.app)
        .await
        .expect("assignments");
    let row = &v["assignments"][0];
    assert_eq!(row["user_name"], "Nia");
    assert_eq!(row["supervisor_id"], serde_json::Value::Null);
    assert_eq!(row["supervisor_name"], serde_json::Value::Null);

    // And `people()` is built on the same audience: Nia is named, Lead is not.
    let people =
        oxy_app::server::api::custom_apps_functions::host::org_directory(&db, fx.org, fx.app)
            .await
            .expect("people");
    let names: Vec<&str> = people["people"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"Nia") && names.contains(&"Boss"));
    assert!(!names.contains(&"Lead Nobody Granted"));
}
