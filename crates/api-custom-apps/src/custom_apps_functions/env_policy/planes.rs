//! Which plane each host op lives on
//! (`internal-docs/2026-11-02-custom-app-branches-design.md` §3).

use super::{EnvPolicy, HostOp, PlaneHomes};
use crate::custom_apps_planes::{Plane, ResolvedPlanes};

impl EnvPolicy {
    /// The same policy running its planes in `planes`: the environment's row
    /// read over the defaults for its kind, and the homes they put it in.
    /// Ignored in production, which has no planes.
    pub fn with_planes(mut self, planes: ResolvedPlanes) -> Self {
        if !self.is_production() {
            self.homes = PlaneHomes::resolve(&planes, &self.environment);
            self.planes = Some(planes);
            self.overlay_warehouse_overrides();
        }
        self
    }

    /// The modes this run's planes are in. `None` in production.
    pub fn planes(&self) -> Option<&ResolvedPlanes> {
        self.planes.as_ref()
    }
}

impl HostOp {
    /// The plane of app state this op reads or writes. The match is
    /// exhaustive, so a new [`HostOp`] does not compile until it names one.
    /// A handle op (`tx.query`, `tx.exec`, `tx.commit`) is listed on the
    /// warehouse plane, but its real home is where its `begin` went
    /// (`EnvPolicy::decide_on_handle`).
    pub const fn plane(self) -> Plane {
        use HostOp::*;
        match self {
            Query | QueryStream | WarehouseQuery | OrgPeople | OrgPlaces | OrgAssignments
            | PlatformFacts => Plane::Reads,
            // Not a read, but the call writes nothing itself: the callee runs under
            // its own environment's policy (`host/apps_op.rs`).
            AppsCall => Plane::Reads,
            PlatformFacts | EventsChanges => Plane::Reads,
            SemanticQuery => Plane::Semantic,
            WarehouseInsert | WarehouseExec | WarehouseUpsert | TxBegin | TxQuery | TxExec
            | TxCommit | TxRollback => Plane::Warehouse,
            TxBeginOltp | OltpQuery | OltpExec | EventsPublish | TxPublish | AppsSend => {
                Plane::Oltp
            }
            AirhouseQuery | AirhouseExec | AirhouseAppend => Plane::Airhouse,
            StorageGetUploadUrl
            | StorageGetDownloadUrl
            | StoragePut
            | StorageGet
            | StorageHead
            | StorageList
            | StorageDelete
            | StorageCopy => Plane::Storage,
            SecretsSet => Plane::Secrets,
            EmailSend | Notify => Plane::Messages,
            Fetch => Plane::Fetch,
            // Listed here, but never decided by the plane's mode: Airway acts
            // on the project, so it is refused in every non-production
            // environment (`AIRWAY_FIX`) and stays `Staged::Decided`.
            AirwayRun => Plane::WorkspaceActions,
            // Listed here for the same reason, and never decided by a plane
            // either: an app's platform access acts on the org (its kiosks) or
            // the workspace (its landing zone), not on an app environment, so
            // it is refused outside production (`APP_ACCESS_FIX`).
            FrontlineDevicesList
            | FrontlineDevicesCreate
            | FrontlineDevicesReissueLink
            | FrontlineDevicesRevoke
            | SourceUploadsReport => Plane::WorkspaceActions,
        }
    }
}
