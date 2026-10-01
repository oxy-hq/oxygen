//! Table tests over pokehouse-shaped specs. Pure: no database, no network.
//! Two cases build real connectors offline (placeholder credentials, no I/O),
//! the chain the preview analyze task runs, so the resource names classify
//! reads are the ones a connector actually advertises.

use std::collections::HashMap;

use airway::connector::{Environment, ResourceInfo};
use airway::schema::{Column, Table};
use airway::types::{ColumnHints, DataType, WriteDisposition};

use super::*;
use crate::placeholder::substitute_secret_vars;

fn spec(yaml: &str) -> AirwayPipelineSpec {
    AirwayPipelineSpec::from_yaml_str(yaml).expect("fixture spec parses")
}

fn res(name: &str, wd: WriteDisposition, pk: Option<&[&str]>) -> ResourceInfo {
    ResourceInfo {
        name: name.to_string(),
        description: None,
        write_disposition: wd,
        primary_key: pk.map(|k| k.iter().map(|s| s.to_string()).collect()),
        cursor_field: None,
    }
}

fn table(name: &str, wd: WriteDisposition, cols: &[(&str, DataType, bool)]) -> Table {
    let mut t = Table::new(name);
    t.write_disposition = wd;
    for (col, dt, pk) in cols {
        let mut c = Column::new(*col, dt.clone());
        c.primary_key = *pk;
        t.columns.insert(col.to_string(), c);
    }
    t
}

fn schema(tables: Vec<Table>) -> Schema {
    let mut s = Schema::new("stored");
    for t in tables {
        s.tables.insert(t.name.clone(), t);
    }
    s
}

fn live_cols(tables: &[&str]) -> Vec<LiveColumn> {
    tables
        .iter()
        .map(|t| LiveColumn {
            table: t.to_string(),
            column: "id".into(),
            data_type: "VARCHAR".into(),
            nullable: true,
        })
        .collect()
}

fn hints(entries: &[(&str, &str, DataType)]) -> ColumnHintsByResource {
    let mut out: ColumnHintsByResource = HashMap::new();
    for (resource, column, dt) in entries {
        out.entry(resource.to_string()).or_default().insert(
            column.to_string(),
            ColumnHints {
                data_type: Some(dt.clone()),
                ..Default::default()
            },
        );
    }
    out
}

/// Owns everything [`PipelineInputs`] borrows.
#[derive(Default)]
struct Fixture {
    live: Option<AirwayPipelineSpec>,
    branch: Option<AirwayPipelineSpec>,
    live_res: Vec<ResourceInfo>,
    branch_res: Vec<ResourceInfo>,
    live_map: HashMap<String, String>,
    branch_map: HashMap<String, String>,
    live_hints: ColumnHintsByResource,
    branch_hints: ColumnHintsByResource,
    stored: Option<Schema>,
    live_columns: Vec<LiveColumn>,
}

impl Fixture {
    /// The same spec and resources on both sides — a no-op change to edit.
    fn same(yaml: &str, resources: Vec<ResourceInfo>) -> Self {
        Self {
            live: Some(spec(yaml)),
            branch: Some(spec(yaml)),
            live_res: resources.clone(),
            branch_res: resources,
            ..Default::default()
        }
    }

    fn check(&self) -> PipelineCheck {
        classify(&PipelineInputs {
            live_spec: self.live.as_ref(),
            branch_spec: self.branch.as_ref(),
            live_resources: &self.live_res,
            branch_resources: &self.branch_res,
            live_mappings: &self.live_map,
            branch_mappings: &self.branch_map,
            live_hints: &self.live_hints,
            branch_hints: &self.branch_hints,
            stored_schema: self.stored.as_ref(),
            live_columns: &self.live_columns,
        })
    }
}

const QB_EASTBAY: &str = r#"
name: quickbooks_financials_eastbay
source:
  kind: quickbooks
  config:
    client_id: client-123
    realm_id: "9341456441393444"
    client_secret_var: QB_EASTBAY_CLIENT_SECRET
    refresh_token_var: QB_EASTBAY_REFRESH_TOKEN
destination:
  database: airhouse
  dataset_name: quickbooks_financials_eastbay
