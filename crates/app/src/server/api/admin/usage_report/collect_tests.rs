use super::super::model::fixtures::period;
use super::*;
use chrono::TimeZone;

fn catalog_row(app: u128, org: u128, name: &str, published: bool) -> CatalogRow {
    CatalogRow {
        app_id: Uuid::from_u128(app),
        org_id: Uuid::from_u128(org),
        name: name.to_string(),
        slug: name.to_lowercase(),
        published_at: published.then(|| Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap()),
        org_name: format!("Org {org}"),
        org_slug: format!("org-{org}"),
    }
}

fn viewed(app: u128, people: i64, prev_people: i64) -> ViewRow {
    ViewRow {
        app_id: Uuid::from_u128(app),
        views: people * 3,
        people,
        sessions: people,
        prev_views: prev_people * 3,
        prev_people,
        prev_sessions: prev_people,
    }
}

#[test]
fn a_live_app_nobody_opened_is_in_the_report_and_an_idle_draft_is_not() {
    let report = assemble(
        period(),
        vec![
            catalog_row(10, 1, "Live", true),
            catalog_row(11, 1, "Draft", false),
        ],
        Raw::default(),
    );
    let names: Vec<&str> = report.orgs[0]
        .apps
        .iter()
        .map(|a| a.name.as_str())
        .collect();
    assert_eq!(names, ["Live"]);
    assert_eq!(report.orgs[0].apps[0].current, Counts::default());
}

#[test]
fn an_unpublished_app_that_was_used_is_reported() {
    let raw = Raw {
        functions: vec![FunctionRow {
            app_id: Uuid::from_u128(11),
            prev_calls: 4,
            ..FunctionRow::default()
        }],
        ..Raw::default()
    };
    let report = assemble(period(), vec![catalog_row(11, 1, "Draft", false)], raw);
    assert_eq!(report.orgs[0].apps[0].previous.function_calls, 4);
}

#[test]
fn an_org_with_nothing_to_report_is_left_out() {
    let report = assemble(
        period(),
        vec![catalog_row(11, 1, "Draft", false)],
        Raw::default(),
    );
    assert!(report.orgs.is_empty());
}

#[test]
fn org_people_come_from_the_org_row_not_from_adding_its_apps() {
    // The same four people opened both apps.
    let raw = Raw {
        views: vec![viewed(10, 4, 0), viewed(11, 4, 0)],
        org_people: vec![OrgPeopleRow {
            org_id: Uuid::from_u128(1),
            people: 4,
            prev_people: 0,
        }],
        ..Raw::default()
    };
    let report = assemble(
        period(),
        vec![catalog_row(10, 1, "A", true), catalog_row(11, 1, "B", true)],
        raw,
    );
    assert_eq!(report.orgs[0].people, 4);
    assert_eq!(report.summary().people, 4);
    assert_eq!(report.orgs[0].views(), 24);
}

#[test]
fn a_first_week_is_an_app_opened_now_and_never_before() {
    let raw = Raw {
        views: vec![viewed(10, 3, 0), viewed(11, 3, 0), viewed(12, 3, 2)],
        seen_before: HashSet::from([Uuid::from_u128(11)]),
        ..Raw::default()
    };
    let report = assemble(
        period(),
        vec![
            catalog_row(10, 1, "New", true),
            catalog_row(11, 1, "Returning", true),
            catalog_row(12, 1, "Steady", true),
        ],
        raw,
    );
    let first: Vec<(&str, bool)> = report.orgs[0]
        .apps
        .iter()
        .map(|a| (a.name.as_str(), a.first_week))
        .collect();
    assert_eq!(
        first,
        [("New", true), ("Returning", false), ("Steady", false)]
    );
}

#[test]
fn orgs_and_apps_are_listed_busiest_first() {
    let raw = Raw {
        views: vec![viewed(10, 1, 0), viewed(11, 5, 0), viewed(20, 9, 0)],
        org_people: vec![
            OrgPeopleRow {
                org_id: Uuid::from_u128(1),
                people: 6,
                prev_people: 0,
            },
            OrgPeopleRow {
                org_id: Uuid::from_u128(2),
                people: 9,
                prev_people: 0,
            },
        ],
        ..Raw::default()
    };
    let report = assemble(
        period(),
        vec![
            catalog_row(10, 1, "Small", true),
            catalog_row(11, 1, "Big", true),
            catalog_row(20, 2, "Other", true),
        ],
        raw,
    );
    let orgs: Vec<&str> = report.orgs.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(orgs, ["Org 2", "Org 1"]);
    let apps: Vec<&str> = report.orgs[1]
        .apps
        .iter()
        .map(|a| a.name.as_str())
        .collect();
    assert_eq!(apps, ["Big", "Small"]);
}

#[test]
fn releases_and_sizes_land_on_their_app_and_an_unmeasured_app_stays_unmeasured() {
    let size = |app: u128, bytes: i64| StorageRow {
        app_id: Uuid::from_u128(app),
        bytes,
    };
    let raw = Raw {
        releases: vec![ReleaseRow {
            app_id: Uuid::from_u128(10),
            releases: 2,
            prev_releases: 1,
        }],
        storage: vec![size(10, 900), size(11, 0)],
        storage_before: vec![size(10, 400)],
        ..Raw::default()
    };
    let report = assemble(
        period(),
        vec![
            catalog_row(10, 1, "Shipping", true),
            catalog_row(11, 1, "Empty", true),
            catalog_row(12, 1, "Unmeasured", true),
        ],
        raw,
    );
    let by_name = |name: &str| {
        report.orgs[0]
            .apps
            .iter()
            .find(|a| a.name == name)
            .unwrap()
            .clone()
    };
    let shipping = by_name("Shipping");
    assert_eq!(
        (shipping.current.releases, shipping.previous.releases),
        (2, 1)
    );
    assert_eq!(
        (shipping.storage_bytes, shipping.storage_bytes_before),
        (Some(900), Some(400))
    );
    // Measured and empty is a size; never measured is not.
    assert_eq!(by_name("Empty").storage_bytes, Some(0));
    assert_eq!(by_name("Empty").storage_bytes_before, None);
    assert_eq!(by_name("Unmeasured").storage_bytes, None);
}

#[test]
fn a_draft_that_shipped_a_release_is_reported() {
    let raw = Raw {
        releases: vec![ReleaseRow {
            app_id: Uuid::from_u128(11),
            releases: 1,
            prev_releases: 0,
        }],
        ..Raw::default()
    };
    let report = assemble(period(), vec![catalog_row(11, 1, "Draft", false)], raw);
    assert_eq!(report.orgs[0].apps[0].current.releases, 1);
}

#[test]
fn a_negative_count_reads_as_zero() {
    assert_eq!(n(-1), 0);
    assert_eq!(n(7), 7);
}
