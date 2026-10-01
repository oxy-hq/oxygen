//! Publish refuses a `nonProduction.destinations` mapping it should not ship:
//! one whose staging database resolves to production's same host and user (a
//! heuristic — the message says so), one onto itself, one onto the
//! workspace's own Airhouse, one naming a database the workspace does not
//! configure, two MotherDuck entries on one token, a chain whose target is a
//! mapped production database, and a target that matches any production
//! database the block maps. A mapping to a separate credential publishes. The
//! host repeats the check on every staging write (`staging_destination_guards`).
//!
//! The workspace is compiled and promoted, as the serve fleet reads it, so the
//! check reads the same config the host resolves connectors from.

use oxy_app::server::api::custom_apps_nonproduction::MappingRefusal;
use oxy_app::server::api::custom_apps_publish::{OrgRef, PublishError, PublishInput, publish};
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use serde_json::json;
use uuid::Uuid;

use crate::custom_app_functions_fixture::{Tenant, seeded_tenant};
use crate::custom_app_functions_shape_zoo::{compile, write_config};
use crate::custom_apps_publish_function_artifacts::tar_gz;

const DATABASES: &str = "  - name: ch\n    type: clickhouse\n    host: https://ch.example.com\n    \
                         user: app\n    database: default\n  \
                         - name: ch_copy\n    type: clickhouse\n    host: ch.example.com\n    \
                         user: app\n    database: staging\n  \
                         - name: ch_staging\n    type: clickhouse\n    host: ch.example.com\n    \
                         user: app_staging\n    database: staging\n  \
                         - name: lake\n    type: airhouse_managed\n  \
                         - name: ch_two\n    type: clickhouse\n    host: ch2.example.com\n    \
                         user: app2\n    database: default\n  \
                         - name: ch_same_db\n    type: clickhouse\n    host: ch.example.com\n    \
                         user: app_readonly\n    database: default\n  \
                         - name: md_prod\n    type: motherduck\n    token_var: MD_TOKEN\n    \
                         database: prod\n  \
                         - name: md_staging\n    type: motherduck\n    token_var: MD_TOKEN\n    \
                         database: staging\n";

/// Publish app `slug` to staging with `nonProduction` as its mapping block.
async fn publish_mapping(
    t: &Tenant,
    workspace: Uuid,
    slug: &str,
    non_production: serde_json::Value,
) -> Result<(), PublishError> {
    let manifest = json!({
        "schemaVersion": 2,
        "slug": slug,
        "functions": { "write": { "route": true, "destinations": ["ch", "ch_copy", "ch_staging"] } },
        "nonProduction": non_production,
    })
    .to_string();
    let js = b"export default async () => Response.json({});";
    let tarball = tar_gz(&[
        (
            "index.html",
            b"<!doctype html><html><head><title>map</title></head><body></body></html>",
        ),
        ("oxy-app.json", manifest.as_bytes()),
        ("functions/write.js", js),
    ]);
    publish(PublishInput {
        org_ref: Some(OrgRef::Id(t.org_id)),
        app_slug: slug.to_string(),
        project_id: workspace,
        branch: None,
        build_id: format!("map-{}", &Uuid::new_v4().simple().to_string()[..8]),
        name: None,
        promote: false,
        tarball,
        manifest: None,
        source_repo: None,
        commit_sha: None,
        published_by: Some(t.guest_id),
        published_by_email: Some(LOCAL_GUEST_EMAIL.to_string()),
        machine_app_id: None,
        published_via: None,
        semantic_revision_id: None,
    })
    .await
    .map(|_| ())
}

fn author_refusal(outcome: Result<(), PublishError>) -> String {
    match outcome {
        Err(PublishError::DestinationMapping(MappingRefusal::Author(message))) => message,
        other => panic!("expected an author refusal of the mapping, got {other:?}"),
    }
}

#[tokio::test]
async fn publish_refuses_a_staging_destination_on_productions_credential() {
    let t = seeded_tenant().await;
    let root = write_config(DATABASES);
    let workspace = compile(&t, root.path()).await;

    let copied = author_refusal(
        publish_mapping(
            &t,
            workspace,
            "map-copy",
            json!({ "destinations": { "ch": "ch_copy" } }),
        )
        .await,
    );
    assert!(copied.contains("same host and user"), "{copied}");
    assert!(
        copied.contains("heuristic"),
        "says what the check is: {copied}"
    );

    let onto_itself = author_refusal(
        publish_mapping(
            &t,
            workspace,
            "map-self",
            json!({ "destinations": { "ch": "ch" } }),
        )
        .await,
    );
    assert!(onto_itself.contains("onto itself"), "{onto_itself}");

    let own_airhouse = author_refusal(
        publish_mapping(
            &t,
            workspace,
            "map-lake",
            json!({ "destinations": { "ch": "lake" } }),
        )
        .await,
    );
    assert!(own_airhouse.contains("airhouse_managed"), "{own_airhouse}");

    let missing = author_refusal(
        publish_mapping(
            &t,
            workspace,
            "map-missing",
            json!({ "destinations": { "ch": "nope" } }),
        )
        .await,
    );
    assert!(missing.contains("`nope`"), "{missing}");

    // B2: one MotherDuck token reaches every database of its account.
    let shared_token = author_refusal(
        publish_mapping(
            &t,
            workspace,
            "map-md",
            json!({ "destinations": { "md_prod": "md_staging" } }),
        )
        .await,
    );
    assert!(
        shared_token.contains("same host and user"),
        "{shared_token}"
    );

    // Fix round 2: production's database on production's host, under another
    // user, is still production's database.
    let same_database = author_refusal(
        publish_mapping(
            &t,
            workspace,
            "map-same-db",
            json!({ "destinations": { "ch": "ch_same_db" } }),
        )
        .await,
    );
    assert!(
        same_database.contains("same database on the same host"),
        "{same_database}"
    );

    // B3: a target that is itself a mapped production database.
    let chained = author_refusal(
        publish_mapping(
            &t,
            workspace,
            "map-chain",
            json!({ "destinations": { "ch": "ch_staging", "ch_staging": "ch_two" } }),
        )
        .await,
    );
    assert!(
        chained.contains("production database of its own"),
        "{chained}"
    );

    // B3: `ch_copy` is not `ch_two`, but it is production's `ch`.
    let another_key = author_refusal(
        publish_mapping(
            &t,
            workspace,
            "map-keys",
            json!({ "destinations": { "ch": "ch_staging", "ch_two": "ch_copy" } }),
        )
        .await,
    );
    assert!(
        another_key.contains("production database `ch`"),
        "{another_key}"
    );

    publish_mapping(
        &t,
        workspace,
        "map-separate",
        json!({ "destinations": { "ch": "ch_staging" } }),
    )
    .await
    .expect("a staging database on its own credential publishes");
}
