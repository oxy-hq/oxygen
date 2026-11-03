//! A golden table of every non-production decision `EnvPolicy::decide` makes.
//!
//! One line per host op, per non-production environment, per `OltpHome`
//! sample, so a change to the policy shows up as a diff of
//! `golden_decisions.txt` rather than as a silent behavior change. Regenerate
//! it deliberately with `OXY_UPDATE_GOLDEN=1`.

use oxy_app_core::custom_app_environment::AppEnvironment;

use super::tests::sandbox_home;
use super::*;

const GOLDEN_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/custom_apps_functions/env_policy/golden_decisions.txt"
);

/// Every `SandboxUnready` variant with a stable label. The match is explicit
/// so a renamed or added variant is a compile error or a diff, never a
/// silent change to the label.
fn unready_samples() -> Vec<(&'static str, SandboxUnready)> {
    [
        SandboxUnready::NotCreated,
        SandboxUnready::Seeding,
        SandboxUnready::Failed,
        SandboxUnready::BranchReset,
        SandboxUnready::NoSchemaName,
    ]
    .into_iter()
    .map(|why| {
        let reason = match why {
            SandboxUnready::NotCreated => "not-created",
            SandboxUnready::Seeding => "seeding",
            SandboxUnready::Failed => "failed",
            SandboxUnready::BranchReset => "branch-reset",
            SandboxUnready::NoSchemaName => "no-schema-name",
        };
        (reason, why)
    })
    .collect()
}

pub(super) fn home_samples() -> Vec<(String, OltpHome)> {
    let mut homes = vec![
        ("production".to_string(), OltpHome::Production),
        (
            "staging-branch".to_string(),
            OltpHome::StagingBranch("br-staging".into()),
        ),
        ("sandbox-schema".to_string(), sandbox_home()),
    ];
    for (reason, why) in unready_samples() {
        homes.push((
            format!("sandbox-unready-{reason}"),
            OltpHome::SandboxUnready(why),
        ));
    }
    homes
}

/// The table: `<op>\t<environment>\t<home>\t<decision>` per line.
pub(super) fn render_table() -> String {
    render_table_of(|env| EnvPolicy::for_environment(env.clone()))
}

/// [`render_table`] for the policy `policy_of` builds for each environment,
/// before the `OltpHome` sample is set on it.
pub(super) fn render_table_of(policy_of: impl Fn(&AppEnvironment) -> EnvPolicy) -> String {
    let environments = [
        AppEnvironment::Staging,
        AppEnvironment::parse("dev-a1").expect("a dev slot"),
    ];
    let homes = home_samples();
    let mut table = String::new();
    for op in HostOp::ALL {
        for env in &environments {
            for (label, home) in &homes {
                let decision = policy_of(env).with_oltp_home(home.clone()).decide(*op);
                table.push_str(&format!(
                    "{}\t{}\t{}\t{:?}\n",
                    op.name(),
                    env.name(),
                    label,
                    decision
                ));
            }
        }
    }
    table
}

#[test]
fn golden_non_production_decisions() {
    let table = render_table();
    if std::env::var("OXY_UPDATE_GOLDEN").is_ok_and(|v| v == "1") {
        std::fs::write(GOLDEN_PATH, &table).expect("write golden_decisions.txt");
        return;
    }
    assert!(
        table == include_str!("golden_decisions.txt"),
        "a non-production decision changed; review the diff of golden_decisions.txt \
         (regenerate with OXY_UPDATE_GOLDEN=1 only if the change is intended)"
    );
}
