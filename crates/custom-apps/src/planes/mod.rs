//! The planes of a custom app: the kinds of state an app has, each of which an
//! environment or branch gives its own home
//! (`internal-docs/2026-11-02-custom-app-branches-design.md` §3).

pub mod airhouse;
mod environment;
mod fetch;
#[cfg(test)]
mod fetch_tests;
mod request;
mod semantic;
mod spec;
#[cfg(test)]
mod spec_tests;
mod warehouse;

pub use environment::{environment_planes, planes_from_row};
pub use fetch::{
    FetchSetting, PROVIDER_SANDBOXES, ProviderSandbox, SandboxAdmits, provider_sandbox,
    validate_fetch,
};
pub use request::{PlaneContext, PlaneError, RequestedPlanes};
pub use semantic::{SemanticChoice, valid_branch_name};
pub use spec::{
    AirhouseMode, BuildMode, FetchMode, MessagesMode, OltpMode, PRESET_FULL, PlaneSpec, ReadsMode,
    ResolvedPlanes, SchedulesMode, SecretsMode, SemanticMode, StorageMode, WarehouseMode,
    WorkspaceActionsMode, default_parent,
};
pub use warehouse::WarehousePlane;

/// One kind of app state. Every host op lives on exactly one plane
/// (`HostOp::plane`). A registry row can say its op is decided by its plane's
/// home (`Staged::ByHome`): OLTP, storage, secrets, messages and fetch do,
/// and Airhouse's `query` decides by its mode. Every other plane's op is
/// decided by its row alone, and its mode here is reported, not yet read
/// (`internal-docs/2026-11-02-custom-app-branches-design.md` §3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Plane {
    Build,
    Semantic,
    Reads,
    Oltp,
    Airhouse,
    Warehouse,
    Storage,
    Secrets,
    Messages,
    Fetch,
    Schedules,
    WorkspaceActions,
}

impl Plane {
    /// Every plane, in declaration order.
    pub const ALL: &'static [Plane] = &[
        Plane::Build,
        Plane::Semantic,
        Plane::Reads,
        Plane::Oltp,
        Plane::Airhouse,
        Plane::Warehouse,
        Plane::Storage,
        Plane::Secrets,
        Plane::Messages,
        Plane::Fetch,
        Plane::Schedules,
        Plane::WorkspaceActions,
    ];

    /// The plane's stable kebab-case key.
    pub const fn key(self) -> &'static str {
        match self {
            Plane::Build => "build",
            Plane::Semantic => "semantic",
            Plane::Reads => "reads",
            Plane::Oltp => "oltp",
            Plane::Airhouse => "airhouse",
            Plane::Warehouse => "warehouse",
            Plane::Storage => "storage",
            Plane::Secrets => "secrets",
            Plane::Messages => "messages",
            Plane::Fetch => "fetch",
            Plane::Schedules => "schedules",
            Plane::WorkspaceActions => "workspace-actions",
        }
    }
}
