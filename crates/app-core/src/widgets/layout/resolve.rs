//! What a layout draws (`resolve`), and the complete document for it
//! (`effective`), the form an agent reads, edits and writes back.

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use super::{CatalogEntry, Item, LAYOUT_VERSION, Layout, Section, copy_key};
use crate::widgets::model::{Size, WidgetDef};

/// What the home draws, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Drawn {
    /// A group of widget copies, in order; `hide_others` is the first group's
    /// own flag (kept so the effective layout writes it back).
    Widgets {
        title: Option<String>,
        placed: Vec<Placed>,
        hide_others: bool,
    },
    Apps,
    Insights,
}

/// One copy the home draws: its catalog index, size, and settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    pub index: usize,
    pub size: Size,
    /// The copy's id, when the layout named one.
    pub instance: Option<String>,
    /// The layout's title for it, shown instead of the widget's.
    pub title: Option<String>,
    /// Its settings, defaults filled in; empty when it declares none.
    pub params: Map<String, Value>,
}

impl Placed {
    /// How the copy is named: `<app>/<widget>` or `<app>/<widget>#<id>`.
    pub fn key(&self, catalog: &[CatalogEntry]) -> String {
        copy_key(&catalog[self.index].key, self.instance.as_deref())
    }
}

/// A widget with every setting defaulted, or `None` when one has no default
/// (it cannot be drawn without a layout entry saying which).
fn default_params(def: &WidgetDef) -> Option<Map<String, Value>> {
    match &def.params {
        None => Some(Map::new()),
        Some(schema) => schema.values(None).ok(),
    }
}

fn by_default(index: usize, catalog: &[CatalogEntry]) -> Option<Placed> {
    let def = &catalog[index].def;
    Some(Placed {
        index,
        size: def.size,
        instance: None,
        title: None,
        params: default_params(def)?,
    })
}

/// What `layout` (or the default, for `None`) draws from `catalog`, which is in
/// default order (app name, then manifest order).
pub fn resolve(layout: Option<&Layout>, catalog: &[CatalogEntry]) -> Vec<Drawn> {
    let Some(layout) = layout else {
        let all = (0..catalog.len())
            .filter_map(|i| by_default(i, catalog))
            .collect();
        return vec![
            Drawn::Widgets {
                title: None,
                placed: all,
                hide_others: false,
            },
            Drawn::Apps,
            Drawn::Insights,
        ];
    };
    let listed: BTreeSet<usize> = layout
        .sections
        .iter()
        .flat_map(|s| match s {
            Section::Widgets { items, .. } => items.as_slice(),
            _ => &[],
        })
        .filter_map(|item| catalog.iter().position(|e| e.key == item.widget))
        .collect();
    let mut seen = BTreeSet::new();
    let mut first = true;
    layout
        .sections
        .iter()
        .map(|s| match s {
            Section::Apps => Drawn::Apps,
            Section::Insights => Drawn::Insights,
            Section::Widgets {
                title,
                items,
                hide_others,
            } => {
                let mut placed = place(items, catalog, &mut seen);
                if std::mem::take(&mut first) && !hide_others {
                    // Widgets the layout does not name go to the first group.
                    placed.extend(
                        (0..catalog.len())
                            .filter(|i| !listed.contains(i))
                            .filter_map(|i| by_default(i, catalog)),
                    );
                }
                Drawn::Widgets {
                    title: title.clone(),
                    placed,
                    hide_others: *hide_others,
                }
            }
        })
        .collect()
}

/// A group's items, leniently: a widget an app dropped, a repeated copy, or
/// settings its schema no longer accepts skip that copy rather than fail the home.
fn place(items: &[Item], catalog: &[CatalogEntry], seen: &mut BTreeSet<String>) -> Vec<Placed> {
    let mut placed = Vec::new();
    for item in items {
        let Some(index) = catalog.iter().position(|e| e.key == item.widget) else {
            continue;
        };
        if !seen.insert(item.key()) || item.hidden {
            continue;
        }
        let def = &catalog[index].def;
        let params = match &def.params {
            Some(schema) => match schema.values(item.params.as_ref()) {
                Ok(values) => values,
                Err(_) => continue,
            },
            None => Map::new(),
        };
        placed.push(Placed {
            index,
            size: item
                .size
                .filter(|s| def.sizes.contains(s))
                .unwrap_or(def.size),
            instance: item.id.clone(),
            title: item.title.clone(),
            params,
        });
    }
    placed
}

/// A complete layout document for what `drawn` shows: every copy in its group,
/// in order, with its size, id, title and settings; every widget not drawn
/// listed `hidden` in the first group. The round-trip form: read it, change
/// it, write it back.
pub fn effective(drawn: &[Drawn], catalog: &[CatalogEntry]) -> Value {
    let shown: BTreeSet<usize> = drawn
        .iter()
        .flat_map(|d| match d {
            Drawn::Widgets { placed, .. } => placed.iter().map(|p| p.index).collect(),
            _ => vec![],
        })
        .collect();
    let mut first = true;
    let sections: Vec<Value> = drawn
        .iter()
        .map(|d| match d {
            Drawn::Apps => json!({ "kind": "apps" }),
            Drawn::Insights => json!({ "kind": "insights" }),
            Drawn::Widgets {
                title,
                placed,
                hide_others,
            } => {
                let mut items: Vec<Value> = placed.iter().map(|p| item_doc(p, catalog)).collect();
                let mut group = json!({ "kind": "widgets" });
                if let Some(title) = title {
                    group["title"] = json!(title);
                }
                if std::mem::take(&mut first) {
                    items.extend(
                        (0..catalog.len())
                            .filter(|i| !shown.contains(i))
                            .map(|i| json!({ "widget": catalog[i].key, "hidden": true })),
                    );
                    group["others"] = json!(if *hide_others { "hide" } else { "show" });
                }
                group["items"] = Value::Array(items);
                group
            }
        })
        .collect();
    json!({ "version": LAYOUT_VERSION, "sections": sections })
}

fn item_doc(p: &Placed, catalog: &[CatalogEntry]) -> Value {
    let mut item = json!({ "widget": catalog[p.index].key, "size": p.size.as_str() });
    if let Some(id) = &p.instance {
        item["id"] = json!(id);
    }
    if let Some(title) = &p.title {
        item["title"] = json!(title);
    }
    if !p.params.is_empty() {
        item["params"] = Value::Object(p.params.clone());
    }
    item
}
