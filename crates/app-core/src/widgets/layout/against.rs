//! A layout checked against the org's widgets: every named widget exists, each
//! copy is named once, at a size its widget allows, with settings its schema
//! accepts.

use std::collections::BTreeSet;

use super::{CatalogEntry, Item, Layout, Problem, Section, problem};
use crate::widgets::model::Size;

pub(super) fn check_widgets(
    layout: &Layout,
    catalog: &[CatalogEntry],
    problems: &mut Vec<Problem>,
) {
    let mut seen = BTreeSet::new();
    for (s, section) in layout.sections.iter().enumerate() {
        let Section::Widgets { items, .. } = section else {
            continue;
        };
        for (i, item) in items.iter().enumerate() {
            let at = format!("sections[{s}].items[{i}]");
            check_item(item, &at, catalog, &mut seen, problems);
        }
    }
}

fn check_item(
    item: &Item,
    at: &str,
    catalog: &[CatalogEntry],
    seen: &mut BTreeSet<String>,
    problems: &mut Vec<Problem>,
) {
    let Some(entry) = catalog.iter().find(|e| e.key == item.widget) else {
        problems.push(problem(
            format!("{at}.widget"),
            format!("no widget named {}", item.widget),
            match nearest(&item.widget, catalog) {
                Some(close) => format!(
                    "Did you mean {close}? `GET …/home` lists every widget under `catalog`."
                ),
                None => "`GET …/home` lists every widget under `catalog`.".to_string(),
            },
        ));
        return;
    };
    if !seen.insert(item.key()) {
        problems.push(problem(
            format!("{at}.widget"),
            format!("{} is listed twice", item.key()),
            "To show a widget more than once, give each copy its own `id`.",
        ));
    }
    if let Some(size) = item.size
        && !entry.def.sizes.contains(&size)
    {
        problems.push(problem(
            format!("{at}.size"),
            format!("{} does not allow \"{}\"", item.widget, size.as_str()),
            format!("Use one of: {}.", names(&entry.def.sizes)),
        ));
    }
    // A hidden copy is not drawn, so its settings are not asked for.
    if !item.hidden {
        check_params(item, entry, at, problems);
    }
}

fn check_params(item: &Item, entry: &CatalogEntry, at: &str, problems: &mut Vec<Problem>) {
    match &entry.def.params {
        Some(schema) => {
            if let Err(found) = schema.values(item.params.as_ref()) {
                problems.extend(found.into_iter().map(|(name, message, fix)| {
                    problem(
                        format!("{at}.params.{name}"),
                        format!("{}: {message}", item.widget),
                        fix,
                    )
                }));
            }
        }
        None if item.params.as_ref().is_some_and(|p| !p.is_empty()) => {
            problems.push(problem(
                format!("{at}.params"),
                format!("{} takes no settings", item.widget),
                "Remove `params`.",
            ));
        }
        None => {}
    }
}

/// The catalog key closest to `name` by edit distance, when close enough to be
/// a typo.
fn nearest<'a>(name: &str, catalog: &'a [CatalogEntry]) -> Option<&'a str> {
    catalog
        .iter()
        .map(|e| (edit_distance(name, &e.key), e.key.as_str()))
        .filter(|(d, _)| *d <= 3)
        .min()
        .map(|(_, k)| k)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != *cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

fn names(sizes: &[Size]) -> String {
    sizes
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}
