//! The warehouse plane's choice: its mode, and the per-environment overrides
//! of the build's `nonProduction.destinations`
//! (`internal-docs/2026-11-02-custom-app-branches-design.md` §5.5).
//!
//! On the wire the plane is the bare mode string (`"mapped"`, no overrides, the
//! only form until now) or an object
//! `{"mode": "mapped", "destinations": {"<db>": "<mapped>"}}`. The object form
//! is the shape a plane that grows settings takes; fetch hosts reuse it.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer};
use serde_json::Value;

use super::WarehouseMode;
use super::request::PlaneError;

/// The warehouse plane, resolved: its mode and the overrides. Each override
/// replaces the build manifest's mapping for that production database, key by
/// key; the manifest's other keys stay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WarehousePlane {
    pub mode: WarehouseMode,
    /// Production database → the database this environment writes instead.
    /// A `BTreeMap`, so equality and anything printed from it are
    /// deterministic.
    pub destinations: BTreeMap<String, String>,
}

impl Default for WarehousePlane {
    /// Today's behaviour: mapped, no overrides.
    fn default() -> Self {
        WarehousePlane {
            mode: WarehouseMode::Mapped,
            destinations: BTreeMap::new(),
        }
    }
}

/// The object form, strict: an unknown key is an error, as everywhere in
/// `planes`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Object {
    mode: WarehouseMode,
    #[serde(default)]
    destinations: BTreeMap<String, String>,
}

impl<'de> Deserialize<'de> for WarehousePlane {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Not `untagged`: its error would be "did not match any variant",
        // naming neither the bad mode nor the bad key.
        match Value::deserialize(deserializer)? {
            value @ Value::String(_) => {
                let mode = serde_json::from_value(value).map_err(serde::de::Error::custom)?;
                Ok(WarehousePlane {
                    mode,
                    destinations: BTreeMap::new(),
                })
            }
            value @ Value::Object(_) => {
                let Object { mode, destinations } =
                    serde_json::from_value(value).map_err(serde::de::Error::custom)?;
                Ok(WarehousePlane { mode, destinations })
            }
            _ => Err(serde::de::Error::custom(
                "the warehouse plane is a mode string like \"mapped\", or an object like \
                 {\"mode\": \"mapped\", \"destinations\": {\"<db>\": \"<mapped>\"}}",
            )),
        }
    }
}

impl WarehousePlane {
    /// The checks on the overrides that need no fact about the workspace: each
    /// name is a database name, an override is not a mapping onto its own
    /// production database, and none starts a chain (a mapped name that is
    /// itself overridden is a production database). The same rules as
    /// `nonProduction.destinations` at publish; the checks that need the
    /// workspace's databases run beforehand and arrive as
    /// `PlaneContext::warehouse_refusal`.
    pub(super) fn check(&self) -> Result<(), PlaneError> {
        for (from, to) in &self.destinations {
            if from.trim().is_empty() || to.trim().is_empty() {
                return Err(PlaneError(format!(
                    "planes.warehouse.destinations maps {from:?} to {to:?}; both must name a database"
                )));
            }
            if from == to {
                return Err(PlaneError(format!(
                    "planes.warehouse.destinations maps `{from}` onto itself, which would write \
                     production"
                )));
            }
            if self.destinations.contains_key(to) {
                return Err(PlaneError(format!(
                    "planes.warehouse.destinations maps `{from}` to `{to}`, which it also maps as \
                     a production database of its own — the environment would write a production \
                     database. Map `{from}` to a database no mapping starts from"
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "warehouse_tests.rs"]
mod tests;
