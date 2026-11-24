//! What one install does, decided before anything changes: every declared
//! table checked against the shape its store has now, then which tables are
//! captured for the first time, which have a new shape, which are captured as
//! they stand, and which captured tables the build no longer declares. Pure,
//! so each of those decisions is pinned by a unit test here.

use oxy_custom_apps::manifest::OltpChangesConfig;
use oxy_oltp::changes::{
    CaptureShape, Column, DeclaredTable, TableShape, next_schema_version, validate,
};

use super::InstallSummary;

/// The tables `config` declares, in name order (`tables` is a `BTreeMap`).
pub(super) fn declared_tables(config: Option<&OltpChangesConfig>) -> Vec<DeclaredTable> {
    config
        .map(|config| {
            config
                .tables
                .iter()
                .map(|(name, table)| DeclaredTable {
                    name: name.clone(),
                    exclude: table.exclude.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every declared table against the shape read for it (`None`: no such
/// table), all of them before any is installed. `Err` is the refusal, naming
/// each table at fault and its fix, so one republish fixes them all.
pub(super) fn validate_all(
    read: &[(DeclaredTable, Option<TableShape>)],
) -> Result<Vec<CaptureShape>, String> {
    let mut captures = Vec::with_capacity(read.len());
    let mut refusals = Vec::new();
    for (decl, shape) in read {
        match validate(decl, shape.as_ref()) {
            Ok(capture) => captures.push(capture),
            Err(refusal) => refusals.push(refusal.to_string()),
        }
    }
    if refusals.is_empty() {
        Ok(captures)
    } else {
        Err(refusal(&refusals))
    }
}

/// The install's refusal, naming every table at fault (`refusals`, one each)
/// and saying that nothing changed: production keeps its previous build.
pub(super) fn refusal(refusals: &[String]) -> String {
    format!(
        "change capture was not installed, so production keeps its previous build and every \
         table's capture stays as it was: {}",
        refusals.join("; ")
    )
}

/// A table this writer's capture covers, as `oxy_oltp::changes::INSTALLED_SQL`
/// lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Installed {
    pub relid: u32,
    /// Its name when it was installed: what the summary reports.
    pub table: String,
    /// The `schema_version` its functions bake in.
    pub schema_version: i32,
    /// False: an install recorded it and did not reach its commit.
    pub installed: bool,
}

/// A table's latest recorded shape (`oxy_changes.shapes`), kept whether or
/// not the table is captured now, so a table declared again continues its
/// versions instead of reusing one whose columns differ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Recorded {
    pub version: i32,
    pub columns: Vec<Column>,
    pub primary_key: Vec<String>,
}

/// What an install does to one declared table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Change {
    /// Not captured before, or an earlier install of it never committed.
    New,
    /// Captured, and its captured columns or key changed: a new
    /// `schema_version`.
    Reshaped,
    /// Captured as it stands: installed again at its version, which puts
    /// back anything that drifted.
    Unchanged,
}

/// One declared table's install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TableInstall {
    pub capture: CaptureShape,
    pub schema_version: i32,
    pub change: Change,
}

/// The whole install: each declared table in name order, then each captured
/// table no longer declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Plan {
    pub install: Vec<TableInstall>,
    pub remove: Vec<Installed>,
}

/// The plan for `captures` (each validated table with its latest recorded
/// shape) against what `installed` says the writer's capture covers. Tables
/// are matched by oid, so a table dropped and created again under its name is
/// a new table, and the old one's row is removed.
pub(super) fn plan(
    captures: Vec<(CaptureShape, Option<Recorded>)>,
    installed: &[Installed],
) -> Plan {
    let install: Vec<TableInstall> = captures
        .into_iter()
        .map(|(capture, recorded)| {
            let latest = recorded
                .as_ref()
                .map(|r| (r.version, r.columns.as_slice(), r.primary_key.as_slice()));
            let schema_version = next_schema_version(latest, &capture);
            let change = match installed
                .iter()
                .find(|row| row.relid == capture.shape.relid)
            {
                Some(row) if row.installed && row.schema_version == schema_version => {
                    Change::Unchanged
                }
                Some(row) if row.installed => Change::Reshaped,
                _ => Change::New,
            };
            TableInstall {
                capture,
                schema_version,
                change,
            }
        })
        .collect();
    let remove = installed
        .iter()
        .filter(|row| !install.iter().any(|t| t.capture.shape.relid == row.relid))
        .cloned()
        .collect();
    Plan { install, remove }
}

impl Plan {
    /// What the plan installs, reshapes and removes, by table name.
    pub(super) fn summary(&self) -> InstallSummary {
        let named = |change: Change| -> Vec<String> {
            self.install
                .iter()
                .filter(|t| t.change == change)
                .map(|t| t.capture.shape.table.clone())
                .collect()
        };
        InstallSummary {
            installed: named(Change::New),
            reshaped: named(Change::Reshaped),
            removed: self.remove.iter().map(|row| row.table.clone()).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use oxy_custom_apps::manifest::{OltpChangesConfig, OltpChangesTable};
    use oxy_oltp::changes::{CaptureShape, Column, DeclaredTable, TableShape, validate};

    use super::{Change, Installed, Recorded, declared_tables, plan, validate_all};

    fn column(attnum: i16, name: &str, type_name: &str) -> Column {
        Column {
            attnum,
            name: name.into(),
            type_name: type_name.into(),
        }
    }

    /// `name (id bigint PRIMARY KEY, body text)` in `app_x`, oid `relid`.
    fn shape(relid: u32, name: &str) -> TableShape {
        TableShape {
            relid,
            schema: "app_x".into(),
            table: name.into(),
            columns: vec![column(1, "id", "bigint"), column(2, "body", "text")],
            primary_key: vec!["id".into()],
        }
    }

    fn declared(name: &str) -> DeclaredTable {
        DeclaredTable {
            name: name.into(),
            exclude: Vec::new(),
        }
    }

    fn capture(relid: u32, name: &str) -> CaptureShape {
        validate(&declared(name), Some(&shape(relid, name))).expect("capturable")
    }

    /// The shape `capture` records, at `version`.
    fn recorded(capture: &CaptureShape, version: i32) -> Recorded {
        Recorded {
            version,
            columns: capture.captured.clone(),
            primary_key: capture.shape.primary_key.clone(),
        }
    }

    fn installed(relid: u32, table: &str, schema_version: i32, done: bool) -> Installed {
        Installed {
            relid,
            table: table.into(),
            schema_version,
            installed: done,
        }
    }

    #[test]
    fn declared_tables_follow_the_manifest_in_name_order() {
        let config = OltpChangesConfig {
            tables: BTreeMap::from([
                (
                    "leases".to_string(),
                    OltpChangesTable {
                        exclude: vec!["contact_email".into()],
                    },
                ),
                ("addresses".to_string(), OltpChangesTable::default()),
            ]),
        };
        let tables = declared_tables(Some(&config));
        let names: Vec<&str> = tables.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["addresses", "leases"]);
        assert_eq!(tables[1].exclude, ["contact_email"]);
        assert!(declared_tables(None).is_empty());
    }

    /// One table at fault refuses the install, and the refusal names every
    /// table at fault, not only the first; a good table names nothing.
    #[test]
    fn every_refusal_is_listed() {
        let mut keyless = shape(3, "notes");
        keyless.primary_key.clear();
        let refusal = validate_all(&[
            (declared("leases"), Some(shape(1, "leases"))),
            (declared("ghosts"), None),
            (declared("notes"), Some(keyless)),
        ])
        .expect_err("two tables are at fault");
        assert!(refusal.contains("\"ghosts\""), "{refusal}");
        assert!(refusal.contains("does not exist"), "{refusal}");
        assert!(refusal.contains("\"notes\""), "{refusal}");
        assert!(refusal.contains("no primary key"), "{refusal}");
        assert!(!refusal.contains("\"leases\""), "{refusal}");
        assert!(refusal.contains("previous build"), "{refusal}");

        let captures =
            validate_all(&[(declared("leases"), Some(shape(1, "leases")))]).expect("capturable");
        assert_eq!(captures.len(), 1);
    }

    #[test]
    fn a_table_never_captured_is_new_at_version_one() {
        let planned = plan(vec![(capture(1, "leases"), None)], &[]);
        assert_eq!(planned.install.len(), 1);
        assert_eq!(planned.install[0].schema_version, 1);
        assert_eq!(planned.install[0].change, Change::New);
        assert!(planned.remove.is_empty());
        let summary = planned.summary();
        assert_eq!(summary.installed, ["leases"]);
        assert!(summary.reshaped.is_empty() && summary.removed.is_empty());
    }

    /// Installed again as it stands: the same version, and nothing for the
    /// summary to report.
    #[test]
    fn an_unchanged_table_keeps_its_version() {
        let leases = capture(1, "leases");
        let latest = recorded(&leases, 3);
        let planned = plan(
            vec![(leases, Some(latest))],
            &[installed(1, "leases", 3, true)],
        );
        assert_eq!(planned.install[0].schema_version, 3);
        assert_eq!(planned.install[0].change, Change::Unchanged);
        assert!(planned.summary().is_empty());
    }

    #[test]
    fn a_changed_shape_is_reshaped_at_the_next_version() {
        let leases = capture(1, "leases");
        let mut before = recorded(&leases, 2);
        before.columns.pop();
        let planned = plan(
            vec![(leases, Some(before))],
            &[installed(1, "leases", 2, true)],
        );
        assert_eq!(planned.install[0].schema_version, 3);
        assert_eq!(planned.install[0].change, Change::Reshaped);
        assert_eq!(planned.summary().reshaped, ["leases"]);
    }

    /// An install that recorded the table and failed before its commit left
    /// `installed` false: the next one installs it as new.
    #[test]
    fn an_uncommitted_install_is_installed_again_as_new() {
        let leases = capture(1, "leases");
        let latest = recorded(&leases, 1);
        let planned = plan(
            vec![(leases, Some(latest))],
            &[installed(1, "leases", 1, false)],
        );
        assert_eq!(planned.install[0].schema_version, 1);
        assert_eq!(planned.install[0].change, Change::New);
    }

    /// A table removed earlier keeps its shapes; declared again, it continues
    /// from the latest, rather than recording a different shape under a
    /// version the relay already read.
    #[test]
    fn a_table_declared_again_continues_its_versions() {
        let leases = capture(1, "leases");
        let mut old = recorded(&leases, 4);
        old.columns.push(column(3, "contact_email", "text"));
        let planned = plan(vec![(leases, Some(old))], &[]);
        assert_eq!(planned.install[0].schema_version, 5);
        assert_eq!(planned.install[0].change, Change::New);
    }

    #[test]
    fn a_captured_table_no_longer_declared_is_removed() {
        let planned = plan(
            vec![(capture(2, "landlords"), None)],
            &[
                installed(1, "leases", 1, true),
                installed(2, "landlords", 1, true),
                installed(3, "drafts", 1, false),
            ],
        );
        let removed: Vec<u32> = planned.remove.iter().map(|row| row.relid).collect();
        assert_eq!(removed, [1, 3]);
        assert_eq!(planned.summary().removed, ["leases", "drafts"]);
    }

    /// A table dropped and created again under its name has a new oid: it is
    /// installed as new, and the old oid's row is removed.
    #[test]
    fn a_table_created_again_is_new_and_its_old_row_removed() {
        let planned = plan(
            vec![(capture(9, "leases"), None)],
            &[installed(1, "leases", 2, true)],
        );
        assert_eq!(planned.install[0].change, Change::New);
        assert_eq!(planned.install[0].schema_version, 1);
        assert_eq!(planned.remove, [installed(1, "leases", 2, true)]);
    }
}
