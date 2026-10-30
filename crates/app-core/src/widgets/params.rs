//! A widget's settings: the small JSON Schema its manifest declares (`params`),
//! and the values a layout gives each copy of it.
//!
//! Deliberately a subset — an object of at most `MAX_PARAMS` flat properties,
//! each a string, integer, number or boolean with an optional title,
//! description, default, enum and bounds — because the values are a handful of
//! settings an agent fills in (which store, which vendor), the way a WidgetKit
//! widget's configuration or a Backstage home widget's settings schema are.

use serde_json::{Map, Value};

use crate::notifications::catalog::model::valid_segment;

pub const MAX_PARAMS: usize = 8;
const PROPERTY_KEYS: &[&str] = &[
    "type",
    "title",
    "description",
    "default",
    "enum",
    "minimum",
    "maximum",
    "minLength",
    "maxLength",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamType {
    String,
    Integer,
    Number,
    Boolean,
}

impl ParamType {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "string" => Some(Self::String),
            "integer" => Some(Self::Integer),
            "number" => Some(Self::Number),
            "boolean" => Some(Self::Boolean),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Integer => "integer",
            Self::Number => "number",
            Self::Boolean => "boolean",
        }
    }

    fn admits(self, v: &Value) -> bool {
        match self {
            Self::String => v.is_string(),
            Self::Integer => v.is_i64() || v.is_u64(),
            Self::Number => v.is_number(),
            Self::Boolean => v.is_boolean(),
        }
    }
}

/// One setting.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: ParamType,
    pub description: Option<String>,
    pub default: Option<Value>,
    pub choices: Option<Vec<Value>>,
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
    pub min_length: Option<u64>,
    pub max_length: Option<u64>,
}

/// A widget's settings schema, parsed; `raw` is the manifest's own object,
/// echoed to `GET …/home` so an agent reads the schema as written.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamsSchema {
    pub raw: Value,
    pub props: Vec<Param>,
    pub required: Vec<String>,
}

impl Eq for ParamsSchema {}

impl ParamsSchema {
    /// Settings a copy cannot be drawn without: required, with no default.
    pub fn needs(&self) -> Vec<&str> {
        self.required
            .iter()
            .filter(|name| self.prop(name).is_some_and(|p| p.default.is_none()))
            .map(String::as_str)
            .collect()
    }

    fn prop(&self, name: &str) -> Option<&Param> {
        self.props.iter().find(|p| p.name == name)
    }

    /// `given` checked and completed with defaults: every problem as
    /// `(property, message, fix)`.
    pub fn values(
        &self,
        given: Option<&Map<String, Value>>,
    ) -> Result<Map<String, Value>, Vec<(String, String, String)>> {
        let mut out = Map::new();
        let mut problems = Vec::new();
        let empty = Map::new();
        let given = given.unwrap_or(&empty);
        for (name, value) in given {
            match self.prop(name) {
                None => problems.push((
                    name.clone(),
                    format!("`{name}` is not a setting of this widget"),
                    format!("Remove it; the settings are: {}.", self.names()),
                )),
                Some(p) => match p.check(value) {
                    Ok(()) => {
                        out.insert(name.clone(), value.clone());
                    }
                    Err((message, fix)) => problems.push((name.clone(), message, fix)),
                },
            }
        }
        for p in &self.props {
            if out.contains_key(&p.name) || given.contains_key(&p.name) {
                continue;
            }
            match (&p.default, self.required.contains(&p.name)) {
                (Some(default), _) => {
                    out.insert(p.name.clone(), default.clone());
                }
                (None, true) => problems.push((
                    p.name.clone(),
                    format!("this widget needs \"{}\"", p.name),
                    format!("Set params.{}: {}.", p.name, p.describe()),
                )),
                (None, false) => {}
            }
        }
        match problems.is_empty() {
            true => Ok(out),
            false => Err(problems),
        }
    }

