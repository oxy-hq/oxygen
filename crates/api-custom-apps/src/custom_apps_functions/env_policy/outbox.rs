//! Which outbox an event published under a policy is written to (app events
//! design §6.2, Task 17): it lands where `ctx.oltp` does.
//!
//! Production writes production's store. Staging and a sandbox on a schema of
//! the org's OLTP staging branch write the branch, named by the environment
//! ([`OutboxTarget::Branch`]). A sandbox on an OLTP branch of its own writes
//! that branch ([`OutboxTarget::EnvBranch`]). Each is the store the relay
//! reads those events from (`app_events::stores`).

use oxy_oltp::events::OutboxTarget;

use super::{EnvPolicy, OltpHome};

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
            OltpHome::EnvironmentBranch(home) => Ok(OutboxTarget::EnvBranch {
                app_id: home.app_id,
                environment: home.env_name.clone(),
            }),
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

    /// A sandbox on an OLTP branch of its own writes that branch's outbox,
    /// where its `ctx.oltp` writes, and the relay reads it from there.
    #[test]
    fn a_sandbox_on_its_own_branch_writes_that_branch() {
        assert_eq!(
            sandbox().with_oltp_home(own_branch()).outbox_target(),
            Ok(OutboxTarget::EnvBranch {
                app_id: uuid::Uuid::from_u128(8),
                environment: "dev-luong".into(),
            })
        );
    }

    /// No home a publish is never admitted to falls back to production's
    /// store.
    #[test]
    fn nothing_else_has_an_outbox() {
        let refused = [
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
