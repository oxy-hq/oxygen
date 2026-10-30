//! The strict read of a layout for a write (`check`: every problem, each with a
//! path and a fix) and the lenient read of a stored one (`read`). This file
//! reads the document's shape; `against` checks it against the org's widgets.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use super::{CatalogEntry, Item, LAYOUT_VERSION, Layout, Problem, Section, SectionKind, problem};
use crate::notifications::catalog::model::valid_segment;
use crate::widgets::model::{MAX_TITLE_CHARS, Size};

/// `doc` checked against `catalog` for a write: the layout, or every problem.
pub fn check(doc: &Value, catalog: &[CatalogEntry]) -> Result<Layout, Vec<Problem>> {
    let mut problems = Vec::new();
    let layout = parse(doc, &mut problems);
    if let Some(layout) = &layout {
        super::against::check_widgets(layout, catalog, &mut problems);
    }
    match (layout, problems.is_empty()) {
        (Some(layout), true) => Ok(layout),
        _ => Err(problems),
    }
}

/// A stored layout, read leniently: `None` when it no longer parses at all.
/// What it says about widgets is settled against the catalog when drawn.
pub fn read(doc: &Value) -> Option<Layout> {
    let mut problems = Vec::new();
    let layout = parse(doc, &mut problems)?;
    problems.is_empty().then_some(layout)
}

