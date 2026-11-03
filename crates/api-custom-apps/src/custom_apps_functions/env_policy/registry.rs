//! The host-op registry: one row per `ctx.*` op and sub-op, and everything
//! the platform knows about it that is not its implementation.
//!
//! A row carries the op's fixed name (`HOST_OPS`, the SDK and `oxyc`
//! contract — the literals here are byte-identical to the ones they replaced
//! and are pinned by `sdk_testing_drift` and `cli_capabilities_drift`), the
//! [`Family`] it arrives on (one per `HostCall` variant), the [`Capability`]
//! the manifest must grant, what every non-production environment decides
//! for it ([`Staged`]), and how it holds ([`HoldBy`]). The broker
//! (`runtime::host_call`) reads the row before any implementation runs: an
//! op with no row is refused as unknown, one whose capability the manifest
//! lacks is refused with the host's own words, and one the environment holds
//! or refuses by not running is answered without reaching the host. A new op
//! therefore cannot ship open: until it has a row it does not compile, and
//! its row states its gate and its environment decision in one place.
//!
//! `HostOp::ALL` is in table order, which is `HOST_OPS` order.

use super::op_kinds::{Capability, Family, HoldBy, HostOpSpec, Staged, Verb};
use super::{Decision, Target};
use crate::custom_apps_planes::Plane;

/// Declares [`HostOp`], its closed list [`HostOp::ALL`] and one
/// [`HostOpSpec`] per variant, so the list cannot miss a row.
macro_rules! host_ops {
    ($( $(#[$doc:meta])* $variant:ident = $name:literal {
        family: $family:ident $(($sub:literal))?,
        capability: $cap:ident,
        staging: $staged:expr,
        holds: $holds:expr $(,)?
    }, )*) => {
        /// Every operation a function can ask the host for, one variant per
        /// entry of `host_call_attrs::HOST_OPS` (a unit test pins the two
        /// together, in order), sub-ops included.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum HostOp {
            $( $(#[$doc])* $variant, )*
        }

        impl HostOp {
            /// Every variant, in `HOST_OPS` order.
            pub const ALL: &'static [HostOp] = &[ $( HostOp::$variant, )* ];

            /// Every name, in the same order: what `HOST_OPS` is.
            pub const NAMES: &'static [&'static str] = &[ $( $name, )* ];

            /// The table, one row per variant in variant order.
            pub const SPECS: &'static [HostOpSpec] = &[ $( HostOpSpec {
                op: HostOp::$variant,
                name: $name,
                family: Family::$family,
                sub_op: host_ops!(@sub $($sub)?),
                capability: Capability::$cap,
                staging: $staged,
                holds: $holds,
            }, )* ];

            /// The op's registry row.
            pub const fn spec(self) -> &'static HostOpSpec {
                &HostOp::SPECS[self as usize]
            }
        }
    };
    (@sub $sub:literal) => { Some($sub) };
    (@sub) => { None };
}

const ALLOW: Staged = Staged::Decided(Decision::Allow);
const HOLD: Staged = Staged::Decided(Decision::Hold);
const fn isolate(target: Target) -> Staged {
    Staged::Decided(Decision::Isolate(target))
}
const fn by_not_running(plane: &'static str, verb: &'static str) -> HoldBy {
    HoldBy::Broker {
        plane,
        verb: Verb::Fixed(verb),
    }
}

/// `Refuse` fix for `ctx.airway.run` outside production.
pub const AIRWAY_FIX: &str = "an Airway run belongs to the project, not to an app environment, so \
     any run writes production. Trigger the pipeline from production instead";

