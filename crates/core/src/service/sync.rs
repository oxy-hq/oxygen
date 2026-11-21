use std::collections::HashMap;

/// Which databases, datasets or tables a sync renders
/// ([`crate::semantic::render::render_databases`]).
#[derive(Debug, Clone, Default)]
pub struct SyncFilter {
    /// Database name to sync (e.g., "clickhouse"). If None, sync all databases.
    pub database: Option<String>,
    /// Dataset/schema names to include. Empty means all.
    pub datasets: Vec<String>,
    /// Specific tables to include, as "schema.table" strings.
    /// When non-empty, only these tables are synced (overrides datasets).
    pub tables: Vec<String>,
}

impl SyncFilter {
    /// Convert to the legacy filter tuple format (database, datasets).
    /// When `tables` is set, groups them by schema and returns table-level filter.
    pub fn into_filter(self) -> (Option<String>, HashMap<String, Vec<String>>) {
        if !self.tables.is_empty() {
            // Group "schema.table" entries by schema
            let mut schema_tables: HashMap<String, Vec<String>> = HashMap::new();
            for qualified in &self.tables {
                if let Some((schema, table)) = qualified.split_once('.') {
                    schema_tables
                        .entry(schema.to_string())
                        .or_default()
                        .push(table.to_string());
                }
            }
            (self.database, schema_tables)
        } else if !self.datasets.is_empty() {
            // Schema-level filter: all tables within listed schemas
            let datasets_map: HashMap<String, Vec<String>> = self
                .datasets
                .into_iter()
                .map(|d| (d, vec!["*".to_string()]))
                .collect();
            (self.database, datasets_map)
        } else {
            (self.database, HashMap::new())
        }
    }
}
