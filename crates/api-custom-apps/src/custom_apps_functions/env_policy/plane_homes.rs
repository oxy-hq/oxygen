//! Where the planes that have a home land
//! (`internal-docs/2026-11-02-custom-app-branches-design.md` §5.2).
//!
//! [`OltpHome`] decides where `ctx.oltp` lands, once, at admission. This is
//! the same for the other planes that gain a second home in P1: storage
//! (the silo, with or without the fallback read), secrets (the chain, or the
//! environment's own path), messages (the invoker, or a capture), workspace
//! actions (held, or refused) and, from P4, fetch (held, or a provider sandbox). They are resolved from the
//! environment's [`ResolvedPlanes`] and carried on the policy beside it
//! (`EnvPolicy::with_planes`), so a host call asks one place.
//!
//! **A home only ever narrows.** No home decides `Allow`, and none names
//! production's silo, secret path or recipient: a mode a plane gains later is
//! a new variant here, and `plane_homes_tests` ranges over every mode of every
//! plane to hold it to that. The warehouse plane's home (the destination map)
//! arrives with its override; its ops still decide by their registry rows.

use oxy_app_core::custom_app_environment::{AppEnvironment, AppEnvironmentKind};

use super::{Decision, EnvPolicy, HostOp, MISROUTED_FIX, Target};
use crate::custom_apps_planes::{
    FetchMode, MessagesMode, Plane, PlaneSpec, ResolvedPlanes, SecretsMode, StorageMode,
    WorkspaceActionsMode,
};
use crate::custom_apps_secrets::scope::EnvLookup;

/// Where `ctx.storage` works. Writes always land in the environment's own
/// silo; the home says what a read that misses it may see.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StorageHome {
    /// The silo, then production's same relative key, read-only. Today's
    /// behaviour.
    SiloWithFallback,
    /// The silo alone: a key that exists only in production reads as absent.
    SiloOnly,
}

/// Where `ctx.env` is read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SecretsHome {
    /// The environment's own path, then the fallbacks of its kind (a sandbox
    /// reads staging's, and any environment reads production's `shared`
    /// keys). Today's behaviour.
    Chain,
    /// The environment's own path alone.
    OwnOnly,
}

/// Who `ctx.email.send` and `ctx.notify` reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MessagesHome {
    /// The invoking user only, with an `[env]` prefix. Today's behaviour.
    Invoker,
    /// Nobody: the rendered message is kept in the environment's outbox, in
    /// its own storage silo (`custom_apps_outbox`).
    Capture,
}

/// What a bundle's writes through the workspace `/api` do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorkspaceActionsHome {
    /// An agent ask or an automation run is held, as staging's is.
    Held,
    /// Refused with a `403`.
    Refused,
    /// An agent ask is held; an automation run runs as a workspace-preview
    /// dry run.
    Preview,
}

/// Where a mutating `ctx.fetch` goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FetchHome {
    /// Held, as staging's is. Today's behaviour.
    HeldWrites,
    /// Sent, but only to a registered provider sandbox the environment lists,
    /// which `EnvPolicy::decide_on_fetch` confirms call by call.
    ProviderSandbox,
}

/// The homes of the planes that have one, for one non-production environment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PlaneHomes {
    pub storage: StorageHome,
    pub secrets: SecretsHome,
    pub messages: MessagesHome,
    pub workspace_actions: WorkspaceActionsHome,
    pub fetch: FetchHome,
}

impl SecretsHome {
    /// How far `ctx.env` looks under this home.
    pub const fn lookup(self) -> EnvLookup {
        match self {
            SecretsHome::Chain => EnvLookup::Chain,
            SecretsHome::OwnOnly => EnvLookup::OwnOnly,
        }
    }
}

impl PlaneHomes {
    /// The homes `planes` puts `environment` in. `None` for production, which
    /// has no planes and so no homes: it works in the live ones.
    pub fn resolve(planes: &ResolvedPlanes, environment: &AppEnvironment) -> Option<PlaneHomes> {
        if *environment == AppEnvironment::Production {
            return None;
        }
        Some(PlaneHomes {
            storage: match planes.storage {
                StorageMode::Branch => StorageHome::SiloWithFallback,
                StorageMode::Empty => StorageHome::SiloOnly,
            },
            secrets: match planes.secrets {
                SecretsMode::Inherit => SecretsHome::Chain,
                SecretsMode::Empty => SecretsHome::OwnOnly,
            },
            messages: match planes.messages {
                MessagesMode::Invoker => MessagesHome::Invoker,
                MessagesMode::Capture => MessagesHome::Capture,
            },
            workspace_actions: match planes.workspace_actions {
                WorkspaceActionsMode::Held => WorkspaceActionsHome::Held,
                WorkspaceActionsMode::Refused => WorkspaceActionsHome::Refused,
                WorkspaceActionsMode::Preview => WorkspaceActionsHome::Preview,
            },
            fetch: match planes.fetch {
                FetchMode::HeldWrites => FetchHome::HeldWrites,
                FetchMode::Allow => FetchHome::ProviderSandbox,
            },
        })
    }

    /// The homes of an environment of `kind` whose row sets nothing: today's.
    /// `None` for production.
    pub fn default_for(kind: AppEnvironmentKind) -> Option<PlaneHomes> {
        // The defaults depend on the kind alone, so any environment of it
        // will do; a sandbox needs a handle to be one.
        let environment = match kind {
            AppEnvironmentKind::Production => return None,
            AppEnvironmentKind::Staging => AppEnvironment::Staging,
            AppEnvironmentKind::Dev => AppEnvironment::Dev {
                handle: "default".to_string(),
            },
        };
        let planes = PlaneSpec::default().resolve(&environment)?;
        PlaneHomes::resolve(&planes, &environment)
    }

    /// What an op that decides by its plane's home does here, for the planes
    /// this type holds a home of. Any other op fails closed: it should have
    /// decided by its registry row.
    pub(super) fn decision(&self, op: HostOp) -> Decision {
        match (op.plane(), op) {
            (Plane::Storage, _) => match self.storage {
                StorageHome::SiloWithFallback | StorageHome::SiloOnly => {
                    Decision::Isolate(Target::StorageSilo)
                }
            },
            (Plane::Secrets, _) => match self.secrets {
                SecretsHome::Chain | SecretsHome::OwnOnly => Decision::Isolate(Target::EnvSecrets),
            },
            (Plane::Messages, HostOp::EmailSend) => match self.messages {
                MessagesHome::Invoker => Decision::Isolate(Target::InvokerEmail),
                MessagesHome::Capture => Decision::Isolate(Target::Outbox),
            },
            (Plane::Messages, HostOp::Notify) => match self.messages {
                MessagesHome::Invoker => Decision::Isolate(Target::InvokerNotify),
                MessagesHome::Capture => Decision::Isolate(Target::Outbox),
            },
            (Plane::Fetch, _) => match self.fetch {
                FetchHome::HeldWrites => Decision::Hold,
                FetchHome::ProviderSandbox => Decision::Isolate(Target::ProviderSandbox),
            },
            _ => Decision::Refuse { fix: MISROUTED_FIX },
        }
    }
}

impl EnvPolicy {
    /// The homes this run's planes are in. `None` in production.
    pub fn homes(&self) -> Option<&PlaneHomes> {
        self.homes.as_ref()
    }
}