host_ops! {
    // Reads of production data (§4.2), allowed everywhere.
    Query = "query" {
        family: Query, capability: None, staging: ALLOW,
        holds: by_not_running("warehouse", "QUERY"),
    },
    QueryStream = "query_stream" {
        family: QueryStream, capability: None, staging: ALLOW,
        holds: by_not_running("warehouse", "QUERY"),
    },
    /// A read method is still sent. A mutating one answers 409 unsent, unless
    /// it is this invocation's own upload into its environment's silo, or the
    /// environment's `planes.fetch` is `allow` and the call passes the
    /// provider-sandbox checks (`EnvPolicy::decide_on_fetch`). The row says
    /// which: `held-writes` (the default) holds, `allow` names the sandbox.
    Fetch = "fetch" {
        family: Fetch, capability: None, staging: Staged::ByHome(Plane::Fetch),
        holds: HoldBy::Host,
    },
    SemanticQuery = "semantic.query" {
        family: Semantic("query"), capability: None, staging: ALLOW,
        holds: by_not_running("semantic", "QUERY"),
    },
    /// A run has no environment, so there is no copy it could write instead.
    AirwayRun = "airway.run" {
        family: Airway("run"), capability: None,
        staging: Staged::Decided(Decision::Refuse { fix: AIRWAY_FIX }),
        holds: by_not_running("airway", "RUN"),
    },
    // A customer's warehouse: the database `nonProduction.destinations` maps
    // it to (P5b); unmapped, it holds (`decide_on_database`).
    WarehouseInsert = "warehouse.insert" {
        family: Warehouse("insert"), capability: WarehouseWrite,
        staging: isolate(Target::MappedDestination), holds: HoldBy::Host,
    },
    WarehouseExec = "warehouse.exec" {
        family: Warehouse("exec"), capability: WarehouseWrite,
        staging: isolate(Target::MappedDestination), holds: HoldBy::Host,
    },
    WarehouseUpsert = "warehouse.upsert" {
        family: Warehouse("upsert"), capability: WarehouseWrite,
        staging: isolate(Target::MappedDestination), holds: HoldBy::Host,
    },
    WarehouseQuery = "warehouse.query" {
        family: Warehouse("query"), capability: None, staging: ALLOW,
        holds: by_not_running("warehouse", "QUERY"),
    },
    TxBegin = "tx.begin" {
        family: Tx("begin"), capability: WarehouseWrite,
        staging: isolate(Target::MappedDestination), holds: HoldBy::Host,
    },
    // The app's OLTP store (P4b): on the org's staging branch when it has
    // one; without one a single read is sent in a READ ONLY transaction and
    // every other statement is held unsent. A statement or commit on a
    // `ctx.oltp.tx` handle follows where its `begin` went
    // (`EnvPolicy::decide_on_handle`); a held commit rolls back.
    TxBeginOltp = "tx.begin_oltp" {
        family: Tx("begin_oltp"), capability: Oltp, staging: Staged::ByHome(Plane::Oltp),
        holds: HoldBy::Host,
    },
    TxQuery = "tx.query" {
        family: Tx("query"), capability: None, staging: HOLD, holds: HoldBy::Host,
    },
    TxExec = "tx.exec" {
        family: Tx("exec"), capability: None, staging: HOLD, holds: HoldBy::Host,
    },
    TxCommit = "tx.commit" {
        family: Tx("commit"), capability: None, staging: HOLD, holds: HoldBy::Host,
    },
    /// Writes nothing.
    TxRollback = "tx.rollback" {
        family: Tx("rollback"), capability: None, staging: ALLOW, holds: HoldBy::Host,
    },
    OltpQuery = "oltp.query" {
        family: Oltp("query"), capability: Oltp, staging: Staged::ByHome(Plane::Oltp),
        holds: HoldBy::Host,
    },
    OltpExec = "oltp.exec" {
        family: Oltp("exec"), capability: Oltp, staging: Staged::ByHome(Plane::Oltp),
        holds: HoldBy::Host,
    },
    /// Reads production's schema under `write-only`; under `branch` and
    /// `snapshot`, through the environment's overlay (`airhouse_overlay`).
    AirhouseQuery = "airhouse.query" {
        family: Airhouse("query"), capability: Airhouse,
        staging: Staged::ByHome(Plane::Airhouse),
        holds: HoldBy::Host,
    },
    // The app schema's sibling, `app_<writer>__<env>` (P5b); a schema that
    // names no sibling holds (`decide_in_schema`).
    AirhouseExec = "airhouse.exec" {
        family: Airhouse("exec"), capability: Airhouse,
        staging: isolate(Target::SiblingSchema), holds: HoldBy::Host,
    },
    AirhouseAppend = "airhouse.append" {
        family: Airhouse("append"), capability: Airhouse,
        staging: isolate(Target::SiblingSchema), holds: HoldBy::Host,
    },
    // The environment's own storage silo (P5a). Reads look there first, then
    // at production's same key, read-only; `list` is the silo alone.
    StorageGetUploadUrl = "storage.getUploadUrl" {
        family: Storage("getUploadUrl"), capability: StorageWrite,
        staging: Staged::ByHome(Plane::Storage), holds: STORAGE_HOLD,
    },
    StorageGetDownloadUrl = "storage.getDownloadUrl" {
        family: Storage("getDownloadUrl"), capability: StorageRead,
        staging: Staged::ByHome(Plane::Storage), holds: STORAGE_HOLD,
    },
    StoragePut = "storage.put" {
        family: Storage("put"), capability: StorageWrite,
        staging: Staged::ByHome(Plane::Storage), holds: STORAGE_HOLD,
    },
    StorageGet = "storage.get" {
        family: Storage("get"), capability: StorageRead,
        staging: Staged::ByHome(Plane::Storage), holds: STORAGE_HOLD,
    },
    StorageHead = "storage.head" {
        family: Storage("head"), capability: StorageRead,
        staging: Staged::ByHome(Plane::Storage), holds: STORAGE_HOLD,
    },
    StorageList = "storage.list" {
        family: Storage("list"), capability: StorageRead,
        staging: Staged::ByHome(Plane::Storage), holds: STORAGE_HOLD,
    },
    StorageDelete = "storage.delete" {
        family: Storage("delete"), capability: StorageWrite,
        staging: Staged::ByHome(Plane::Storage), holds: STORAGE_HOLD,
    },
    StorageCopy = "storage.copy" {
        family: Storage("copy"), capability: StorageReadAndWrite,
        staging: Staged::ByHome(Plane::Storage), holds: STORAGE_HOLD,
    },
    /// The environment's secret path (P5a).
    SecretsSet = "secrets.set" {
        family: Secrets("set"), capability: SecretsWrite,
        staging: Staged::ByHome(Plane::Secrets), holds: by_not_running("secrets", "SET"),
    },
    /// Invoker-only email (P5a); where held, resolves `{ held: true }`.
    EmailSend = "email.send" {
        family: Email("send"), capability: EmailSend,
        staging: Staged::ByHome(Plane::Messages), holds: HoldBy::Host,
    },
    /// The invoking user alone; where held, answered rather than thrown.
    Notify = "notify" {
        family: Notify, capability: Notify,
        staging: Staged::ByHome(Plane::Messages), holds: HoldBy::Host,
    },
    OrgPeople = "org.people" {
        family: Org("people"), capability: OrgRead, staging: ALLOW,
        holds: by_not_running("org", "READ"),
    },
    OrgPlaces = "org.places" {
        family: Org("places"), capability: OrgRead, staging: ALLOW,
        holds: by_not_running("org", "READ"),
    },
    OrgAssignments = "org.assignments" {
        family: Org("assignments"), capability: OrgRead, staging: ALLOW,
        holds: by_not_running("org", "READ"),
    },
    /// Ids and counts the platform holds, the same in every environment.
    PlatformFacts = "platform.facts" {
        family: Platform("facts"), capability: PlatformFacts, staging: ALLOW,
        holds: by_not_running("platform", "READ"),
    },
}

