use std::{collections::HashMap, hash::Hash, path::PathBuf};

use indexmap::IndexMap;
use itertools::Itertools;
use minijinja::Value;
use rmcp::model::ContentBlock;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::exec_types::{Output, Table};
use oxy_shared::errors::OxyError;

use super::reference::ReferenceKind;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metadata {
    #[serde(alias = "value")]
    pub output: Box<OutputContainer>,
    pub references: Vec<ReferenceKind>,
    pub metadata: HashMap<String, String>,
}

impl std::fmt::Display for Metadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.output)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OutputContainer {
    List(Vec<OutputContainer>),
    Map(IndexMap<String, OutputContainer>),
    Single(Output),
    Variable(JsonValue),
    Consistency {
        #[serde(flatten)]
        value: Metadata,
        score: f32,
    },
    Metadata {
        #[serde(flatten)]
        value: Metadata,
    },
}

impl Default for OutputContainer {
    fn default() -> Self {
        OutputContainer::Map(IndexMap::new())
    }
}

impl OutputContainer {
    pub fn try_get_metadata(&self) -> Result<Metadata, OxyError> {
        match self {
            OutputContainer::Consistency { value, .. } => Ok(value.clone()),
            OutputContainer::Metadata { value, .. } => Ok(value.clone()),
            _ => Err(OxyError::RuntimeError(format!(
                "Cannot get metadata from {self:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableData {
    pub file_path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Data {
    Bool(bool),
    Text(String),
    Table(TableData),
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DataContainer {
    List(Vec<DataContainer>),
    Map(HashMap<String, DataContainer>),
    Single(Data),
    None,
}

impl DataContainer {
    pub fn load_from_file(file_path: &PathBuf) -> Result<Self, OxyError> {
        let file = std::fs::File::open(file_path).map_err(|e| {
            OxyError::RuntimeError(format!("Error opening file {}: {}", file_path.display(), e))
        })?;
        let reader = std::io::BufReader::new(file);
        let output_container: DataContainer = serde_yaml::from_reader(reader)
            .map_err(|e| OxyError::RuntimeError(format!("Error deserializing yaml: {e}")))?;
        Ok(output_container)
    }
}

impl OutputContainer {
    pub fn to_markdown(&self) -> String {
        match self {
            OutputContainer::List(list) => {
                let mut rs = String::new();
                for item in list {
                    rs.push_str(&format!("{}\n", item.to_markdown()));
                }
                rs
            }
            OutputContainer::Map(map) => {
                let mut rs = String::new();
                for (key, value) in map {
                    if let OutputContainer::Variable(_) = value {
                        continue;
                    }
                    rs.push_str(&format!(
                        "<details open>\n<summary>{}</summary>\n\n{}\n\n</details>\n",
                        key,
                        value.to_markdown()
                    ));
                }
                rs
            }
            OutputContainer::Single(output) => output.to_markdown(),
            OutputContainer::Metadata { value, .. } => value.output.to_markdown(),
            OutputContainer::Consistency { value, .. } => value.output.to_markdown(),
            OutputContainer::Variable(output) => output.to_string(),
        }
    }
    pub fn to_json(&self) -> Result<serde_json::Value, OxyError> {
        match self {
            OutputContainer::List(list) => {
                let mut rs = vec![];
                for item in list {
                    rs.push(item.to_json()?);
                }
                Ok(serde_json::Value::Object(serde_json::Map::from_iter([
                    ("type".to_string(), serde_json::Value::from("list")),
                    (
                        "value".to_string(),
                        serde_json::Value::Array(rs.into_iter().collect()),
                    ),
                ])))
            }
            OutputContainer::Map(map) => {
                let mut rs = serde_json::Map::new();
                for (k, v) in map {
                    rs.insert(k.clone(), v.to_json()?);
                }
                Ok(serde_json::Value::Object(serde_json::Map::from_iter([
                    ("type".to_string(), serde_json::Value::from("map")),
                    ("value".to_string(), serde_json::Value::Object(rs)),
                ])))
            }
            OutputContainer::Single(output) => {
                let mut value = serde_json::Map::new();
                match output {
                    Output::Bool(b) => {
                        value.insert("type".to_string(), serde_json::Value::from("bool"));
                        value.insert("value".to_string(), serde_json::Value::from(*b));
                    }
                    Output::Text(t) => {
                        value.insert("type".to_string(), serde_json::Value::from("text"));
                        value.insert("value".to_string(), serde_json::Value::from(t.clone()));
                    }
                    Output::Prompt(p) => {
                        value.insert("type".to_string(), serde_json::Value::from("text"));
                        value.insert("value".to_string(), serde_json::Value::from(p.0.clone()));
                    }
                    Output::SQL(s) => {
                        value.insert("type".to_string(), serde_json::Value::from("sql"));
                        value.insert("value".to_string(), serde_json::Value::from(s.0.clone()));
                    }
                    Output::Table(t) => {
                        value = t.to_json()?;
                    }
                    _ => {
                        value.insert("type".to_string(), serde_json::Value::from("unknown"));
                        value.insert(
                            "value".to_string(),
                            serde_json::Value::from(format!("{output}")),
                        );
                    }
                }
                Ok(serde_json::Value::Object(value))
            }
            OutputContainer::Metadata { value, .. } => value.output.to_json(),
            OutputContainer::Consistency { value, score } => {
                let mut map = serde_json::Map::new();
                map.insert("type".to_string(), serde_json::Value::from("consistency"));
                map.insert("value".to_string(), value.output.to_json()?);
                map.insert("score".to_string(), serde_json::Value::from(*score));
                Ok(serde_json::Value::Object(map))
            }
            OutputContainer::Variable(output) => {
                Ok(serde_json::Value::Object(serde_json::Map::from_iter([
                    ("type".to_string(), serde_json::Value::from("variable")),
                    ("value".to_string(), output.clone()),
                ])))
            }
        }
    }

    pub fn to_data(
        self,
        relative_path: &PathBuf,
        base_path: &PathBuf,
    ) -> Result<DataContainer, OxyError> {
        match self {
            OutputContainer::List(list) => {
                let mut rs = vec![];
                for item in list {
                    rs.push(item.to_data(relative_path, base_path)?);
                }
                Ok(DataContainer::List(rs))
            }
            OutputContainer::Map(map) => {
                let mut rs = HashMap::new();
                for (k, v) in map {
                    rs.insert(k, v.to_data(relative_path, base_path)?);
                }

                Ok(DataContainer::Map(rs))
            }
            OutputContainer::Single(output) => Ok(DataContainer::Single(
                output.to_data(relative_path, base_path)?,
            )),
            OutputContainer::Consistency { value, .. } => {
                value.output.to_data(relative_path, base_path)
            }
            OutputContainer::Metadata { value, .. } => {
                value.output.to_data(relative_path, base_path)
            }
            OutputContainer::Variable(output) => {
                Ok(DataContainer::Single(Data::Text(output.to_string())))
            }
        }
    }
    pub fn merge(self, other: OutputContainer) -> OutputContainer {
        match (self, other) {
            (OutputContainer::List(mut list1), OutputContainer::List(list2)) => {
                list1.extend(list2);
                OutputContainer::List(list1)
            }
            (OutputContainer::Map(mut map1), OutputContainer::Map(map2)) => {
                map1.extend(map2);
                OutputContainer::Map(map1)
            }
            _ => panic!("Cannot merge different output type"),
        }
    }

    pub fn project_ref(&self, task_ref: &str) -> Result<Vec<&OutputContainer>, OxyError> {
        let mut containers = vec![self];
        for part in task_ref.split('.') {
            containers = containers
                .iter()
                .map(|container| container.find_ref(part))
                .try_collect::<Vec<&OutputContainer>, Vec<_>, OxyError>()
                .map(|item| item.into_iter().flatten().collect())?;
        }
        Ok(containers)
    }

    pub fn find_ref(&self, task_ref: &str) -> Result<Vec<&OutputContainer>, OxyError> {
        match self {
            OutputContainer::List(list) => list
                .iter()
                .map(|item| item.find_ref(task_ref))
                .try_collect::<Vec<&OutputContainer>, Vec<_>, OxyError>()
                .map(|item| item.into_iter().flatten().collect()),
            OutputContainer::Map(map) => {
                map.get(task_ref)
                    .map(|item| vec![item])
                    .ok_or(OxyError::RuntimeError(format!(
                        "Task ref `{task_ref}` not found"
                    )))
            }
            _ => Err(OxyError::RuntimeError(format!(
                "Cannot find `{task_ref}` in {self:?}"
            ))),
        }
    }
}

impl std::fmt::Display for OutputContainer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputContainer::List(list) => {
                for item in list {
                    writeln!(f, "{item}")?;
                }
                Ok(())
            }
            OutputContainer::Map(map) => {
                for (key, value) in map {
                    writeln!(f, "{key}: {value}")?;
                }
                Ok(())
            }
            OutputContainer::Single(output) => writeln!(f, "{output}"),
            OutputContainer::Metadata { value, .. } => writeln!(f, "{value}"),
            OutputContainer::Consistency { value, .. } => {
                writeln!(f, "{value}")
            }
            OutputContainer::Variable(output) => writeln!(f, "{output}"),
        }
    }
}

impl From<&OutputContainer> for Value {
    fn from(value: &OutputContainer) -> Self {
        match value {
            OutputContainer::List(list) => Value::from_iter(list.iter().map(Value::from)),
            OutputContainer::Map(map) => Value::from_iter(
                map.iter()
                    .map(|(k, v)| (k, Into::<Value>::into(v)))
                    .collect::<Vec<_>>(),
            ),
            OutputContainer::Single(output) => Value::from_object(output.clone()),
            OutputContainer::Metadata { value, .. } => (value.output.as_ref()).into(),
            OutputContainer::Consistency { value, score, .. } => {
                let mut map = HashMap::new();
                map.insert("value".to_string(), (value.output.as_ref()).into());
                map.insert("score".to_string(), Value::from(*score));
                Value::from_iter(map)
            }
            OutputContainer::Variable(output) => Value::from_serialize(output),
        }
    }
}

impl TryFrom<OutputContainer> for ContentBlock {
    type Error = OxyError;

    fn try_from(value: OutputContainer) -> Result<Self, Self::Error> {
        let value = serde_json::to_string(&value).map_err(|e| {
            OxyError::SerializerError(format!("Error serializing OutputContainer to JSON: {e}"))
        })?;
        Ok(ContentBlock::text(value))
    }
}

impl From<Output> for OutputContainer {
    fn from(val: Output) -> Self {
        OutputContainer::Single(val)
    }
}

impl From<Table> for OutputContainer {
    fn from(val: Table) -> Self {
        OutputContainer::Single(Output::Table(val))
    }
}

impl From<HashMap<String, OutputContainer>> for OutputContainer {
    fn from(val: HashMap<String, OutputContainer>) -> Self {
        OutputContainer::Map(val.into_iter().collect())
    }
}

impl Hash for OutputContainer {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            OutputContainer::List(list) => {
                list.hash(state);
            }
            OutputContainer::Map(map) => {
                map.iter()
                    .sorted_by_key(|item| item.0)
                    .for_each(|(key, value)| {
                        key.hash(state);
                        value.hash(state);
                    });
            }
            OutputContainer::Single(output) => {
                output.hash(state);
            }
            OutputContainer::Metadata { value, .. } => {
                value.output.hash(state);
            }
            OutputContainer::Consistency { value, score } => {
                value.output.hash(state);
                score.to_bits().hash(state);
            }
            OutputContainer::Variable(output) => {
                output.hash(state);
            }
        }
    }
}