"#;

const NCES_SCHOOLS: &str = r#"
name: nces_schools
source:
  kind: rest_api
  config:
    base_url: https://educationdata.urban.org/api/v1
    endpoints:
      - name: schools
        path: /schools/ccd/directory/2022/
        data_path: results
        write_disposition: replace
destination:
  database: airhouse
  dataset_name: nces
"#;

const CLICKHOUSE_INGEST: &str = r#"
name: clickhouse_ingest
source:
  kind: clickhouse
  config:
    url: https://clickhouse.example.com:8443
    user: reader
    password_var: CLICKHOUSE_PASSWORD
destination:
  database: airhouse
  dataset_name: clickhouse
  schema_separator: "___"
"#;

/// Build a connector the way the analyze task will: substitute placeholders,
/// build offline, read what it advertises.
fn offline_resources(spec: &AirwayPipelineSpec) -> Vec<ResourceInfo> {
    let mut source = spec.source.clone();
    substitute_secret_vars(&mut source.config);
    crate::build_source_connector(&source, None, Environment::Production)
        .expect("connector builds offline")
        .resources()
}

#[test]
fn a_moved_dataset_needs_reset() {
    let live = spec(QB_EASTBAY);
    let branch = spec(&QB_EASTBAY.replace(
        "dataset_name: quickbooks_financials_eastbay",
        "dataset_name: qb_eastbay",
    ));
    let f = Fixture {
        live_res: offline_resources(&live),
        branch_res: offline_resources(&branch),
        live: Some(live),
        branch: Some(branch),
        ..Default::default()
    };
    assert!(
        !f.live_res.is_empty(),
        "quickbooks advertises resources offline"
    );

    let check = f.check();
    assert_eq!(check.verdict, Verdict::NeedsReset);
    assert_eq!(
        check.findings,
        vec![Finding::DestinationMoved {
            from: "airhouse.quickbooks_financials_eastbay".into(),
            to: "airhouse.qb_eastbay".into(),
        }]
    );
    assert_eq!(
        check.findings[0].prod_action(),
        Some("Reset schema on the old dataset; the new one starts empty")
    );
}

#[test]
fn a_rest_api_resource_added_is_additive() {
    let live = spec(NCES_SCHOOLS);
    let branch = spec(&NCES_SCHOOLS.replace(
        "        write_disposition: replace\n",
        "        write_disposition: replace\n      - name: districts\n        path: /school-districts/ccd/directory/2022/\n        data_path: results\n",
    ));
    let f = Fixture {
        live_res: offline_resources(&live),
        branch_res: offline_resources(&branch),
        stored: Some(schema(vec![table(
            "schools",
            WriteDisposition::Replace,
            &[],
        )])),
        live_columns: live_cols(&["schools"]),
        live: Some(live),
        branch: Some(branch),
        ..Default::default()
    };

    let check = f.check();
    assert_eq!(check.verdict, Verdict::Additive, "{:?}", check.findings);
    assert!(check.findings.contains(&Finding::ResourceAdded {
        resource: "districts".into()
    }));
    assert!(check.findings.iter().all(|x| x.prod_action().is_none()));
}

#[test]
fn replace_to_merge_needs_reset_then_backfill() {
    let mut f = Fixture::same(
        NCES_SCHOOLS,
        vec![res(
            "schools",
            WriteDisposition::Replace,
            Some(&["ncessch"]),
        )],
    );
    f.branch_res = vec![res("schools", WriteDisposition::Merge, Some(&["ncessch"]))];
    f.stored = Some(schema(vec![table(
        "schools",
        WriteDisposition::Replace,
        &[],
    )]));
    f.live_columns = live_cols(&["schools"]);

    let check = f.check();
    assert_eq!(check.verdict, Verdict::NeedsReset);
    assert_eq!(
        check.findings,
        vec![Finding::WriteDispositionChanged {
            resource: "schools".into(),
            from: "replace".into(),
            to: "merge".into(),
        }]
    );
    assert_eq!(
        check.findings[0].prod_action(),
        Some("Reset schema, then backfill")
    );
}

