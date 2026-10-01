//! Where a function's `ctx.oltp` lands — decided once, at admission (previews
//! P4b; env design §4.2 `ctx.oltp` row, §4.3).
//!
//! Production writes production's database. A staging run writes the org's
//! OLTP **staging branch** when the org has one: a copy of production's
//! database (P4a), so its reads and writes run there as asked. An org with no
//! branch keeps Phase 3's hold: a single read on production, `READ ONLY`,
//! every other statement held.
//!
//! The lookup happens in `environment_gate::with_oltp_home`, once per
//! invocation; the policy carries the answer and no host call re-derives it.
//! A branch the admission found is never swapped for production later: if the
//! branch is gone or not active by the time a call connects, the call fails
//! rather than falling back.

use super::{Decision, EnvPolicy, Target};

/// The OLTP database a run's `ctx.oltp` reaches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OltpHome {
    /// Production's own database: written in production, read-only (every
    /// write held) anywhere else. The default for every policy.
    Production,
    /// The org's OLTP staging branch, by the provider branch id the admission
    /// found. Reads and writes run on it; production is never reached.
    StagingBranch(String),
}

/// Why a staging `ctx.oltp` write was held — the note on its held-row entry
/// and the end of the error the function sees.
pub const NO_BRANCH_NOTE: &str =
    "no staging branch — provision one with `oxyc oltp provision --branch staging`";

/// `Refuse` fix for a statement on the staging branch that reaches outside
/// that database (`oltp_branch_sql`).
pub const BRANCH_REFUSED_FIX: &str = "a staging function's OLTP statements run on the org's \
     staging branch, a copy of production's database; a call that reads or writes server \
     files, opens another connection or changes cluster-wide state reaches outside that copy, \
     so it is not sent. Run it from production";

impl OltpHome {
    /// What staging's `ctx.oltp` ops (`oltp.query`, `oltp.exec`,
    /// `tx.begin_oltp`) decide with this home.
    pub(super) fn decision(&self) -> Decision {
        match self {
            OltpHome::StagingBranch(_) => Decision::Isolate(Target::OltpBranch),
            OltpHome::Production => Decision::Hold,
        }
    }
}

impl EnvPolicy {
    /// The same policy writing `ctx.oltp` to `home`. Ignored in production,
    /// which always writes its own database.
    pub fn with_oltp_home(mut self, home: OltpHome) -> Self {
        if !self.is_production() {
            self.oltp = home;
        }
        self
    }

    /// Where this run's `ctx.oltp` lands.
    pub fn oltp_home(&self) -> &OltpHome {
        &self.oltp
    }
}

// A statement on a `ctx.oltp.tx` handle follows where its `begin` went:
// `EnvPolicy::decide_on_handle` (in `homes`), shared with `ctx.tx`.
