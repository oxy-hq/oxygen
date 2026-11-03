//! What an environment's `app_environments.planes` column says: one optional
//! mode per plane, parsed strictly, and resolved against the defaults for the
//! environment's kind. Every default is today's behaviour, and P0 has one
//! mode per plane (two for `workspace-actions`), so resolving changes no
//! decision (`internal-docs/2026-11-02-custom-app-branches-design.md` §3).

use std::collections::BTreeSet;

use oxy_app_core::custom_app_environment::{AppEnvironment, AppEnvironmentKind};
use serde::{Deserialize, Serialize};

use super::warehouse::WarehousePlane;
use super::{FetchSetting, Plane, SemanticChoice};

/// A plane's mode enum: kebab-case on the wire, from this release on a
/// contract.
macro_rules! modes {
    ($($(#[$meta:meta])* $name:ident { $($variant:ident => $wire:literal),+ $(,)? })+) => {$(
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "kebab-case")]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            /// Every mode, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// The mode's wire string.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire),+
                }
            }
        }
    )+};
}

modes! {
    /// The build the environment serves.
    BuildMode { Own => "own" }
    /// The semantic revision the environment reads: the build's own, or a git
    /// branch (the branch name is on [`SemanticChoice`], not here, so the
    /// resolved modes stay `Copy`). In a spec the `branch` mode is written
    /// `branch:<name>` (`"semantic": "branch:feat-x"`); the wire string
    /// `"branch"` alone names no branch and is refused. `release:staging` waits
    /// on W4's data releases and is not a mode yet.
    SemanticMode { BuildPin => "build-pin", Branch => "branch" }
    /// Live reads of the org's data.
    ReadsMode { Live => "live" }
    /// The app's transactional (`ctx.oltp`) data.
    OltpMode { Schema => "schema", Branch => "branch", Empty => "empty" }
    /// The app's Airhouse data. Writes land in the sibling in every mode;
    /// `branch` reads them back through a copy-on-write overlay, and
    /// `snapshot` also reads every table it has not written at a pinned point
    /// in time (`env_policy::airhouse_overlay`).
    AirhouseMode { WriteOnly => "write-only", Branch => "branch", Snapshot => "snapshot" }
    /// Customer-warehouse access: writes land in the mapped destination.
    WarehouseMode { Mapped => "mapped" }
    /// File storage: `branch` reads through to production's same key,
    /// `empty` reads the environment's own silo alone.
    StorageMode { Branch => "branch", Empty => "empty" }
    /// Secret values: `inherit` reads the environment's own path and then the
    /// fallbacks of its kind, `empty` reads the environment's own path alone.
    SecretsMode { Inherit => "inherit", Empty => "empty" }
    /// Who a notification or email reaches: `invoker` the invoking user alone,
    /// `capture` nobody, the rendered message kept in the environment's outbox.
    MessagesMode { Invoker => "invoker", Capture => "capture" }
    /// Outbound HTTP: `held-writes` holds every non-read call, `allow` sends a
    /// call to a listed provider sandbox (staging and sandboxes only).
    FetchMode { HeldWrites => "held-writes", Allow => "allow" }
    /// Scheduled function runs.
    SchedulesMode { Switch => "switch" }
    /// What the bundle's writes through the workspace `/api` do: an agent ask
    /// or an automation run. Staging and a sandbox hold them by default (an ask
    /// runs with its writes held, an automation run answers `409`); `refused`
    /// answers the workspace `/api` write `403` instead; `preview` holds an ask
    /// the same way and runs an automation as a workspace-preview dry run on
    /// the environment's branch, its Airhouse writes in the overlay (design §3
    /// row 12, P5). `airway.run` is refused under every mode.
    WorkspaceActionsMode { Held => "held", Refused => "refused", Preview => "preview" }
}