    fn names(&self) -> String {
        self.props
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl Param {
    fn check(&self, v: &Value) -> Result<(), (String, String)> {
        let fix = || format!("Use {}.", self.describe());
        if !self.ty.admits(v) {
            return Err((format!("{v} is not a {}", self.ty.name()), fix()));
        }
        if let Some(choices) = &self.choices
            && !choices.contains(v)
        {
            return Err((format!("{v} is not one of the choices"), fix()));
        }
        if let Some(n) = v.as_f64() {
            let low = self.minimum.is_some_and(|min| n < min);
            let high = self.maximum.is_some_and(|max| n > max);
            if low || high {
                return Err((format!("{v} is out of range"), fix()));
            }
        }
        if let Some(s) = v.as_str() {
            let len = s.chars().count() as u64;
            let short = self.min_length.is_some_and(|min| len < min);
            let long = self.max_length.is_some_and(|max| len > max);
            if short || long {
                return Err((format!("\"{s}\" has the wrong length"), fix()));
            }
        }
        Ok(())
    }

    /// "a string (A store slug)", "one of: \"toast\", \"doordash\"", "an integer from 0".
    fn describe(&self) -> String {
        if let Some(choices) = &self.choices {
            let list: Vec<String> = choices.iter().map(Value::to_string).collect();
            return format!("one of: {}", list.join(", "));
        }
        let mut s = match self.ty {
            ParamType::Integer => "an integer".to_string(),
            other => format!("a {}", other.name()),
        };
        match (self.minimum, self.maximum) {
            (Some(min), Some(max)) => s.push_str(&format!(" from {min} to {max}")),
            (Some(min), None) => s.push_str(&format!(" from {min}")),
            (None, Some(max)) => s.push_str(&format!(" up to {max}")),
            (None, None) => {}
        }
        if let Some(d) = &self.description {
            s.push_str(&format!(" ({d})"));
        }
        s
    }
}

/// A manifest's `params`, read leniently: `None` when absent or malformed
/// (publish refuses a malformed one; this keeps an older build drawable).
pub fn parse(raw: Option<&Value>) -> Option<ParamsSchema> {
    let raw = raw?;
    check(raw).ok()?;
    let props = raw["properties"]
        .as_object()?
        .iter()
        .map(|(name, p)| Param {
            name: name.clone(),
            ty: p["type"]
                .as_str()
                .and_then(ParamType::parse)
                .unwrap_or(ParamType::String),
            description: p["description"].as_str().map(str::to_string),
            default: p.get("default").cloned(),
            choices: p["enum"].as_array().cloned(),
            minimum: p["minimum"].as_f64(),
            maximum: p["maximum"].as_f64(),
            min_length: p["minLength"].as_u64(),
            max_length: p["maxLength"].as_u64(),
        })
        .collect();
    let required = raw
        .get("required")
        .and_then(Value::as_array)
        .map(|r| {
            r.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Some(ParamsSchema {
        raw: raw.clone(),
        props,
        required,
    })
}

/// The publish check of a `params` schema: the first problem, as a message.
pub fn check(raw: &Value) -> Result<(), String> {
    let obj = raw.as_object().ok_or("`params` must be an object schema")?;
    if let Some(key) = obj
        .keys()
        .find(|k| !["type", "properties", "required"].contains(&k.as_str()))
    {
        return Err(format!(
            "`params.{key}` is not a field; a schema takes type, properties, required"
        ));
    }
    if obj.get("type").and_then(Value::as_str) != Some("object") {
        return Err("`params.type` must be \"object\"".into());
    }
    let props = obj
        .get("properties")
        .and_then(Value::as_object)
        .ok_or("`params.properties` must be an object")?;
    if props.len() > MAX_PARAMS {
        return Err(format!(
            "`params` declares {} settings; at most {MAX_PARAMS}",
            props.len()
        ));
    }
    for (name, p) in props {
        check_property(name, p)?;
    }
    if let Some(required) = obj.get("required") {
        let list = required
            .as_array()
            .ok_or("`params.required` must be a list")?;
        for r in list {
            let name = r.as_str().ok_or("`params.required` lists setting names")?;
            if !props.contains_key(name) {
                return Err(format!(
                    "`params.required` names `{name}`, which is not declared"
                ));
            }
        }
    }
    Ok(())
}

fn check_property(name: &str, p: &Value) -> Result<(), String> {
    let at = format!("params.properties.{name}");
    if !valid_segment(name) {
        return Err(format!(
            "`{at}`: a setting name uses lowercase letters, digits, `-` and `_`"
        ));
    }
    let obj = p.as_object().ok_or(format!("`{at}` must be an object"))?;
    if let Some(key) = obj.keys().find(|k| !PROPERTY_KEYS.contains(&k.as_str())) {
        return Err(format!("`{at}.{key}` is not supported"));
    }
    let ty = obj
        .get("type")
        .and_then(Value::as_str)
        .and_then(ParamType::parse)
        .ok_or(format!(
            "`{at}.type` must be string, integer, number or boolean"
        ))?;
    for key in ["title", "description"] {
        if obj.get(key).is_some_and(|v| !v.is_string()) {
            return Err(format!("`{at}.{key}` must be a string"));
        }
    }
    let numeric = matches!(ty, ParamType::Integer | ParamType::Number);
    for key in ["minimum", "maximum"] {
        if let Some(v) = obj.get(key)
            && (!numeric || !v.is_number())
        {
            return Err(format!("`{at}.{key}` is for number settings, as a number"));
        }
    }
    for key in ["minLength", "maxLength"] {
        if let Some(v) = obj.get(key)
            && (ty != ParamType::String || !v.is_u64())
        {
            return Err(format!(
                "`{at}.{key}` is for string settings, as a whole number"
            ));
        }
    }
    let choices = match obj.get("enum") {
        None => None,
        Some(v) => {
            let list = v
                .as_array()
                .filter(|l| !l.is_empty())
                .ok_or(format!("`{at}.enum` must be a non-empty list"))?;
            if list.iter().any(|c| !ty.admits(c)) {
                return Err(format!(
                    "`{at}.enum` holds a value that is not a {}",
                    ty.name()
                ));
            }
            Some(list)
        }
    };
    if let Some(d) = obj.get("default") {
        if !ty.admits(d) {
            return Err(format!("`{at}.default` is not a {}", ty.name()));
        }
        if choices.is_some_and(|c| !c.contains(d)) {
            return Err(format!("`{at}.default` is not one of its `enum`"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn schema() -> ParamsSchema {
        parse(Some(&json!({
            "type": "object",
            "properties": {
                "store": { "type": "string", "description": "A store slug", "minLength": 1 },
                "late_after": { "type": "integer", "minimum": 0, "default": 60 },
                "vendor": { "type": "string", "enum": ["toast", "doordash"] }
            },
            "required": ["store"]
        })))
        .expect("a valid schema")
    }

    #[test]
    fn values_get_defaults_and_every_problem_is_named() {
        let ok = schema().values(Some(json!({ "store": "mission" }).as_object().unwrap()));
        assert_eq!(
            Value::Object(ok.unwrap()),
            json!({ "store": "mission", "late_after": 60 })
        );
        let bad = schema()
            .values(Some(
                json!({ "late_after": -5, "vendor": "olo", "colour": 1 })
                    .as_object()
                    .unwrap(),
            ))
            .unwrap_err();
        let names: Vec<&str> = bad.iter().map(|(n, _, _)| n.as_str()).collect();
        for want in ["late_after", "vendor", "colour", "store"] {
            assert!(names.contains(&want), "{want} in {bad:?}");
        }
        let store = bad.iter().find(|(n, _, _)| n == "store").unwrap();
        assert_eq!(store.2, "Set params.store: a string (A store slug).");
    }

    #[test]
    fn a_setting_without_default_is_what_a_copy_needs() {
        assert_eq!(schema().needs(), ["store"]);
    }

    #[test]
    fn the_publish_check_refuses_what_the_subset_leaves_out() {
        for bad in [
            json!([]),
            json!({ "type": "array", "properties": {} }),
            json!({ "type": "object" }),
            json!({ "type": "object", "properties": { "s": { "type": "object" } } }),
            json!({ "type": "object", "properties": { "s": { "type": "string", "$ref": "x" } } }),
            json!({ "type": "object", "properties": { "s": { "type": "string", "minimum": 1 } } }),
            json!({ "type": "object", "properties": { "s": { "type": "integer", "enum": [] } } }),
            json!({ "type": "object", "properties": { "s": { "type": "integer", "enum": ["a"] } } }),
            json!({ "type": "object", "properties": { "s": { "type": "string", "enum": ["a"], "default": "b" } } }),
            json!({ "type": "object", "properties": { "S": { "type": "string" } } }),
            json!({ "type": "object", "properties": {}, "required": ["x"] }),
        ] {
            assert!(check(&bad).is_err(), "{bad}");
        }
        let nine: Map<String, Value> = (0..9)
            .map(|i| (format!("p{i}"), json!({ "type": "string" })))
            .collect();
        assert!(check(&json!({ "type": "object", "properties": nine })).is_err());
        assert!(check(&schema().raw).is_ok());
    }
}
