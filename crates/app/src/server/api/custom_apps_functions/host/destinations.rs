//! Customer warehouses are read-only to apps: where a `ctx.warehouse` /
//! `ctx.tx` write would land, and whether the function said why it may write
//! there (`internal-docs/data-placement.md`).

use std::collections::BTreeMap;

use oxy::config::model::DatabaseType;

/// Where a `ctx.warehouse` / `ctx.tx` write would land, as the read-only rule
/// for customer warehouses sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::server::api::custom_apps_functions) enum DestinationKind {
    /// The workspace's Airhouse — the facts store Oxy runs.
    Airhouse,
    /// The org's managed OLTP, which `ctx.warehouse` reaches as the read-only analyst.
    ManagedOltp,
    /// A warehouse the customer owns.
    CustomerWarehouse,
}

/// Which surface is writing, so a refusal can speak to what the author did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::server::api::custom_apps_functions) enum WriteSurface {
    /// `ctx.warehouse.insert / upsert / exec`.
    Warehouse,
    /// `ctx.tx(database, fn)` — checked as a write even when the callback only
    /// reads, because `begin` cannot know what the statements will be.
    Transaction,
}

/// Unknown and future database types land on `CustomerWarehouse`: the rule
/// fails closed.
pub(in crate::server::api::custom_apps_functions) fn destination_kind(
    database_type: &DatabaseType,
) -> DestinationKind {
    match database_type {
        DatabaseType::Airhouse(_) | DatabaseType::AirhouseManaged(_) => DestinationKind::Airhouse,
        DatabaseType::PostgresManaged(_) => DestinationKind::ManagedOltp,
        _ => DestinationKind::CustomerWarehouse,
    }
}

/// Customer warehouses are read-only to apps (`internal-docs/data-placement.md`).
/// A function writes one only when its manifest names the database in
/// `customerWarehouseWrites` with a non-blank reason.
pub(in crate::server::api::custom_apps_functions) fn destination_write_policy(
    database: &str,
    kind: DestinationKind,
    exceptions: &BTreeMap<String, String>,
    surface: WriteSurface,
) -> Result<(), String> {
    match kind {
        DestinationKind::Airhouse => Ok(()),
        DestinationKind::ManagedOltp => Err(format!(
            "database '{database}' is the org's OLTP store, which this surface reaches as the \
             read-only analyst — write this app's own records with ctx.oltp"
        )),
        DestinationKind::CustomerWarehouse => match exceptions.get(database) {
            Some(reason) if !reason.trim().is_empty() => Ok(()),
            _ => {
                let mut refusal = format!(
                    "database '{database}' is a customer warehouse, and customer warehouses are \
                     read-only to apps. Facts this app records belong in Airhouse \
                     (ctx.airhouse); records it edits belong in ctx.oltp. If this write has to \
                     stay, say why on the function in oxy-app.json: \
                     \"customerWarehouseWrites\": {{ \"{database}\": \"<reason>\" }}"
                );
                if surface == WriteSurface::Transaction {
                    refusal.push_str(
                        ". A ctx.tx transaction counts as a write even when it only reads — read \
                         with ctx.warehouse.query instead",
                    );
                }
                Err(refusal)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn customer_warehouses_are_read_only_unless_the_function_says_why() {
        let none = BTreeMap::new();
        assert!(
            destination_write_policy(
                "pokehouse",
                DestinationKind::Airhouse,
                &none,
                WriteSurface::Warehouse
            )
            .is_ok()
        );
        let err = destination_write_policy(
            "clickhouse",
            DestinationKind::CustomerWarehouse,
            &none,
            WriteSurface::Warehouse,
        )
        .unwrap_err();
        assert!(err.contains("\"customerWarehouseWrites\""), "{err}");

        let mut declared = BTreeMap::new();
        declared.insert(
            "clickhouse".to_string(),
            "legacy facts until Airhouse".to_string(),
        );
        declared.insert("blank".to_string(), "  ".to_string());
        assert!(
            destination_write_policy(
                "clickhouse",
                DestinationKind::CustomerWarehouse,
                &declared,
                WriteSurface::Warehouse
            )
            .is_ok()
        );
        assert!(
            destination_write_policy(
                "snowflake",
                DestinationKind::CustomerWarehouse,
                &declared,
                WriteSurface::Warehouse
            )
            .is_err(),
            "an exception names one database, not every customer warehouse"
        );
        assert!(
            destination_write_policy(
                "blank",
                DestinationKind::CustomerWarehouse,
                &declared,
                WriteSurface::Warehouse
            )
            .is_err(),
            "a blank reason is no reason"
        );
    }

    #[test]
    fn only_a_transaction_refusal_says_reads_count_as_writes() {
        let none = BTreeMap::new();
        let tx = destination_write_policy(
            "pg",
            DestinationKind::CustomerWarehouse,
            &none,
            WriteSurface::Transaction,
        )
        .unwrap_err();
        assert!(
            tx.contains("counts as a write even when it only reads"),
            "{tx}"
        );
        let write = destination_write_policy(
            "pg",
            DestinationKind::CustomerWarehouse,
            &none,
            WriteSurface::Warehouse,
        )
        .unwrap_err();
        assert!(!write.contains("only reads"), "{write}");
    }

    #[test]
    fn a_write_to_the_managed_oltp_analyst_points_at_ctx_oltp() {
        let err = destination_write_policy(
            "oltp",
            DestinationKind::ManagedOltp,
            &BTreeMap::new(),
            WriteSurface::Warehouse,
        )
        .unwrap_err();
        assert!(err.contains("ctx.oltp"), "{err}");
    }
}