/// The modes an environment's row sets; a plane left out takes the default.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct PlaneSpec {
    pub build: Option<BuildMode>,
    pub semantic: Option<SemanticChoice>,
    pub reads: Option<ReadsMode>,
    pub oltp: Option<OltpMode>,
    pub airhouse: Option<AirhouseMode>,
    pub warehouse: Option<WarehousePlane>,
    pub storage: Option<StorageMode>,
    pub secrets: Option<SecretsMode>,
    pub messages: Option<MessagesMode>,
    pub fetch: Option<FetchSetting>,
    pub schedules: Option<SchedulesMode>,
    pub workspace_actions: Option<WorkspaceActionsMode>,
}

/// Every plane's mode, with nothing left unset. `Clone`, not `Copy`: a plane
/// that grows settings (a map of destinations, a list of hosts) must not break
/// every holder. `fetch_hosts` is the sandbox hosts `fetch: allow` names,
/// lowercased, empty under any other mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedPlanes {
    pub build: BuildMode,
    pub semantic: SemanticMode,
    pub reads: ReadsMode,
    pub oltp: OltpMode,
    pub airhouse: AirhouseMode,
    pub warehouse: WarehousePlane,
    pub storage: StorageMode,
    pub secrets: SecretsMode,
    pub messages: MessagesMode,
    pub fetch: FetchMode,
    pub fetch_hosts: BTreeSet<String>,
    pub schedules: SchedulesMode,
    pub workspace_actions: WorkspaceActionsMode,
}

/// What `preset: "full"` expands to: one `(plane, mode)` entry for each plane
/// whose isolating mode has shipped. A plane with no entry keeps the default
/// of its kind, so a preset never names a mode that does not exist. Each
/// phase adds its own plane's entry
/// (`internal-docs/2026-11-02-custom-app-branches-design.md` §5.5).
/// `preset: "light"` is `{}`: every default.
///
/// OLTP's and Airhouse's are `branch`, which are for sandboxes only, so `full`
/// on staging is refused by `validate` with the reason. Fetch is deliberately
/// absent: opening third-party writes is a person's per-host decision, never a
/// preset. There is no `semantic` entry either: its isolating mode is
/// `branch:<name>`, and a preset cannot invent the branch to read. Choose it
/// with an explicit `planes: {"semantic": "branch:<name>"}`, which wins over
/// the preset.
pub const PRESET_FULL: &[(Plane, &str)] = &[
    (Plane::Oltp, "branch"),
    (Plane::Secrets, "empty"),
    (Plane::Messages, "capture"),
    (Plane::Airhouse, "branch"),
    (Plane::WorkspaceActions, "preview"),
];

impl PlaneSpec {
    /// The git branch the `semantic` plane reads, when the spec names one.
    pub fn semantic_branch(&self) -> Option<&str> {
        match &self.semantic {
            Some(SemanticChoice::Branch(name)) => Some(name),
            _ => None,
        }
    }

    /// Read a `planes` column value. An unknown plane or an unknown mode is an
    /// error naming it, never ignored.
    pub fn parse(value: &serde_json::Value) -> Result<PlaneSpec, String> {
        PlaneSpec::deserialize(value).map_err(|e| e.to_string())
    }