/// A `ctx.storage` op is logged under the sub-op it named.
const STORAGE_HOLD: HoldBy = HoldBy::Broker {
    plane: "storage",
    verb: Verb::SubOp,
};

impl HostOp {
    /// The fixed name the op pages under (`HOST_OPS`).
    pub const fn name(self) -> &'static str {
        self.spec().name
    }

    /// The op's family — the `HostCall` variant it arrives on.
    pub const fn family(self) -> Family {
        self.spec().family
    }

    /// The sub-op the isolate names for this op, when its family has them.
    pub const fn sub_name(self) -> Option<&'static str> {
        self.spec().sub_op
    }

    /// The op named `name`, or `None` for a name off the list.
    pub fn from_name(name: &str) -> Option<HostOp> {
        HostOp::ALL.iter().copied().find(|op| op.name() == name)
    }

    /// The op of `family` whose sub-op is `sub_op`, or `None` for one off the
    /// list — which the broker then refuses as unknown.
    pub fn of(family: Family, sub_op: Option<&str>) -> Option<HostOp> {
        HostOp::ALL
            .iter()
            .copied()
            .find(|op| op.family() == family && op.sub_name() == sub_op)
    }

    /// The sub-op `op` of the family named `family` (`"warehouse"`,
    /// `"insert"`), or `None` for one off the list.
    pub fn sub_op(family: &str, op: &str) -> Option<HostOp> {
        HostOp::of(Family::from_wire(family)?, Some(op))
    }

    /// Every op refused while the `FunctionCapabilities` field `field` is not
    /// granted, by name — what the SDK's and `oxyc`'s gate maps list.
    pub fn gated_by(field: &str) -> Vec<&'static str> {
        HostOp::ALL
            .iter()
            .filter(|op| op.spec().capability.reads_field(field))
            .map(|op| op.name())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name is its family's wire name and its sub-op, so the two columns
    /// cannot disagree with the literal the SDK reads.
    #[test]
    fn every_name_is_its_family_and_sub_op() {
        for op in HostOp::ALL {
            let spec = op.spec();
            let derived = match spec.sub_op {
                Some(sub) => format!("{}.{sub}", spec.family.wire()),
                None => spec.family.wire().to_string(),
            };
            assert_eq!(spec.name, derived, "{op:?}");
            assert_eq!(HostOp::of(spec.family, spec.sub_op), Some(*op));
        }
        assert_eq!(HostOp::NAMES.len(), HostOp::ALL.len());
        // `spec` indexes the table by discriminant.
        for (i, op) in HostOp::ALL.iter().enumerate() {
            assert_eq!(HostOp::SPECS[i].op, *op);
            assert_eq!(HostOp::NAMES[i], op.name());
        }
    }

    #[test]
    fn a_family_with_sub_ops_pages_an_unknown_one_under_other() {
        assert_eq!(HostOp::sub_op("storage", "rename"), None);
        assert_eq!(Family::Storage.other_name(), "storage.other");
        assert_eq!(Family::Email.other_name(), "other");
        assert_eq!(Family::from_wire("nope"), None);
    }

    #[test]
    fn gate_fields_list_the_ops_they_refuse() {
        assert_eq!(
            HostOp::gated_by("storage_write"),
            [
                "storage.getUploadUrl",
                "storage.put",
                "storage.delete",
                "storage.copy"
            ]
        );
        assert_eq!(
            HostOp::gated_by("oltp"),
            ["tx.begin_oltp", "oltp.query", "oltp.exec"]
        );
        assert!(HostOp::gated_by("fetch_max_bytes").is_empty());
    }
}
