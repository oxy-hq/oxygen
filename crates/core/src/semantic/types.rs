use std::{collections::HashMap, str::FromStr};

use serde::{Serialize, ser::SerializeStruct};

use oxy_shared::errors::OxyError;

pub struct SemanticKey {
    pub database: String,
    pub dataset: String,
}

impl SemanticKey {
    pub fn new(database: String, dataset: String) -> Self {
        SemanticKey { database, dataset }
    }
}

#[derive(Debug, Clone)]
pub struct SemanticTableRef {
    pub database: String,
    pub dataset: String,
    pub table: String,
}

impl SemanticTableRef {
    pub fn table_ref(&self) -> String {
        format!("{}.{}.{}.", self.database, self.dataset, self.table)
    }

    pub fn to_target(&self, dimension: &str) -> String {
        format!(
            "{}.{}.{}.{}",
            self.database, self.dataset, self.table, dimension
        )
    }
}

impl FromStr for SemanticTableRef {
    type Err = OxyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() < 3 {
            return Err(OxyError::SerializerError(format!(
                "Invalid semantic table reference format: '{s}'. Expected format: 'database.dataset.table'"
            )));
        }
        Ok(SemanticTableRef {
            database: parts[0].to_string(),
            dataset: parts[1].to_string(),
            table: parts[2].to_string(),
        })
    }
}

pub struct Dataset {
    pub name: String,
    pub tables: Vec<Table>,
}

pub struct Table {
    pub name: String,
    pub columns: Vec<Column>,
}

pub struct Column {
    pub name: String,
    pub data_type: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DatasetInfo {
    pub dataset: String,
    pub ddl: Option<String>,
    pub semantic_info: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct DatabaseInfo {
    pub name: String,
    pub dialect: String,
    pub datasets: HashMap<String, DatasetInfo>,
}

impl DatabaseInfo {
    pub fn tables(&self) -> Vec<String> {
        self.datasets
            .values()
            .filter_map(|dataset| dataset.ddl.clone())
            .collect()
    }
}

impl Serialize for DatabaseInfo {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("DatabaseInfo", 4)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("dialect", &self.dialect)?;
        state.serialize_field("datasets", &self.datasets)?;
        state.serialize_field("tables", &self.tables())?;
        state.end()
    }
}