    /// The modes `env` runs with: this spec over the defaults for its kind.
    /// `None` for production, which has no planes.
    pub fn resolve(&self, env: &AppEnvironment) -> Option<ResolvedPlanes> {
        let workspace_actions = match env.kind() {
            AppEnvironmentKind::Production => return None,
            // Staging and a sandbox both hold an agent ask and refuse an
            // automation run; `refused` is a mode either can choose.
            AppEnvironmentKind::Staging | AppEnvironmentKind::Dev => WorkspaceActionsMode::Held,
        };
        let fetch = self
            .fetch
            .as_ref()
            .map_or(FetchMode::HeldWrites, |f| f.mode());
        let fetch_hosts = match (&self.fetch, fetch) {
            (Some(setting), FetchMode::Allow) => setting
                .hosts()
                .iter()
                .map(|h| h.to_ascii_lowercase())
                .collect(),
            _ => BTreeSet::new(),
        };
        Some(ResolvedPlanes {
            build: self.build.unwrap_or(BuildMode::Own),
            semantic: self
                .semantic
                .as_ref()
                .map_or(SemanticMode::BuildPin, SemanticChoice::mode),
            reads: self.reads.unwrap_or(ReadsMode::Live),
            oltp: self.oltp.unwrap_or(OltpMode::Schema),
            airhouse: self.airhouse.unwrap_or(AirhouseMode::WriteOnly),
            warehouse: self.warehouse.clone().unwrap_or_default(),
            storage: self.storage.unwrap_or(StorageMode::Branch),
            secrets: self.secrets.unwrap_or(SecretsMode::Inherit),
            messages: self.messages.unwrap_or(MessagesMode::Invoker),
            fetch,
            fetch_hosts,
            schedules: self.schedules.unwrap_or(SchedulesMode::Switch),
            workspace_actions: self.workspace_actions.unwrap_or(workspace_actions),
        })
    }
}

impl Plane {
    /// Every mode the plane takes, as wire strings. One arm per plane, so a
    /// plane added to [`Plane`] does not compile until it lists its modes.
    pub fn modes(self) -> Vec<&'static str> {
        fn wires<M: Copy>(all: &[M], wire: fn(M) -> &'static str) -> Vec<&'static str> {
            all.iter().map(|mode| wire(*mode)).collect()
        }
        match self {
            Plane::Build => wires(BuildMode::ALL, BuildMode::as_str),
            Plane::Semantic => wires(SemanticMode::ALL, SemanticMode::as_str),
            Plane::Reads => wires(ReadsMode::ALL, ReadsMode::as_str),
            Plane::Oltp => wires(OltpMode::ALL, OltpMode::as_str),
            Plane::Airhouse => wires(AirhouseMode::ALL, AirhouseMode::as_str),
            Plane::Warehouse => wires(WarehouseMode::ALL, WarehouseMode::as_str),
            Plane::Storage => wires(StorageMode::ALL, StorageMode::as_str),
            Plane::Secrets => wires(SecretsMode::ALL, SecretsMode::as_str),
            Plane::Messages => wires(MessagesMode::ALL, MessagesMode::as_str),
            Plane::Fetch => wires(FetchMode::ALL, FetchMode::as_str),
            Plane::Schedules => wires(SchedulesMode::ALL, SchedulesMode::as_str),
            Plane::WorkspaceActions => {
                wires(WorkspaceActionsMode::ALL, WorkspaceActionsMode::as_str)
            }
        }
    }
}

impl ResolvedPlanes {
    /// Each plane and its mode's wire string, in [`Plane::ALL`] order.
    pub fn entries(&self) -> Vec<(Plane, &'static str)> {
        vec![
            (Plane::Build, self.build.as_str()),
            (Plane::Semantic, self.semantic.as_str()),
            (Plane::Reads, self.reads.as_str()),
            (Plane::Oltp, self.oltp.as_str()),
            (Plane::Airhouse, self.airhouse.as_str()),
            (Plane::Warehouse, self.warehouse.mode.as_str()),
            (Plane::Storage, self.storage.as_str()),
            (Plane::Secrets, self.secrets.as_str()),
            (Plane::Messages, self.messages.as_str()),
            (Plane::Fetch, self.fetch.as_str()),
            (Plane::Schedules, self.schedules.as_str()),
            (Plane::WorkspaceActions, self.workspace_actions.as_str()),
        ]
    }
}

/// The parent an environment has when its row names none: staging is cut from
/// production, a sandbox from staging, and production has no parent.
pub fn default_parent(env: &AppEnvironment) -> Option<AppEnvironment> {
    match env.kind() {
        AppEnvironmentKind::Production => None,
        AppEnvironmentKind::Staging => Some(AppEnvironment::Production),
        AppEnvironmentKind::Dev => Some(AppEnvironment::Staging),
    }
}
