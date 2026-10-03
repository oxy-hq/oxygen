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
//!
//! **A sandbox does not share staging's schema on the branch.** In an org
//! with a branch, a sandbox's `ctx.oltp` runs in the sandbox's **own schema**
//! there ([`OltpHome::SandboxSchema`]; `internal-docs/per-org-oltp-postgres.md`
//! → Sandbox schemas on the staging branch) — or, while that schema is not
//! ready, is refused ([`OltpHome::SandboxUnready`]). It never lands in
//! staging's `app_<writer>`, and never reads production in its place.

use oxy_oltp::branches::BranchCut;
use oxy_oltp::sandbox_schema::SandboxSchema;

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
    /// A sandbox's own schema on that branch. Reads and writes run there, as
    /// the app's writer with the schema as the only `search_path` entry, and
    /// a statement that names another schema is refused (`oltp_sandbox_sql`).
    SandboxSchema(SandboxHome),
    /// The org has a branch, but this sandbox's schema on it cannot be used:
    /// every `ctx.oltp` call is refused, saying why.
    SandboxUnready(SandboxUnready),
}

/// A sandbox's schema, and the branch cut the admission found it ready on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxHome {
    /// A branch reset or re-cut since is a different cut: the schema went
    /// with the old copy, and the call fails rather than follow.
    pub cut: BranchCut,
    pub schema: SandboxSchema,
}

/// Why a sandbox's schema on the org's staging branch cannot be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxUnready {
    /// No publish to the sandbox has queued it since the org got its branch.
    NotCreated,
    /// The task that creates and seeds it has not finished.
    Seeding,
    /// Creating or seeding it failed; `oxyc env show` has the reason.
    Failed,
    /// The branch was reset or re-cut since it was seeded, which replaced the
    /// database it was in.
    BranchReset,
    /// The app and the sandbox name no schema: the name would be over
    /// Postgres's 63 bytes, or the app's own schema already holds `__`.
    NoSchemaName,
}

impl SandboxUnready {
    /// The `Refuse` fix: what is wrong, and what puts it right.
    pub const fn fix(self) -> &'static str {
        match self {
            Self::NotCreated => {
                "this sandbox has no schema of its own yet on the org's OLTP staging branch; a \
                 publish to the sandbox creates and seeds it (`oxyc publish --app-env <sandbox>`)"
            }
            Self::Seeding => {
                "this sandbox's own schema on the org's OLTP staging branch is still being \
                 created and seeded; try again in a few seconds (`oxyc env show` reports \
                 oltp_schema.status), and if it stays seeding, publish to the sandbox again"
            }
            Self::Failed => {
                "this sandbox's own schema on the org's OLTP staging branch could not be created \
                 (`oxyc env show` reports why in oltp_schema.error); publish to the sandbox again \
                 once that is fixed"
            }
            Self::BranchReset => {
                "the org's OLTP staging branch was reset since this sandbox's schema was seeded, \
                 which removed it; publish to the sandbox again to seed a new copy and apply \
                 its migrations"
            }
            Self::NoSchemaName => {
                "this sandbox has no schema of its own on the org's OLTP staging branch: its name \
                 (app_<app>__dev_<handle>) would be over Postgres's 63 bytes, or the app's slug \
                 holds `--`. Use a shorter sandbox handle"
            }
        }
    }
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
            OltpHome::StagingBranch(_) | OltpHome::SandboxSchema(_) => {
                Decision::Isolate(Target::OltpBranch)
            }
            OltpHome::SandboxUnready(why) => Decision::Refuse { fix: why.fix() },
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
