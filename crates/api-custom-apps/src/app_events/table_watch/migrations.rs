//! The ship gate for a watched table's columns (app events design §7.3,
//! "The watched table's columns become a contract").
//!
//! A function that watches `app_x.lines` reads the rows each event carries by
//! column name, so a migration that drops, renames or retypes one of them, or
//! drops or renames the table, breaks it. At runtime the tracker notices (the
//! cursor goes `broken` and pages); [`check`] is the same rule one step
//! earlier, on the files a build would apply:
//!
//! * the files of the build's `airhouseMigrations` that production's ledger
//!   has not applied yet ([`pending_in_production`]) are parsed
//!   ([`breaks`]);
//! * each break that names a table some production cursor watches in the
//!   app's own schema is a **refusal** of a ship (the listeners named), and
//!   `--break-listeners` overrides it, audited like an event break; a
//!   rollback never refuses and only warns.
//!
//! Where the table comes from somewhere else (a procedure, a pipeline) there
//! is no migration to read, and only the runtime check applies.

use entity::{app_builds, apps};
use oxy_custom_apps::events::table::TableRef;
use sea_orm::{DatabaseConnection, DbErr};
use serde_json::Value;
use sqlparser::ast::{
    AlterColumnOperation, AlterTableOperation, ObjectName, ObjectNamePart, ObjectType, Statement,
};
use sqlparser::dialect::DuckDbDialect;

use super::cursors::of_environment;
use crate::app_events::gate::{GateReport, GateScope};
use crate::app_events::subscriptions::listeners;
use crate::custom_apps_migrations::{declared_in_build, pending_in_production};

const PRODUCTION: &str = "production";

/// What one statement does to what a listener reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Break {
    DropTable {
        schema: Option<String>,
        table: String,
    },
    RenameTable {
        schema: Option<String>,
        table: String,
    },
    DropColumn {
        schema: Option<String>,
        table: String,
        column: String,
    },
    RenameColumn {
        schema: Option<String>,
        table: String,
        column: String,
    },
    RetypeColumn {
        schema: Option<String>,
        table: String,
        column: String,
    },
}

impl Break {
    fn target(&self) -> (&Option<String>, &str, Option<&str>) {
        match self {
            Break::DropTable { schema, table } | Break::RenameTable { schema, table } => {
                (schema, table, None)
            }
            Break::DropColumn {
                schema,
                table,
                column,
            }
            | Break::RenameColumn {
                schema,
                table,
                column,
            }
            | Break::RetypeColumn {
                schema,
                table,
                column,
            } => (schema, table, Some(column)),
        }
    }

    fn says(&self) -> String {
        match self {
            Break::DropTable { .. } => "drops the table".to_string(),
            Break::RenameTable { .. } => "renames the table".to_string(),
            Break::DropColumn { column, .. } => format!("drops column `{column}`"),
            Break::RenameColumn { column, .. } => format!("renames column `{column}`"),
            Break::RetypeColumn { column, .. } => format!("changes the type of column `{column}`"),
        }
    }
}

fn parts(name: &ObjectName) -> (Option<String>, String) {
    let idents: Vec<String> = name
        .0
        .iter()
        .filter_map(|p| match p {
            ObjectNamePart::Identifier(i) => Some(i.value.clone()),
            _ => None,
        })
        .collect();
    match idents.as_slice() {
        [.., schema, table] => (Some(schema.clone()), table.clone()),
        [table] => (None, table.clone()),
        [] => (None, String::new()),
    }
}

