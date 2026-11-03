//! The columns of a host-op registry row (`registry`), each a closed set:
//! the [`Family`] an op arrives on, the manifest [`Capability`] it is
//! behind, what every non-production environment decides for it
//! ([`Staged`]) and how it holds ([`HoldBy`]). The row itself is
//! [`HostOpSpec`].

use super::{Decision, HostOp};
use crate::custom_apps_planes::Plane;

/// The surface a call arrives on — one per `HostCall` variant, and so per
/// `op_ctx_*` op in the bootstrap. A family with sub-ops (`warehouse`,
/// `storage`, …) carries the sub-op as a string the isolate chose; the
/// registry resolves it to a row, and an unknown one pages as
/// `<family>.other`, never as the string itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Family {
    Query,
    QueryStream,
    Fetch,
    Semantic,
    Airway,
    Warehouse,
    Tx,
    Oltp,
    Airhouse,
    Storage,
    Secrets,
    Email,
    Notify,
    Org,
    Platform,
}

impl Family {
    /// Every family, in registry order.
    pub const ALL: &'static [Family] = &[
        Family::Query,
        Family::QueryStream,
        Family::Fetch,
        Family::Semantic,
        Family::Airway,
        Family::Warehouse,
        Family::Tx,
        Family::Oltp,
        Family::Airhouse,
        Family::Storage,
        Family::Secrets,
        Family::Email,
        Family::Notify,
        Family::Org,
        Family::Platform,
    ];

    /// The family's wire name: the head of each of its ops' names.
    pub const fn wire(self) -> &'static str {
        match self {
            Family::Query => "query",
            Family::QueryStream => "query_stream",
            Family::Fetch => "fetch",
            Family::Semantic => "semantic",
            Family::Airway => "airway",
            Family::Warehouse => "warehouse",
            Family::Tx => "tx",
            Family::Oltp => "oltp",
            Family::Airhouse => "airhouse",
            Family::Storage => "storage",
            Family::Secrets => "secrets",
            Family::Email => "email",
            Family::Notify => "notify",
            Family::Org => "org",
            Family::Platform => "platform",
        }
    }

    /// The family named `wire`, or `None` for a name off the list.
    pub fn from_wire(wire: &str) -> Option<Family> {
        Family::ALL.iter().copied().find(|f| f.wire() == wire)
    }

    /// The fixed name a sub-op off the list pages under: `<family>.other`
    /// for the families whose sub-op the isolate chooses, `other` elsewhere.
    pub const fn other_name(self) -> &'static str {
        match self {
            Family::Warehouse => "warehouse.other",
            Family::Tx => "tx.other",
            Family::Oltp => "oltp.other",
            Family::Airhouse => "airhouse.other",
            Family::Storage => "storage.other",
            _ => "other",
        }
    }

    /// What the host says to a sub-op off the list. `ctx.storage` names its
    /// surface; the other families say only what was asked.
    pub fn unknown_op_message(self, op: &str) -> String {
        match self {
            Family::Storage => format!("ctx.storage: unknown op '{op}'"),
            _ => format!("unknown op '{op}'"),
        }
    }
}

/// The manifest gate a host op is behind, checked by the broker before the
/// op's implementation runs (`FunctionCapabilities::allows`). Each variant is
/// one `hostField` of `oxyc`'s and the SDK's gate maps
/// (`cli_capabilities_drift`, `sdk_testing_drift`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Capability {
    /// No manifest gate: every function may ask.
    None,
    /// `secrets.write` — `ctx.secrets.set`.
    SecretsWrite,
    /// `email.send`.
    EmailSend,
    /// `notify` — the declared kinds; the kind named is checked by the op.
    Notify,
    /// `org.read` — the three `ctx.org` reads.
    OrgRead,
    /// `platform.facts`; the op then also requires an internal org.
    PlatformFacts,
    /// `storage.read`.
    StorageRead,
    /// `storage.write`.
    StorageWrite,
    /// Both: `storage.copy` reads its source and writes its destination.
    StorageReadAndWrite,
    /// `oltp.enabled`, resolved against the app's slug (`WriterCapability`).
    Oltp,
    /// `airhouse.enabled`, resolved the same way.
    Airhouse,
    /// A customer-warehouse write: gated per database the call names
    /// (`destinations` and `customerWarehouseWrites`), by the host's
    /// `check_write_destination`, never at the op level — so the broker
    /// passes it and the host decides.
    WarehouseWrite,
}

impl Capability {
    /// The `FunctionCapabilities` field the gate reads, as `oxyc` and the SDK
    /// name it (`hostField`); `None` for an ungated op.
    pub const fn host_field(self) -> Option<&'static str> {
        match self {
            Capability::None => None,
            Capability::SecretsWrite => Some("secrets_write"),
            Capability::EmailSend => Some("email_send"),
            Capability::Notify => Some("notify_kinds"),
            Capability::OrgRead => Some("org_read"),
            Capability::PlatformFacts => Some("platform_facts"),
            Capability::StorageRead => Some("storage_read"),
            Capability::StorageWrite => Some("storage_write"),
            Capability::StorageReadAndWrite => None,
            Capability::Oltp => Some("oltp"),
            Capability::Airhouse => Some("airhouse"),
            Capability::WarehouseWrite => Some("customer_warehouse_writes"),
        }
    }

    /// Whether an op behind this gate is refused while `field` is not
    /// granted — `storage.copy` answers to both storage fields.
    pub fn reads_field(self, field: &str) -> bool {
        match self {
            Capability::StorageReadAndWrite => matches!(field, "storage_read" | "storage_write"),
            other => other.host_field() == Some(field),
        }
    }
}

/// What every non-production environment decides for an op — staging and
/// each sandbox read one table (`env_policy` module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Staged {
    /// The decision, whatever the org.
    Decided(Decision),
    /// Decided at admission by where the op's plane lands. [`Plane::Oltp`]
    /// decides by its [`OltpHome`](super::OltpHome) (the staging branch, a
    /// sandbox schema on it, or held on production); storage, secrets and
    /// messages by their [`PlaneHomes`](super::PlaneHomes).
    ByHome(Plane),
}

/// How an op outside production is kept from running when the policy holds
/// or refuses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldBy {
    /// By not running: the broker refuses a `Hold` / `Refuse` decision
    /// before the implementation, and notes it in the held-write log under
    /// `plane` and `verb` (`host::env_guard`'s `HeldTarget`).
    Broker { plane: &'static str, verb: Verb },
    /// By a mechanism of the op's own, inside the host, because what is held
    /// depends on what the call names (a database, a statement, a handle) or
    /// the op answers held rather than throwing (`fetch`, `email.send`,
    /// `notify`). The host asks the policy itself; `staging_write_probe`
    /// proves each such op holds.
    Host,
}

/// The verb a broker-held call is logged under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    Fixed(&'static str),
    /// The sub-op the call named (`ctx.storage.put` → `put`).
    SubOp,
}

/// One registry row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostOpSpec {
    pub op: HostOp,
    /// The fixed name the op pages under (`HOST_OPS`).
    pub name: &'static str,
    pub family: Family,
    /// The member after the family in the name: the sub-op the isolate names
    /// for a family that has them (`warehouse.insert`), the one member of a
    /// dotted single-op family (`semantic.query`), `None` for a bare name
    /// (`query`, `fetch`, `notify`).
    pub sub_op: Option<&'static str>,
    pub capability: Capability,
    pub staging: Staged,
    pub holds: HoldBy,
}