/// Main already runs with the stored table disagreeing with the resource;
/// a branch that leaves the disposition alone is not the cause of that.
#[test]
fn a_disposition_the_branch_did_not_touch_is_not_its_finding() {
    let mut f = Fixture::same(
        NCES_SCHOOLS,
        vec![res("schools", WriteDisposition::Replace, None)],
    );
    f.stored = Some(schema(vec![table("schools", WriteDisposition::Merge, &[])]));
    f.live_columns = live_cols(&["schools"]);
    let check = f.check();
    assert_eq!(check.verdict, Verdict::Additive);
    assert!(check.findings.is_empty(), "{:?}", check.findings);
}

#[test]
fn removing_the_schema_separator_needs_reset() {
    let mut f = Fixture::same(
        CLICKHOUSE_INGEST,
        vec![res("toast___orders", WriteDisposition::Append, None)],
    );
    f.branch = Some(spec(
        &CLICKHOUSE_INGEST.replace("  schema_separator: \"___\"\n", ""),
    ));
    // Flattened names land in their own schema, not `clickhouse`, so their
    // absence from the dataset's columns is not drift.
    f.stored = Some(schema(vec![table(
        "toast___orders",
        WriteDisposition::Append,
        &[],
    )]));

    let check = f.check();
    assert_eq!(check.verdict, Verdict::NeedsReset);
    assert_eq!(
        check.findings,
        vec![Finding::SchemaSeparatorChanged {
            from: Some("___".into()),
            to: None,
        }]
    );
}

#[test]
fn a_changed_type_hint_needs_reset() {
    let mut f = Fixture::same(
        NCES_SCHOOLS,
        vec![res("schools", WriteDisposition::Replace, None)],
    );
    f.live_hints = hints(&[
        ("schools", "enrollment", DataType::Double),
        ("schools", "zip", DataType::Text),
    ]);
    f.branch_hints = hints(&[
        ("schools", "enrollment", DataType::BigInt),
        ("schools", "zip", DataType::Text),
        // New column: an addition airway evolves in, not a retype.
        ("schools", "charter", DataType::Bool),
    ]);
    f.stored = Some(schema(vec![table(
        "schools",
        WriteDisposition::Replace,
        &[
            ("enrollment", DataType::Double, false),
            ("zip", DataType::Text, false),
        ],
    )]));
    f.live_columns = live_cols(&["schools"]);

    let check = f.check();
    assert_eq!(check.verdict, Verdict::NeedsReset);
    assert_eq!(
        check.findings,
        vec![Finding::ColumnTypeChanged {
            table: "schools".into(),
            column: "enrollment".into(),
            live: "double".into(),
            candidate: "bigint".into(),
        }]
    );
}

/// With no stored column yet, the live hint is production's type.
#[test]
fn a_changed_type_hint_without_a_stored_schema_compares_hints() {
    let mut f = Fixture::same(
        NCES_SCHOOLS,
        vec![res("schools", WriteDisposition::Replace, None)],
    );
    f.live_hints = hints(&[("schools", "enrollment", DataType::Double)]);
    f.branch_hints = hints(&[(
        "schools",
        "enrollment",
        DataType::Decimal {
            precision: 18,
            scale: 2,
        },
    )]);
    let check = f.check();
    assert_eq!(
        check.findings,
        vec![Finding::ColumnTypeChanged {
            table: "schools".into(),
            column: "enrollment".into(),
            live: "double".into(),
            candidate: "decimal(18,2)".into(),
        }]
    );
}

#[test]
fn a_stored_table_airhouse_lacks_is_live_drift() {
    let mut f = Fixture::same(
        QB_EASTBAY,
        vec![
            res("accounts", WriteDisposition::Merge, Some(&["Id"])),
            res("journal_entries", WriteDisposition::Merge, Some(&["Id"])),
        ],
    );
    f.branch.as_mut().unwrap().description = Some("East Bay books".into());
    f.stored = Some(schema(vec![
        table(
            "accounts",
            WriteDisposition::Merge,
            &[("Id", DataType::Text, true)],
        ),
        table(
            "journal_entries",
            WriteDisposition::Merge,
            &[("Id", DataType::Text, true)],
        ),
    ]));
    f.live_columns = live_cols(&["accounts"]);

    let check = f.check();
    assert_eq!(check.verdict, Verdict::Warning);
    assert_eq!(
        check.findings,
        vec![
            Finding::ConfigOnly {
                field: "description"
            },
            Finding::LiveDrift {
                table: "journal_entries".into()
            },
        ]
    );
}

