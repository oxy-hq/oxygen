//! What an org's home looks like, as data: the sections it draws in order, the
//! widgets in each at their size, every widget it could draw, and the layout
//! document behind it — so `oxyc` and an LLM can read the home, change it, and
//! write it back. `internal-docs/home-widgets.md` § Describing the home.

use oxy_app_core::widgets::layout::{CatalogEntry, Drawn, Layout, effective, resolve};
use oxy_app_core::widgets::model::Size;
use serde::Serialize;
use serde_json::{Map, Value};
use uuid::Uuid;

use super::load::WidgetApp;

#[derive(Debug, Serialize)]
pub struct Description {
    pub org: OrgRef,
    /// `0` until the home is customized; send it back with a write.
    pub revision: i64,
    pub customized: bool,
    /// The stored document, or `null`.
    pub layout: Option<Value>,
    /// A complete document for what shows now: the round-trip form.
    pub effective_layout: Value,
    pub sections: Vec<SectionView>,
    pub catalog: Vec<CatalogItem>,
}

#[derive(Debug, Serialize)]
pub struct OrgRef {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SectionView {
    Widgets {
        title: Option<String>,
        items: Vec<PlacedView>,
    },
    Apps,
    Insights,
}

#[derive(Debug, Serialize)]
pub struct PlacedView {
    /// The copy's name: `<app>/<widget>` or `<app>/<widget>#<id>`.
    pub key: String,
    pub widget: String,
    /// The copy's id, when the layout named one.
    pub id: Option<String>,
    pub app: AppRef,
    /// The layout's title for the copy, else the widget's.
    pub title: String,
    /// The copy's settings, defaults filled in.
    pub params: Map<String, Value>,
    pub size: Size,
    pub sizes: Vec<Size>,
}

#[derive(Debug, Serialize)]
pub struct AppRef {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct CatalogItem {
    pub widget: String,
    pub app: AppRef,
    pub title: String,
    /// Its default size.
    pub size: Size,
    pub sizes: Vec<Size>,
    /// Its settings schema, as the manifest declares it; `null` when none.
    pub params: Option<Value>,
    /// Settings a copy must be given: it is not shown until a layout does.
    pub needs_params: Vec<String>,
    pub shown: bool,
}

/// The home `layout` (stored as `stored`, at `revision`) draws from `catalog`.
pub fn describe(
    org: OrgRef,
    stored: Option<(Value, i64)>,
    layout: Option<&Layout>,
    catalog: &[(CatalogEntry, &WidgetApp)],
) -> Description {
    let entries: Vec<CatalogEntry> = catalog.iter().map(|(e, _)| e.clone()).collect();
    let drawn = resolve(layout, &entries);
    let shown: Vec<usize> = drawn
        .iter()
        .flat_map(|d| match d {
            Drawn::Widgets { placed, .. } => placed.iter().map(|p| p.index).collect(),
            _ => vec![],
        })
        .collect();
    let app_ref = |a: &WidgetApp| AppRef {
        id: a.id,
        slug: a.slug.clone(),
        name: a.name.clone(),
    };
    let sections = drawn
        .iter()
        .map(|d| match d {
            Drawn::Apps => SectionView::Apps,
            Drawn::Insights => SectionView::Insights,
            Drawn::Widgets { title, placed, .. } => SectionView::Widgets {
                title: title.clone(),
                items: placed
                    .iter()
                    .map(|p| {
                        let (entry, app) = &catalog[p.index];
                        PlacedView {
                            key: p.key(&entries),
                            widget: entry.key.clone(),
                            id: p.instance.clone(),
                            app: app_ref(app),
                            title: p.title.clone().unwrap_or_else(|| entry.def.title.clone()),
                            params: p.params.clone(),
                            size: p.size,
                            sizes: entry.def.sizes.clone(),
                        }
                    })
                    .collect(),
            },
        })
        .collect();
    let catalog_items = catalog
        .iter()
        .enumerate()
        .map(|(i, (entry, app))| CatalogItem {
            widget: entry.key.clone(),
            app: app_ref(app),
            title: entry.def.title.clone(),
            size: entry.def.size,
            sizes: entry.def.sizes.clone(),
            params: entry.def.params.as_ref().map(|p| p.raw.clone()),
            needs_params: entry
                .def
                .params
                .as_ref()
                .map(|p| p.needs().into_iter().map(str::to_string).collect())
                .unwrap_or_default(),
            shown: shown.contains(&i),
        })
        .collect();
    let (layout_doc, revision) = match stored {
        Some((doc, revision)) => (Some(doc), revision),
        None => (None, 0),
    };
    Description {
        org,
        revision,
        customized: layout_doc.is_some(),
        layout: layout_doc,
        effective_layout: effective(&drawn, &entries),
        sections,
        catalog: catalog_items,
    }
}
