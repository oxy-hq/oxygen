//! The branch as an outbox store: its steps, its grants, what it vouches for,
//! and its mark.

use entity::app_event_outbox_marks;
use oxy_oltp::OltpBranch;
use oxy_oltp::entity::branches::{self as oltp_branches, Entity as OltpBranches};
use oxy_oltp::events::{self as outbox, OutboxTarget};
use oxy_oltp::grant_audit::{GrantScope, role_findings_sql};
use oxy_oltp::platform::PLATFORM_SCHEMA_VERSION;
use oxy_oltp::resolver::resolve_branch_writer_for_org;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::json;

use super::{Fx, OK_JS, with_fx};

impl Fx {
    /// The org's staging branch row.
    async fn branch_row(&self) -> oltp_branches::Model {
        let tenant = oxy_oltp::migrator::tenant_for_org(&self.t.db, self.org())
            .await
            .expect("the org's tenant");
        OltpBranches::find()
            .filter(oltp_branches::Column::TenantRowId.eq(tenant.id))
            .filter(oltp_branches::Column::Kind.eq(OltpBranch::Staging))
            .one(&self.t.db)
            .await
            .expect("read the branch row")
            .expect("the staging branch row")
    }

    /// Put the branch back as one cut before the outbox existed: no
    /// `oxy_events` in it, and its row recording version 4.
    async fn cut_before_step_5(&self) {
        self.branch_owner()
            .await
            .batch_execute("DROP SCHEMA oxy_events CASCADE")
            .await
            .expect("drop the branch's outbox");
        self.platform(
            "UPDATE oltp_branches SET platform_schema_version = 4 WHERE id = $1",
            vec![self.branch_row().await.id.into()],
        )
        .await;
        assert!(!self.branch_has_outbox().await);
    }

    async fn branch_has_outbox(&self) -> bool {
        self.branch_owner()
            .await
            .query_one("SELECT to_regclass('oxy_events.outbox') IS NOT NULL", &[])
            .await
            .expect("look for the outbox")
            .get(0)
    }

    /// The publisher's writer role and schema on the branch.
    async fn writer_on_branch(&self) -> (String, String) {
        let conn = resolve_branch_writer_for_org(
            &self.t.db,
            self.org(),
            OltpBranch::Staging,
            &self.store.writer,
        )
        .await
        .expect("resolve the writer on the branch")
        .expect("the org has a staging branch")
        .connection;
        (conn.role, conn.schema)
    }

    /// Whether `role` may use `oxy_events` in the database `client` is on.
    async fn may_publish(client: &tokio_postgres::Client, role: &str) -> bool {
        client
            .query_one(
                "SELECT has_schema_privilege($1, 'oxy_events', 'USAGE')",
                &[&role],
            )
            .await
            .expect("read the privilege")
            .get(0)
    }
}

/// A branch cut before platform step 5 has no outbox: the first grant on it
/// applies the steps after the version its row records, the first claim
/// does the same, and either records it current.
#[tokio::test]
async fn a_branch_cut_before_step_5_gets_the_outbox_lazily() {
    with_fx(|fx| async move {
        let staging = OutboxTarget::Branch("staging".into());
        let (role, _) = fx.writer_on_branch().await;

        fx.cut_before_step_5().await;
        outbox::ensure_publishable(&fx.t.db, fx.org(), &staging, &role)
            .await
            .expect("the first grant brings the branch current");
        assert!(
            fx.branch_has_outbox().await,
            "the grant's touch made the outbox"
        );
        assert_eq!(
            fx.branch_row().await.platform_schema_version,
            PLATFORM_SCHEMA_VERSION
        );
        fx.publish("staging", "k-1").await;

        fx.cut_before_step_5().await;
        let claim = outbox::claim(&fx.t.db, fx.org(), &staging, 10)
            .await
            .expect("the first claim brings the branch current");
        assert!(claim.rows().is_empty(), "a new outbox holds nothing");
        claim.release().await.expect("release");
        assert!(
            fx.branch_has_outbox().await,
            "the claim's touch made the outbox"
        );
        assert_eq!(
            fx.branch_row().await.platform_schema_version,
            PLATFORM_SCHEMA_VERSION
        );
    })
    .await;
}