#[test]
fn removed_resources_and_pipelines_are_warnings() {
    let mut f = Fixture::same(
        NCES_SCHOOLS,
        vec![
            res("schools", WriteDisposition::Replace, None),
            res("districts", WriteDisposition::Replace, None),
        ],
    );
    f.branch_res.retain(|r| r.name == "schools");
    let check = f.check();
    assert_eq!(check.verdict, Verdict::Warning);
    assert_eq!(
        check.findings,
        vec![Finding::ResourceRemoved {
            resource: "districts".into()
        }]
    );

    let gone = Fixture {
        live: Some(spec(NCES_SCHOOLS)),
        ..Default::default()
    };
    let check = gone.check();
    assert_eq!(check.verdict, Verdict::Warning);
    assert_eq!(
        check.findings,
        vec![Finding::PipelineRemoved {
            name: "nces_schools".into()
        }]
    );
    assert_eq!(
        check.findings[0].prod_action(),
        Some("The live table is left behind, not dropped")
    );
}

/// A resource the connector still advertises but the `resources:` list no
/// longer selects is config, not a removal.
#[test]
fn a_narrower_resources_list_is_config_only() {
    let both = vec![
        res("schools", WriteDisposition::Replace, None),
        res("districts", WriteDisposition::Replace, None),
    ];
    let mut f = Fixture::same(NCES_SCHOOLS, both);
    f.branch.as_mut().unwrap().resources = vec!["schools".into()];
    let check = f.check();
    assert_eq!(check.verdict, Verdict::Additive);
    assert_eq!(
        check.findings,
        vec![Finding::ConfigOnly { field: "resources" }]
    );
}

#[test]
fn renames_need_reset() {
    let mut f = Fixture::same(
        QB_EASTBAY,
        vec![res("accounts", WriteDisposition::Merge, Some(&["Id"]))],
    );
    f.branch.as_mut().unwrap().name = "quickbooks_eastbay".into();
    f.live_map = HashMap::from([("orders__checks".to_string(), "order_checks".to_string())]);
    f.branch_map = HashMap::from([("orders__checks".to_string(), "checks".to_string())]);

    let check = f.check();
    assert_eq!(check.verdict, Verdict::NeedsReset);
    assert_eq!(
        check.findings,
        vec![
            Finding::PipelineRenamed {
                from: "quickbooks_financials_eastbay".into(),
                to: "quickbooks_eastbay".into(),
            },
            Finding::TableRenamed {
                resource: "orders__checks".into(),
                from: "order_checks".into(),
                to: "checks".into(),
            },
        ]
    );
}

#[test]
fn a_key_change_needs_reset_only_where_rows_collapse_on_it() {
    let mut merge = Fixture::same(
        QB_EASTBAY,
        vec![res("accounts", WriteDisposition::Merge, Some(&["Id"]))],
    );
    merge.branch_res = vec![res(
        "accounts",
        WriteDisposition::Merge,
        Some(&["Id", "realm_id"]),
    )];
    assert_eq!(
        merge.check().findings,
        vec![Finding::PrimaryKeyChanged {
            resource: "accounts".into(),
            from: Some(vec!["Id".into()]),
            to: Some(vec!["Id".into(), "realm_id".into()]),
        }]
    );

    let mut append = Fixture::same(
        QB_EASTBAY,
        vec![res("accounts", WriteDisposition::Append, Some(&["Id"]))],
    );
    append.branch_res = vec![res("accounts", WriteDisposition::Append, None)];
    assert!(append.check().findings.is_empty());

    // Reordering a composite key is not a change.
    let mut reordered = Fixture::same(
        QB_EASTBAY,
        vec![res("a", WriteDisposition::Merge, Some(&["x", "y"]))],
    );
    reordered.branch_res = vec![res("a", WriteDisposition::Merge, Some(&["y", "x"]))];
    assert!(reordered.check().findings.is_empty());
}

