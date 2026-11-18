//! A publish into a sandbox's own branch after a reset wound it back (F2,
//! and F3 for a reset by a binary that does not know the platform version):
//! a reset copies production, which may still be at platform version 5 —
//! the unsigned definer alone, granted to the writer as a version-5 publish
//! left it — while this process still remembers its grant on the branch. The
//! host brings the branch current and publishes signed, outside a
//! transaction and on a `ctx.oltp.tx` handle, through the same process.

use oxy_oltp::entity::env_branches::{self, Entity as EnvBranches};
use oxy_oltp::platform::PLATFORM_SCHEMA_VERSION;

use super::own_branch::SANDBOX;
use super::*;

/// Which binary reset the branch, and so what its row says after.
enum ResetBy {
    /// This one: version 0, a fresh copy nobody read.
    This,
    /// One from before the version column (F3): `last_reset_at` stamped, the
    /// version left as recorded on the copy before.
    Older,
}

impl Fx {
    /// [`SANDBOX`]'s own branch row.
    async fn own_branch_row(&self) -> env_branches::Model {
        EnvBranches::find()
            .filter(env_branches::Column::AppId.eq(self.publisher))
            .filter(env_branches::Column::EnvName.eq(SANDBOX))
            .one(&self.t.db)
            .await
            .expect("read the sandbox's branch row")
            .expect("the sandbox has its branch")
    }

    /// Leave [`SANDBOX`]'s own branch as a reset copying a version-5
    /// production leaves it: the outbox without the MAC, the writer granted
    /// the unsigned definer (copied with the data), and the row marked as
    /// `by`'s reset marks it.
    async fn reset_own_branch_to_version_5(&self, by: ResetBy) {
        let row = self.own_branch_row().await;
        let tenant = oxy_oltp::migrator::tenant_for_org(&self.t.db, self.org())
            .await
            .expect("the org's tenant");
        let step_five = oxy_oltp::platform::steps_since(4, &tenant.provider, &tenant.database_name)
            .expect("the steps")
            .into_iter()
            .find(|step| step.version == 5)
            .expect("step 5");
        let admin = crate::common::admin_url().await;
        let database = row.database_name.as_deref().expect("the branch's database");
        let branch = connect(&on_database(&admin, database)).await;
        let owner = row.owner_role.as_deref().expect("the branch's owner");
        // As the branch's owner, who owns what the steps create.
        branch
            .batch_execute(&format!(
                "SET ROLE \"{owner}\"; DROP SCHEMA oxy_events CASCADE"
            ))
            .await
            .expect("drop the branch's outbox");
        for statement in &step_five.statements {
            branch.batch_execute(statement).await.expect("run step 5");
        }
        branch
            .batch_execute(&format!(
                "GRANT USAGE ON SCHEMA oxy_events TO \"{role}\"; \
                 GRANT EXECUTE ON FUNCTION {unsigned} TO \"{role}\"",
                role = self.own_branch_writer().await.role,
                unsigned = "oxy_events.publish(uuid,text,text,text,jsonb,uuid,text,uuid,uuid,int,uuid,int)",
            ))
            .await
            .expect("grant the unsigned definer, as a version-5 publish did");
        let marked = match by {
            ResetBy::This => {
                "UPDATE oltp_env_branches \
                    SET platform_schema_version = 0, platform_schema_cut_at = NULL, \
                        last_reset_at = now(), updated_at = now() \
                  WHERE id = $1"
            }
            ResetBy::Older => {
                "UPDATE oltp_env_branches SET last_reset_at = now(), updated_at = now() \
                  WHERE id = $1"
            }
        };
        self.platform(marked, vec![row.id.into()]).await;
    }

    /// The MAC on the row the publisher's writer published under `key` in
    /// [`SANDBOX`]'s own branch, read by a claim the test releases.
    async fn own_branch_mac(&self, key: &str) -> Option<Vec<u8>> {
        let target = OutboxTarget::EnvBranch {
            app_id: self.publisher,
            environment: SANDBOX.to_string(),
        };
        let claim = outbox::claim(&self.t.db, self.org(), &target, 100)
            .await
            .expect("claim the branch's outbox");
        let row = claim
            .rows()
            .iter()
            .find(|row| row.key == key)
            .unwrap_or_else(|| panic!("a row under {key}: {:?}", claim.rows()))
            .clone();
        claim.release().await.expect("release the claim");
        row.provenance
    }
}