fn parse(doc: &Value, problems: &mut Vec<Problem>) -> Option<Layout> {
    let Some(obj) = doc.as_object() else {
        problems.push(problem(
            "",
            "a layout is an object",
            "Send { \"version\": 1, \"sections\": [ … ] }.",
        ));
        return None;
    };
    unknown_keys(obj, "", &["version", "sections"], problems);
    if obj.get("version").and_then(Value::as_u64) != Some(LAYOUT_VERSION) {
        problems.push(problem(
            "version",
            "the layout version must be 1",
            "Set \"version\": 1.",
        ));
    }
    let Some(list) = obj.get("sections").and_then(Value::as_array) else {
        problems.push(problem(
            "sections",
            "`sections` must be a list",
            "List the sections in the order the home draws them: widgets, apps, insights.",
        ));
        return None;
    };
    if list.is_empty() {
        // An empty home is never what an agent means: "clear it" is a reset.
        problems.push(problem(
            "sections",
            "a layout draws at least one section",
            "List at least one of: widgets, apps, insights. To go back to the default home, DELETE the layout (`oxyc home reset`).",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut groups = 0;
    let sections = list
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            let section = section(s, &format!("sections[{i}]"), &mut seen, groups, problems)?;
            groups += usize::from(matches!(section, Section::Widgets { .. }));
            Some(section)
        })
        .collect();
    Some(Layout { sections })
}

/// One section. `apps` and `insights` at most once; `widgets` any number of
/// times, `groups` counting the widgets sections before this one.
fn section(
    raw: &Value,
    at: &str,
    seen: &mut BTreeSet<&'static str>,
    groups: usize,
    problems: &mut Vec<Problem>,
) -> Option<Section> {
    let Some(obj) = raw.as_object() else {
        problems.push(problem(
            at,
            "a section is an object",
            "Write { \"kind\": \"widgets\" }.",
        ));
        return None;
    };
    let Some(kind) = obj
        .get("kind")
        .and_then(Value::as_str)
        .and_then(SectionKind::parse)
    else {
        problems.push(problem(
            format!("{at}.kind"),
            "not a section kind",
            "Use one of: widgets, apps, insights.",
        ));
        return None;
    };
    match kind {
        SectionKind::Widgets => Some(widgets_section(obj, at, groups == 0, problems)),
        SectionKind::Apps | SectionKind::Insights => {
            if !seen.insert(kind.as_str()) {
                problems.push(problem(
                    format!("{at}.kind"),
                    format!("the {} section is listed twice", kind.as_str()),
                    "List it once; only widgets sections may repeat.",
                ));
                return None;
            }
            unknown_keys(obj, at, &["kind"], problems);
            Some(match kind {
                SectionKind::Apps => Section::Apps,
                _ => Section::Insights,
            })
        }
    }
}

fn widgets_section(
    obj: &Map<String, Value>,
    at: &str,
    first: bool,
    problems: &mut Vec<Problem>,
) -> Section {
    unknown_keys(obj, at, &["kind", "title", "items", "others"], problems);
    let title = title(obj.get("title"), &format!("{at}.title"), problems);
    let hide_others = match (obj.get("others").map(Value::as_str), first) {
        (None, _) | (Some(Some("show")), true) => false,
        (Some(Some("hide")), true) => true,
        (Some(_), false) => {
            problems.push(problem(
                format!("{at}.others"),
                "`others` belongs on the first widgets group",
                "Move it to the first widgets section; widgets a layout does not list go there.",
            ));
            false
        }
        (Some(_), true) => {
            problems.push(problem(
                format!("{at}.others"),
                "`others` is \"show\" or \"hide\"",
                "Use \"show\" to add widgets the layout does not list, \"hide\" to leave them out.",
            ));
            false
        }
    };
    let items = match obj.get("items") {
        None => Vec::new(),
        Some(Value::Array(list)) => list
            .iter()
            .enumerate()
            .filter_map(|(i, it)| item(it, &format!("{at}.items[{i}]"), problems))
            .collect(),
        Some(_) => {
            problems.push(problem(
                format!("{at}.items"),
                "`items` must be a list",
                "List widgets as { \"widget\": \"<app>/<widget>\", \"size\"?: …, \"hidden\"?: true }.",
            ));
            Vec::new()
        }
    };
    Section::Widgets {
        title,
        items,
        hide_others,
    }
}

fn item(raw: &Value, at: &str, problems: &mut Vec<Problem>) -> Option<Item> {
    let Some(obj) = raw.as_object() else {
        problems.push(problem(
            at,
            "an item is an object",
            "Write { \"widget\": \"<app>/<widget>\" }.",
        ));
        return None;
    };
    unknown_keys(
        obj,
        at,
        &["widget", "id", "title", "size", "hidden", "params"],
        problems,
    );
    let Some(widget) = obj.get("widget").and_then(Value::as_str).map(str::trim) else {
        problems.push(problem(
            format!("{at}.widget"),
            "`widget` names a widget as <app>/<widget>",
            "`GET …/home` lists every widget under `catalog`.",
        ));
        return None;
    };
    Some(Item {
        widget: widget.to_string(),
        id: copy_id(obj.get("id"), &format!("{at}.id"), problems),
        title: title(obj.get("title"), &format!("{at}.title"), problems),
        size: size(obj.get("size"), &format!("{at}.size"), problems),
        hidden: flag(obj.get("hidden"), &format!("{at}.hidden"), problems),
        params: params(obj.get("params"), &format!("{at}.params"), problems),
    })
}

fn copy_id(v: Option<&Value>, at: &str, problems: &mut Vec<Problem>) -> Option<String> {
    let v = v?;
    match v.as_str().filter(|s| valid_segment(s)) {
        Some(id) => Some(id.to_string()),
        None => {
            problems.push(problem(
                at,
                format!("{v} is not a copy id"),
                "Use lowercase letters, digits, `-` and `_`, e.g. \"mission\".",
            ));
            None
        }
    }
}

fn title(v: Option<&Value>, at: &str, problems: &mut Vec<Problem>) -> Option<String> {
    let v = v?;
    match v.as_str().map(str::trim).filter(|s| !s.is_empty()) {
        Some(t) if t.chars().count() <= MAX_TITLE_CHARS => Some(t.to_string()),
        _ => {
            problems.push(problem(
                at,
                format!("a title is a non-empty string of at most {MAX_TITLE_CHARS} characters"),
                "Shorten it, or leave it out.",
            ));
            None
        }
    }
}

fn size(v: Option<&Value>, at: &str, problems: &mut Vec<Problem>) -> Option<Size> {
    let v = v?;
    let size = v.as_str().and_then(Size::parse);
    if size.is_none() {
        problems.push(problem(
            at,
            format!("{v} is not a size"),
            "Use one of: regular, wide, full.",
        ));
    }
    size
}

fn flag(v: Option<&Value>, at: &str, problems: &mut Vec<Problem>) -> bool {
    match v {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => {
            problems.push(problem(
                at,
                "`hidden` is true or false",
                "Set \"hidden\": true, or leave it out.",
            ));
            false
        }
    }
}

fn params(v: Option<&Value>, at: &str, problems: &mut Vec<Problem>) -> Option<Map<String, Value>> {
    let v = v?;
    match v.as_object() {
        Some(obj) => Some(obj.clone()),
        None => {
            problems.push(problem(
                at,
                "`params` is an object of settings",
                "Write { \"<setting>\": <value> }; `GET …/home` lists each widget's settings under `catalog`.",
            ));
            None
        }
    }
}

fn unknown_keys(obj: &Map<String, Value>, at: &str, allowed: &[&str], problems: &mut Vec<Problem>) {
    for key in obj.keys().filter(|k| !allowed.contains(&k.as_str())) {
        let path = if at.is_empty() {
            key.clone()
        } else {
            format!("{at}.{key}")
        };
        problems.push(problem(
            path,
            format!("`{key}` is not a field here"),
            format!("Remove it; this takes {}.", allowed.join(", ")),
        ));
    }
}
