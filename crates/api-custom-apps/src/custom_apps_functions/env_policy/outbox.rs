//! Which outbox an event published under a policy is written to (app events
//! design §6.2, Task 17): it lands where `ctx.oltp` does.
//!
//! Production writes production's store. Staging and a sandbox on a schema of
//! the org's OLTP staging branch write the branch, named by the environment
//! ([`OutboxTarget::Branch`]): that is the store the relay reads their events
//! from. A sandbox on an OLTP branch of its own is refused: its `ctx.oltp`
//! reaches a database no relay reads, so an event written there would never
//! be delivered.

use oxy_oltp::events::OutboxTarget;

use super::{EnvPolicy, OltpHome};

/// The refusal's fix for an event published from a sandbox on an OLTP branch
/// of its own.
pub const ENV_BRANCH_EVENTS_FIX: &str = "a sandbox on an OLTP branch of its own cannot publish \
     events yet: they are relayed from the org's OLTP staging branch, which this sandbox's \
     `ctx.oltp` does not reach. Publish from a sandbox whose OLTP plane is `schema`";

impl EnvPolicy {
    /// The store an event published under this policy is written to, or the
    /// refusal's fix where none is. Asked only once the publish is admitted
    /// (isolated to the run's OLTP branch, or allowed in production): a home
    /// a publish is never admitted to is refused here too, never written to
    /// production in its place.
    pub fn outbox_target(&self) -> Result<OutboxTarget, &'static str> {
        match &self.oltp {
            OltpHome::Production if self.is_production() => Ok(OutboxTarget::Production),
            OltpHome::StagingBranch(_) | OltpHome::SandboxSchema(_) => {
                Ok(OutboxTarget::Branch(self.environment().name()))
            }
            OltpHome::EnvironmentBranch(_) => Err(ENV_BRANCH_EVENTS_FIX),
            OltpHome::SandboxUnready(why) => Err(why.fix()),
            OltpHome::EnvironmentBranchUnready(why) => Err(why.fix()),
            OltpHome::Production => Err(super::NO_BRANCH_NOTE),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{sandbox, sandbox_home};
    use super::super::{EnvBranchHome, EnvBranchUnready, SandboxUnready};
    use super::*;
    use oxy_app_core::custom_app_environment::AppEnvironment;

    fn own_branch() -> OltpHome {
        OltpHome::EnvironmentBranch(EnvBranchHome {
            cut: oxy_oltp::branches::BranchCut {
                row_id: uuid::Uuid::from_u128(7),
                provider_branch_id: "br-env".into(),
                cut_at: chrono::Utc::now().into(),
                scope: oxy_oltp::branches::CutScope::Environment,
            },
            app_id: uuid::Uuid::from_u128(8),
            env_name: "dev-luong".into(),
        })
    }

    #[test]
    fn production_writes_production_and_a_branch_home_its_environment() {
        assert_eq!(
            EnvPolicy::production().outbox_target(),
            Ok(OutboxTarget::Production)
        );
        let staging = EnvPolicy::for_environment(AppEnvironment::Staging)
            .with_oltp_home(OltpHome::StagingBranch("br-1".into()));
        assert_eq!(
            staging.outbox_target(),
            Ok(OutboxTarget::Branch("staging".into()))
        );
        let on_schema = sandbox().with_oltp_home(sandbox_home());
        let Ok(OutboxTarget::Branch(environment)) = on_schema.outbox_target() else {
            panic!("a sandbox on a schema writes the staging branch");
        };
        assert!(environment.starts_with("dev-"), "{environment}");
    }

    /// A sandbox on its own OLTP branch is refused rather than written where
    /// no relay reads, and no home a publish is never admitted to falls back
    /// to production's store.
    #[test]
    fn nothing_else_has_an_outbox() {
        let refused = [
            (
                sandbox().with_oltp_home(own_branch()),
                ENV_BRANCH_EVENTS_FIX,
            ),
            (
                sandbox().with_oltp_home(OltpHome::SandboxUnready(SandboxUnready::Seeding)),
                SandboxUnready::Seeding.fix(),
            ),
            (
                sandbox().with_oltp_home(OltpHome::EnvironmentBranchUnready(
                    EnvBranchUnready::Cutting,
                )),
                EnvBranchUnready::Cutting.fix(),
            ),
            (
                EnvPolicy::for_environment(AppEnvironment::Staging),
                super::super::NO_BRANCH_NOTE,
            ),
        ];
        for (policy, fix) in refused {
            assert_eq!(policy.outbox_target(), Err(fix));
        }
    }
}