#[test]
fn source_kind_and_new_pipelines() {
    let mut f = Fixture::same(CLICKHOUSE_INGEST, vec![]);
    f.branch.as_mut().unwrap().source.kind = "sql_database".into();
    assert_eq!(
        f.check().findings,
        vec![Finding::SourceKindChanged {
            from: "clickhouse".into(),
            to: "sql_database".into(),
        }]
    );

    let added = Fixture {
        branch: Some(spec(NCES_SCHOOLS)),
        branch_res: vec![res("schools", WriteDisposition::Replace, None)],
        ..Default::default()
    };
    let check = added.check();
    assert_eq!(check.verdict, Verdict::Additive);
    assert_eq!(
        check.findings,
        vec![Finding::ResourceAdded {
            resource: "schools".into()
        }]
    );
}

#[test]
fn verdict_is_the_worst_finding_and_unevaluated_is_never_clean() {
    let check = PipelineCheck::from_findings(vec![
        Finding::ResourceAdded {
            resource: "a".into(),
        },
        Finding::LiveDrift { table: "b".into() },
        Finding::TableRenamed {
            resource: "c".into(),
            from: "c".into(),
            to: "d".into(),
        },
    ]);
    assert_eq!(check.verdict, Verdict::NeedsReset);
    assert_eq!(
        PipelineCheck::from_findings(vec![]).verdict,
        Verdict::Additive
    );
    assert_eq!(
        PipelineCheck::unevaluated("no connector").verdict,
        Verdict::Warning
    );
    assert_eq!(
        serde_json::to_value(Verdict::NeedsReset).unwrap(),
        serde_json::json!("needs_reset")
    );
}

#[test]
fn compare_schemas_reports_what_diff_schemas_does_not() {
    let live = schema(vec![
        table(
            "orders",
            WriteDisposition::Merge,
            &[
                ("guid", DataType::Text, true),
                ("total", DataType::Double, false),
                ("voided", DataType::Bool, false),
            ],
        ),
        table(
            "payments",
            WriteDisposition::Append,
            &[("guid", DataType::Text, false)],
        ),
    ]);
    let candidate = schema(vec![
        table(
            "orders",
            WriteDisposition::Replacing,
            &[
                ("guid", DataType::Text, false),
                ("order_guid", DataType::Text, true),
                (
                    "total",
                    DataType::Decimal {
                        precision: 18,
                        scale: 2,
                    },
                    false,
                ),
                ("tip", DataType::Double, false),
            ],
        ),
        table(
            "refunds",
            WriteDisposition::Append,
            &[("guid", DataType::Text, false)],
        ),
    ]);

    let findings = compare_schemas(&live, &candidate);
    let expect = [
        Finding::TableAdded {
            table: "refunds".into(),
        },
        Finding::ColumnAdded {
            table: "orders".into(),
            column: "order_guid".into(),
        },
        Finding::ColumnAdded {
            table: "orders".into(),
            column: "tip".into(),
        },
        Finding::ColumnRemoved {
            table: "orders".into(),
            column: "voided".into(),
        },
        Finding::ColumnTypeChanged {
            table: "orders".into(),
            column: "total".into(),
            live: "double".into(),
            candidate: "decimal(18,2)".into(),
        },
        Finding::WriteDispositionChanged {
            resource: "orders".into(),
            from: "merge".into(),
            to: "replacing".into(),
        },
        Finding::PrimaryKeyChanged {
            resource: "orders".into(),
            from: Some(vec!["guid".into()]),
            to: Some(vec!["order_guid".into()]),
        },
    ];
    for e in &expect {
        assert!(findings.contains(e), "missing {e:?} in {findings:?}");
    }
    assert_eq!(findings.len(), expect.len(), "{findings:?}");
    // `payments` is absent from the candidate: a bounded sample, not a drop.
    assert_eq!(
        PipelineCheck::from_findings(findings).verdict,
        Verdict::NeedsReset
    );
    assert!(compare_schemas(&live, &live).is_empty());
}