/// R24: a grant on production's store is no grant on the branch; the branch
/// gets its own on its first publish there, which the grant audit accepts,
/// and a forgotten grant is made again there.
#[tokio::test]
async fn a_branch_store_gets_its_own_writer_grant() {
    with_fx(|fx| async move {
        let (role, schema) = fx.writer_on_branch().await;
        let db = &fx.t.db;
        outbox::ensure_publishable(db, fx.org(), &OutboxTarget::Production, &role)
            .await
            .expect("grant on production");
        let branch = fx.branch_owner().await;
        assert!(
            !Fx::may_publish(&branch, &role).await,
            "production's grant stays there"
        );

        let staging = OutboxTarget::Branch("staging".into());
        outbox::ensure_publishable(db, fx.org(), &staging, &role)
            .await
            .expect("grant on the branch");
        assert!(
            Fx::may_publish(&branch, &role).await,
            "the branch has its own grant"
        );
        let audit = role_findings_sql(&GrantScope::app_writer(&role, &schema)).expect("audit sql");
        let findings = branch.query(&audit, &[]).await.expect("run the audit");
        assert!(
            findings.is_empty(),
            "the audit stays green on the branch: {}",
            findings.len()
        );

        branch
            .batch_execute(&format!(
                "REVOKE USAGE ON SCHEMA oxy_events FROM \"{role}\""
            ))
            .await
            .expect("lose the grant");
        outbox::forget_publishable(fx.org(), &staging, &role);
        outbox::ensure_publishable(db, fx.org(), &staging, &role)
            .await
            .expect("grant again");
        assert!(
            Fx::may_publish(&branch, &role).await,
            "granted again on the branch"
        );
    })
    .await;
}

/// R12: the branch vouches for staging and its own sandboxes only. A row
/// there claiming production, or a sandbox the publisher has not got there,
/// is stored as staging's with a relay error and delivered to nobody.
#[tokio::test]
async fn a_branch_row_claiming_production_or_another_sandbox_is_quarantined() {
    with_fx(|fx| async move {
        let (listener, _) = fx.listener("br-l", OK_JS, json!({})).await;
        fx.switch_on(listener, "staging").await;
        fx.publish("production", "as-production").await;
        fx.publish("dev-zz", "as-another-sandbox").await;

        let stats = fx.relay("staging").await;
        assert_eq!((stats.relayed, stats.deliveries, stats.rejected), (0, 0, 2));
        let events = fx.events().await;
        assert_eq!(events.len(), 2, "{events:?}");
        for event in &events {
            assert_eq!(event.environment, "staging", "the branch's own: {event:?}");
            let error = event.relay_error.as_deref().expect("quarantined");
            assert!(error.contains("OLTP staging branch"), "{error}");
        }
        for claimed in ["`production`", "`dev-zz`"] {
            assert!(
                events.iter().any(|e| e
                    .relay_error
                    .as_deref()
                    .is_some_and(|r| r.contains(claimed))),
                "{claimed} is named: {events:?}"
            );
        }
        assert!(fx.deliveries().await.is_empty(), "delivered to nobody");
        assert_eq!(
            fx.relay("production").await,
            Default::default(),
            "never production's"
        );
    })
    .await;
}

/// R28 left no relayed staging marks (the flag was off everywhere), so none
/// needs a backfill; one with nothing waiting behind it drains quietly on
/// its first relay.
#[tokio::test]
async fn a_mark_with_no_outbox_rows_drains_quietly_on_its_first_relay() {
    with_fx(|fx| async move {
        fx.platform(
            "INSERT INTO app_event_outbox_marks (org_id, environment, published_at) \
             VALUES ($1, 'staging', now() - interval '1 day')",
            vec![fx.org().into()],
        )
        .await;

        assert_eq!(fx.relay("staging").await, Default::default());

        let mark = app_event_outbox_marks::Entity::find_by_id((fx.org(), "staging".to_string()))
            .one(&fx.t.db)
            .await
            .expect("read the mark")
            .expect("the mark");
        assert!(mark.drained_at.is_some(), "drained: {mark:?}");
        assert_eq!(mark.pending_since, None, "not waiting: {mark:?}");
        assert!(fx.events().await.is_empty());
    })
    .await;
}
