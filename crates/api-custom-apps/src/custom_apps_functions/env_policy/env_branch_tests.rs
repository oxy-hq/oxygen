//! A sandbox on a branch of its own (`OltpHome::EnvironmentBranch`): which
//! ops it isolates, and what an unready branch says. The golden table
//! (`golden_decisions.txt`) is not regenerated for it: its samples are the
//! homes of an environment on default planes, which these are not.

use std::collections::BTreeSet;

use super::tests::sandbox;
use super::*;
use crate::custom_apps_planes::Plane;

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

const UNREADY: [EnvBranchUnready; 6] = [
    EnvBranchUnready::NotCut,
    EnvBranchUnready::Cutting,
    EnvBranchUnready::Failed,
    EnvBranchUnready::NoTenant,
    EnvBranchUnready::CutAsBranch,
    EnvBranchUnready::CutAsEmpty,
];

/// Every op that opens an OLTP connection isolates to the environment's
/// branch — never `Allow` (production's) and never `Hold` (read-only
/// production) — and no other row moves from what the sandbox decides on
/// default planes.
#[test]
fn an_env_branch_isolates_every_oltp_op() {
    let policy = sandbox().with_oltp_home(own_branch());
    let plain = sandbox();
    let mut isolated = BTreeSet::new();
    for op in HostOp::ALL {
        let decided = policy.decide(*op);
        if op.plane() == Plane::Oltp {
            assert_eq!(decided, Decision::Isolate(Target::OltpBranch), "{op:?}");
            isolated.insert(*op);
        } else {
            assert_eq!(decided, plain.decide(*op), "{op:?} moved");
        }
    }
    assert_eq!(
        isolated,
        BTreeSet::from([HostOp::TxBeginOltp, HostOp::OltpQuery, HostOp::OltpExec])
    );
}

#[test]
fn an_unready_env_branch_refuses_with_its_fix() {
    for why in UNREADY {
        let policy = sandbox().with_oltp_home(OltpHome::EnvironmentBranchUnready(why));
        for op in [HostOp::TxBeginOltp, HostOp::OltpQuery, HostOp::OltpExec] {
            assert_eq!(
                policy.decide(op),
                Decision::Refuse { fix: why.fix() },
                "{why:?} {op:?}"
            );
        }
    }
    for why in UNREADY {
        assert!(!why.fix().contains("staging branch"), "{why:?}");
    }
    for (why, says) in [
        (
            EnvBranchUnready::NotCut,
            "oxyc env reset <app> <env> --plane oltp",
        ),
        (EnvBranchUnready::NotCut, "queues the cut"),
        (EnvBranchUnready::Cutting, "try again in a few seconds"),
        (EnvBranchUnready::Failed, "`oxyc env show` reports why"),
        (EnvBranchUnready::NoTenant, "no active OLTP database"),
        (EnvBranchUnready::CutAsBranch, "cut as `branch`"),
        (EnvBranchUnready::CutAsBranch, "now ask for `empty`"),
        (EnvBranchUnready::CutAsEmpty, "cut as `empty`"),
        (EnvBranchUnready::CutAsEmpty, "now ask for `branch`"),
    ] {
        assert!(why.fix().contains(says), "{why:?}: {}", why.fix());
    }
}

/// Production takes neither home, whatever it is handed.
#[test]
fn production_never_takes_an_env_branch_home() {
    for home in [
        own_branch(),
        OltpHome::EnvironmentBranchUnready(EnvBranchUnready::NotCut),
    ] {
        let production = EnvPolicy::production().with_oltp_home(home);
        assert_eq!(production.oltp_home(), &OltpHome::Production);
        assert_eq!(production.decide(HostOp::OltpExec), Decision::Allow);
    }
}

/// A sandbox on its own branch is told about its own OLTP branch, not the
/// org's staging branch (which it does not use).
#[test]
fn the_statement_errors_of_an_own_branch_do_not_name_the_staging_branch() {
    let environment = AppEnvironment::Dev {
        handle: "luong".into(),
    };
    let refused = env_branch_statement_message(HostOp::OltpExec, &environment, "reads a file");
    let held = env_branch_held_statement_message(HostOp::OltpExec, "runs a DO block");
    for message in [&refused, &held] {
        assert!(!message.contains("staging"), "{message}");
    }
    assert!(refused.contains("its own OLTP branch"), "{refused}");
    assert!(held.contains("the sandbox's own OLTP branch"), "{held}");
}