/// The breaks `sql` makes. SQL that does not parse breaks nothing here: the
/// migration's own checks (`airhouse::sql_rules`) refuse it where it matters.
pub fn breaks(sql: &str) -> Vec<Break> {
    let Ok(statements) = sqlparser::parser::Parser::parse_sql(&DuckDbDialect {}, sql) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for statement in &statements {
        match statement {
            Statement::Drop {
                object_type: ObjectType::Table,
                names,
                ..
            } => {
                for name in names {
                    let (schema, table) = parts(name);
                    found.push(Break::DropTable { schema, table });
                }
            }
            Statement::AlterTable(alter) => {
                let (schema, table) = parts(&alter.name);
                for op in &alter.operations {
                    match op {
                        AlterTableOperation::DropColumn { column_names, .. } => {
                            for column in column_names {
                                found.push(Break::DropColumn {
                                    schema: schema.clone(),
                                    table: table.clone(),
                                    column: column.value.clone(),
                                });
                            }
                        }
                        AlterTableOperation::RenameColumn {
                            old_column_name, ..
                        } => {
                            found.push(Break::RenameColumn {
                                schema: schema.clone(),
                                table: table.clone(),
                                column: old_column_name.value.clone(),
                            });
                        }
                        AlterTableOperation::AlterColumn {
                            column_name,
                            op: AlterColumnOperation::SetDataType { .. },
                        } => found.push(Break::RetypeColumn {
                            schema: schema.clone(),
                            table: table.clone(),
                            column: column_name.value.clone(),
                        }),
                        AlterTableOperation::RenameTable { .. } => {
                            found.push(Break::RenameTable {
                                schema: schema.clone(),
                                table: table.clone(),
                            });
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    found
}

/// Whether `found` hits `watched` (a table in `app_schema`) and, for a column,
/// one its contract holds (`columns`, when the tracker has read it).
fn hits(found: &Break, app_schema: &str, watched: &TableRef, columns: Option<&Value>) -> bool {
    let (schema, table, column) = found.target();
    let schema = schema.as_deref().unwrap_or(app_schema);
    if !schema.eq_ignore_ascii_case(&watched.schema) || !table.eq_ignore_ascii_case(&watched.table)
    {
        return false;
    }
    match (column, columns.and_then(Value::as_object)) {
        (Some(column), Some(contract)) => contract.keys().any(|c| c.eq_ignore_ascii_case(column)),
        _ => true,
    }
}

/// Add to `report` what `target`'s unapplied Airhouse migrations would break
/// for a function watching a table in the app's own schema, in production.
pub async fn check(
    db: &DatabaseConnection,
    app: &apps::Model,
    target: &app_builds::Model,
    scope: GateScope,
    report: &mut GateReport,
) -> Result<(), DbErr> {
    let cursors = of_environment(db, app.org_id, PRODUCTION).await?;
    let Some(app_schema) = app_schema_of(&app.slug) else {
        return Ok(());
    };
    let watched: Vec<_> = cursors
        .iter()
        .filter(|c| c.workspace_id == app.project_id)
        .filter_map(|c| Some((c, super::cursors::table_of(c)?)))
        .filter(|(_, t)| t.schema.eq_ignore_ascii_case(&app_schema))
        .collect();
    if watched.is_empty() {
        return Ok(());
    }
    // A build whose files cannot be read is the publish's own failure to
    // report, not this gate's.
    let Ok(declared) = declared_in_build(target, &app.slug).await else {
        return Ok(());
    };
    let Ok((_, pending)) = pending_in_production(db, app.id, &declared).await else {
        return Ok(());
    };
    for file in declared
        .airhouse
        .iter()
        .filter(|f| pending.contains(&f.filename))
    {
        for found in breaks(&file.sql) {
            for (cursor, table) in &watched {
                if !hits(&found, &app_schema, table, cursor.table_columns.as_ref()) {
                    continue;
                }
                let who = listeners(db, app.org_id, PRODUCTION, &table.event_type()).await?;
                let who: Vec<String> = who
                    .iter()
                    .filter(|l| l.workspace_id == cursor.workspace_id)
                    .map(|l| format!("{}/{}", l.app_slug, l.function_name))
                    .collect();
                let text = format!(
                    "migration `{}` {} of `{}`, which {} watch{} in production.",
                    file.filename,
                    found.says(),
                    table.spec(),
                    who.join(", "),
                    if who.len() == 1 { "es" } else { "" },
                );
                if matches!(scope, GateScope::Ship { .. }) && !who.is_empty() {
                    report.refusals.push(text);
                } else {
                    report.warnings.push(text);
                }
            }
        }
    }
    Ok(())
}

/// The app's Airhouse schema, `app_<writer>`.
fn app_schema_of(slug: &str) -> Option<String> {
    let writer = oxy_oltp::schema::app_writer_name(slug)?;
    oxy_oltp::schema::WriterRef::app(&writer)
        .ok()
        .map(|w| w.schema_name())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_drop_rename_or_retype_of_a_column_is_a_break() {
        let sql = "ALTER TABLE app_x.lines DROP COLUMN amt;\n\
                   ALTER TABLE lines RENAME COLUMN note TO memo;\n\
                   ALTER TABLE lines ALTER COLUMN id SET DATA TYPE BIGINT;\n\
                   ALTER TABLE lines ALTER COLUMN id TYPE VARCHAR;";
        let found = breaks(sql);
        assert_eq!(found.len(), 4, "{found:?}");
        assert!(matches!(&found[0], Break::DropColumn { column, .. } if column == "amt"));
        assert!(matches!(&found[1], Break::RenameColumn { column, .. } if column == "note"));
        assert!(matches!(&found[2], Break::RetypeColumn { .. }));
    }

    #[test]
    fn dropping_or_renaming_the_table_is_a_break() {
        assert!(matches!(
            &breaks("DROP TABLE app_x.lines")[0],
            Break::DropTable { .. }
        ));
        assert!(matches!(
            &breaks("ALTER TABLE lines RENAME TO rows")[0],
            Break::RenameTable { .. }
        ));
    }

    #[test]
    fn adding_a_column_or_a_table_breaks_nothing() {
        assert!(breaks("ALTER TABLE lines ADD COLUMN extra INTEGER").is_empty());
        assert!(breaks("CREATE TABLE other (id INTEGER)").is_empty());
        assert!(breaks("INSERT INTO lines VALUES (1)").is_empty());
        assert!(breaks("not sql").is_empty());
    }

    #[test]
    fn a_break_hits_a_watched_column_not_a_stranger() {
        let watched = TableRef::parse("app_x.lines").unwrap();
        let columns = json!({ "id": "INTEGER", "amt": "INTEGER" });
        let drop = |column: &str| Break::DropColumn {
            schema: None,
            table: "lines".into(),
            column: column.into(),
        };
        assert!(hits(&drop("amt"), "app_x", &watched, Some(&columns)));
        assert!(!hits(
            &drop("never_there"),
            "app_x",
            &watched,
            Some(&columns)
        ));
        assert!(
            hits(&drop("anything"), "app_x", &watched, None),
            "no contract read yet: any column"
        );
        let other = Break::DropColumn {
            schema: Some("app_y".into()),
            table: "lines".into(),
            column: "amt".into(),
        };
        assert!(!hits(&other, "app_x", &watched, Some(&columns)));
        let table = Break::DropTable {
            schema: None,
            table: "lines".into(),
        };
        assert!(hits(&table, "app_x", &watched, Some(&columns)));
    }
}
