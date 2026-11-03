//! The org's cap on environment branches, re-checked by the cut itself under
//! the tenant's lock (`provisioner/env_branches_cap.rs`): the request-time
//! count sees rows only, and cuts are queued work. `LocalProvider` only.

use oxy_oltp::ProvisionerError;
use oxy_oltp::entity::env_branches::{EnvBranchMode, EnvBranchStatus};
use oxy_oltp::env_branches::{EnvBranchKey, count_for_org};
use uuid::Uuid;

use crate::oltp_provisioner::with_fx;

fn key_of(org_id: Uuid, env: &str) -> EnvBranchKey {
    EnvBranchKey {
        org_id,
        app_id: Uuid::new_v4(),
        env_name: env.into(),
    }
}

/// Two cuts at `cap - 1` race for the last slot: one wins, the other is
/// refused with the cap's sentence before it records a row or calls the
/// provider, and the org ends at the cap, not past it.
#[tokio::test]
async fn two_cuts_at_the_cap_leave_one_branch() {
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::set_var("OXY_APP_ENV_OLTP_BRANCHES_MAX", "2") };
    with_fx(|fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let first = key_of(fx.org_id, "dev-a1");
        fx.provisioner
            .cut_env_branch(&first, EnvBranchMode::Branch, None)
            .await
            .expect("the first cut: one below the cap");

        let (b, c) = (key_of(fx.org_id, "dev-b2"), key_of(fx.org_id, "dev-c3"));
        let (left, right) = tokio::join!(
            fx.provisioner
                .cut_env_branch(&b, EnvBranchMode::Branch, None),
            fx.provisioner
                .cut_env_branch(&c, EnvBranchMode::Branch, None),
        );

        let (won, refused) = match (left, right) {
            (Ok(won), Err(refused)) | (Err(refused), Ok(won)) => (won, refused),
            (left, right) => panic!("exactly one cut wins: {left:?} / {right:?}"),
        };
        assert_eq!(won.status, EnvBranchStatus::Migrating);
        let ProvisionerError::EnvBranchCap(said) = &refused else {
            panic!("the loser is refused at the cap: {refused:?}");
        };
        assert!(said.contains("has 2 sandbox OLTP branches"), "{said}");
        assert!(said.contains("oltp=schema"), "{said}");
        assert_eq!(count_for_org(&fx.db, fx.org_id).await.unwrap(), 2);

        // An environment that already holds a branch is not refused by its own
        // row: a retry of the winner returns it untouched.
        let again = fx
            .provisioner
            .cut_env_branch(&first, EnvBranchMode::Branch, None)
            .await
            .expect("its own row is not counted against it");
        assert_eq!(again.env_name, "dev-a1");
    })
    .await;
}