/// `url` with its database replaced by `database`.
fn on_database(url: &str, database: &str) -> String {
    let (base, query) = match url.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (url, None),
    };
    let server = base.rsplit_once('/').map_or(base, |(server, _)| server);
    match query {
        Some(query) => format!("{server}/{database}?{query}"),
        None => format!("{server}/{database}"),
    }
}

/// Outside a transaction: the cached grant sends the publish straight to a
/// definer the reset took away (`42883`), and the host forgets the grant,
/// brings the branch current and publishes again, signed.
#[tokio::test]
async fn a_publish_after_a_reset_to_version_5_brings_the_branch_current() {
    with_fx(|fx| async move {
        fx.own_branch_sandbox().await;
        fx.publish_in_sandbox("before").await;
        fx.reset_own_branch_to_version_5(ResetBy::This).await;

        let answered = fx.publish_in_sandbox("after").await;
        let id = event_id(fx.org(), SANDBOX, &fx.event_type(), "after");
        assert_eq!(answered, json!({ "eventId": id.to_string() }));
        let mac = fx.own_branch_mac("after").await.expect("the row is signed");
        assert_eq!(mac.len(), 32);
    })
    .await;
}

/// On a `ctx.oltp.tx` handle: the old unsigned definer, still granted, must
/// not pass for the current one, or the publish would abort the app's
/// transaction. The host sees the current signature missing, brings the
/// branch current and grants it before anything is sent on the handle.
#[tokio::test]
async fn a_tx_publish_after_a_reset_to_version_5_brings_the_branch_current() {
    with_fx(|fx| async move {
        fx.own_branch_sandbox().await;
        fx.publish_in_sandbox("before").await;
        fx.reset_own_branch_to_version_5(ResetBy::This).await;

        let answered = fx.publish_in_sandbox_tx("after").await;
        let id = event_id(fx.org(), SANDBOX, &fx.event_type(), "after");
        assert_eq!(answered, json!({ "eventId": id.to_string() }));
        let mac = fx.own_branch_mac("after").await.expect("the row is signed");
        assert_eq!(mac.len(), 32);
    })
    .await;
}

/// An older binary's reset (F3) stamps `last_reset_at` and leaves the version
/// a newer binary recorded on the copy before. The recorded version holds for
/// the cut it was recorded on alone, so the publish after it replays the
/// platform steps on the fresh copy, at version 5 here, and lands signed; the
/// row is then recorded current on the new cut.
#[tokio::test]
async fn an_older_binarys_reset_is_not_trusted_with_the_version_of_the_cut_before() {
    with_fx(|fx| async move {
        fx.own_branch_sandbox().await;
        fx.publish_in_sandbox("before").await;
        let first = fx.own_branch_row().await;
        assert_eq!(
            (first.platform_schema_version, first.platform_schema_cut_at),
            (PLATFORM_SCHEMA_VERSION, Some(first.created_at)),
            "recorded current on the first cut"
        );
        fx.reset_own_branch_to_version_5(ResetBy::Older).await;
        let reset = fx.own_branch_row().await;
        assert_eq!(
            reset.platform_schema_version, PLATFORM_SCHEMA_VERSION,
            "what an older binary's reset leaves"
        );

        let answered = fx.publish_in_sandbox("after").await;
        let id = event_id(fx.org(), SANDBOX, &fx.event_type(), "after");
        assert_eq!(answered, json!({ "eventId": id.to_string() }));
        let mac = fx.own_branch_mac("after").await.expect("the row is signed");
        assert_eq!(mac.len(), 32);
        let after = fx.own_branch_row().await;
        assert_eq!(
            (after.platform_schema_version, after.platform_schema_cut_at),
            (PLATFORM_SCHEMA_VERSION, reset.last_reset_at),
            "recorded current on the new cut"
        );
    })
    .await;
}
