//! What a partner reads of a managed org's audit trail
//! (`GET /partners/{id}/audit` → `audit::events_for_partner`).
//!
//! A partner has no reach into an app's non-production environments, so the
//! rows written there are not the partner's to read: a staging or sandbox
//! invocation's held writes carry the Oxy staff actor's email and the write's
//! target. Production's rows in the same org stay the partner's business,
//! and a page is cut from them — the filter is in the query.

use oxy_app_core::audit::{AuditEntry, events_for_partner, record_best_effort};
use serde_json::json;
use uuid::Uuid;

use crate::custom_app_functions_fixture::{Tenant, seeded_tenant, throwaway_org};

/// One audit row in `org`, written by `actor`, in `environment`.
async fn audit_row(
    t: &Tenant,
    org: Uuid,
    actor: &str,
    action: &'static str,
    environment: &str,
    target: &str,
) {
    let entry = AuditEntry::new(actor.to_string(), action)
        .org(org)
        .environment(environment)
        .target("table", "orders", target)
        .metadata(json!({ "invocation_id": Uuid::new_v4() }));
    record_best_effort(&t.db, entry).await;
}

#[tokio::test]
async fn a_partner_reads_productions_audit_rows_and_no_non_production_environments() {
    let t = seeded_tenant().await;
    // A fresh org: its audit rows are this test's alone.
    let org = throwaway_org(&t).await.org_id;
    let partner = Uuid::new_v4();
    let (client, staff) = ("admin@customer.example", "engineer@oxy.tech");

    // Oldest first. The newest rows are all non-production.
    audit_row(&t, org, client, "app.oltp.write", "production", "first").await;
    audit_row(
        &t,
        org,
        staff,
        "app.staging.held",
        "staging",
        "insert orders",
    )
    .await;
    audit_row(
        &t,
        org,
        client,
        "app.warehouse.write",
        "production",
        "second",
    )
    .await;
    audit_row(
        &t,
        org,
        staff,
        "app.staging.held",
        "dev-a1",
        "insert orders",
    )
    .await;
    audit_row(&t, org, staff, "app.oltp.write", "staging", "insert orders").await;

    let read = |limit: u64, offset: u64| {
        let db = t.db.clone();
        async move {
            events_for_partner(&db, partner, &[org], limit, offset)
                .await
                .expect("events_for_partner")
        }
    };

    let all = read(50, 0).await;
    let labels: Vec<_> = all.iter().filter_map(|e| e.target_label.clone()).collect();
    assert_eq!(
        labels,
        ["second", "first"],
        "production's rows, newest first, and no row of staging or a sandbox"
    );
    for row in &all {
        assert_eq!(row.environment, "production", "{row:?}");
        assert_ne!(
            row.actor_email, staff,
            "staff's non-production work: {row:?}"
        );
    }

    // A page is `limit` rows the partner may read: the filter runs before the
    // offset walk, so the non-production rows at the head cost it nothing.
    let page = |rows: Vec<entity::audit_events::Model>| -> Vec<String> {
        rows.into_iter().filter_map(|e| e.target_label).collect()
    };
    assert_eq!(page(read(1, 0).await), ["second"]);
    assert_eq!(page(read(1, 1).await), ["first"]);
    assert!(read(1, 2).await.is_empty());
}
