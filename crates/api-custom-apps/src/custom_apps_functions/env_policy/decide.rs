//! The two tables behind [`super::EnvPolicy::decide`]: what a non-production
//! environment does with each op, and what a production run that reads a
//! branch does.

use super::airhouse_overlay;
use super::op_kinds::Staged;
use super::{BRANCH_AIRWAY_FIX, Decision, HostOp, MISROUTED_FIX, OltpHome, PlaneHomes};
use crate::custom_apps_planes::{AirhouseMode, Plane};
use oxy_app_core::custom_app_environment::AppEnvironmentKind;

/// A production-admitted run reading a branch (previews S9, I9) is a preview:
/// it decides as staging does — its writes held, not live — and
/// `ctx.airway.run` is refused with the reason that applies to it. No caller
/// runs a function on a branch today; one that does later is held, not live.
///
/// It has no environment of its own, so none of staging's isolated homes: a
/// silo, a secret path or a sibling resolved for a production policy would be
/// production's. So an op staging isolates is held here, except the storage
/// reads, which read production's silo as they did before it had a sibling.
/// Its `ctx.oltp` is production's, with no OLTP branch: held as above.
pub(super) fn production_on_branch(op: HostOp) -> Decision {
    use HostOp::*;
    match op {
        AirwayRun => Decision::Refuse {
            fix: BRANCH_AIRWAY_FIX,
        },
        StorageGetDownloadUrl | StorageGet | StorageHead | StorageList => Decision::Allow,
        _ => match non_production(
            op,
            &OltpHome::Production,
            PlaneHomes::default_for(AppEnvironmentKind::Staging).as_ref(),
            Some(AirhouseMode::WriteOnly),
        ) {
            Decision::Isolate(_) => Decision::Hold,
            decided => decided,
        },
    }
}

/// Staging and every sandbox — one table for every non-production
/// environment, read from the registry (`registry::HostOp::spec`):
/// production data read, a write with an isolated home performed there (the
/// app's OLTP store on the org's staging branch when it has one), every
/// other write held, Airway refused. The table names *which kind* of home;
/// *whose* is the environment's (`homes`: its own silo, secret path and
/// Airhouse sibling). A new [`HostOp`] fails to compile until its row states
/// what it does here.
///
/// A row that decides by home asks the plane's home ([`PlaneHomes`]); for
/// `ctx.fetch` that is its mode: `held-writes` holds a mutating call, `allow`
/// names the provider sandbox it would reach, which
/// [`EnvPolicy::decide_on_fetch`] confirms call by call.
pub(super) fn non_production(
    op: HostOp,
    oltp: &OltpHome,
    homes: Option<&PlaneHomes>,
    airhouse: Option<AirhouseMode>,
) -> Decision {
    match op.spec().staging {
        Staged::Decided(decided) => decided,
        Staged::ByHome(Plane::Oltp) => oltp.decision(),
        // `ctx.airhouse.query` reads by the Airhouse plane's mode, which is not
        // a home: under `write-only` it reads production (`Allow`), which no
        // home may decide (`plane_homes`).
        Staged::ByHome(Plane::Airhouse) => match airhouse {
            Some(mode) => airhouse_overlay::query_decision(mode),
            None => Decision::Refuse { fix: MISROUTED_FIX },
        },
        // A policy outside production always has homes; a row that decides by
        // a plane with none fails closed.
        Staged::ByHome(_) => match homes {
            Some(homes) => homes.decision(op),
            None => Decision::Refuse { fix: MISROUTED_FIX },
        },
    }
}
