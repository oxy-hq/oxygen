//! A page of someone's inbox, put back together.
//!
//! The signals (who, when, read or not) are in Oxy's database; what each one
//! says is in its org's own store. This reads a page of signals, then the
//! content for that page — one query per org on the page, never per row — and
//! the app each came from, so the reader sees one list. Each item is placed
//! in its source's catalogue (`catalog`): category, labels, level, actions.
//!
//! An org whose store cannot be read right now still lists its notifications,
//! with `title: null`: the person learns something is there, and the badge
//! count — which never touches an org's store — stays right.
//!
//! A platform notice (no app, an `oxy.*` kind) has no content row: its words
//! are rendered from Oxy's own records ([`super::platform::render`]).

use std::collections::{HashMap, HashSet};

use entity::{apps, notifications, organizations};
use oxy_oltp::inbox::{self, InboxMessage};
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use serde::Serialize;
use uuid::Uuid;

use super::catalog::model::{ActionView, Level};
use super::catalog::platform::oxygen;
use super::catalog::source::{KindInfo, SourceCatalogue};
use super::links::inbox_href;
use super::platform::render::{self, About, Rendered};

/// The app a notification came from, as the inbox names it.
#[derive(Debug, Clone, Serialize)]
pub struct InboxApp {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
}

/// Where an item sits in its source's tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CategoryRef {
    /// `operations/food-safety`; `general` for a kind with none.
    pub path: String,
    /// From the top category down: `["Store operations", "Food safety"]`.
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InboxItem {
    pub id: Uuid,
    pub org_id: Uuid,
    pub kind: String,
    pub created_at: chrono::DateTime<chrono::FixedOffset>,
    pub read_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    /// `None` when the org's store could not be read for this page.
    pub title: Option<String>,
    pub body: Option<String>,
    /// Where tapping it lands, relative to whatever host the reader is on:
    /// a path in Oxygen's web app.
    pub href: Option<String>,
    /// The app-relative path exactly as the app gave it to `ctx.notify`
    /// (`/bookings/1`), for the app's own router — the bell inside the app.
    /// `None` for a platform notice and for a notification without a link.
    pub link: Option<String>,
    pub app: Option<InboxApp>,
    pub category: CategoryRef,
    pub kind_label: String,
    pub level: Level,
    /// The thread's opaque key; items sharing one collapse into the newest.
    pub thread: Option<String>,
    pub actions: Vec<ActionView>,
    /// `href` on the admin host, absolute, for a phone or a shared link.
    /// `None` without a link, or when the deployment has no admin host
    /// configured (`OXY_API_URL`).
    pub url: Option<String>,
}

/// Turn a page of signals into inbox items, in the page's order.
pub async fn items(
    db: &DatabaseConnection,
    page: Vec<notifications::Model>,
) -> Result<Vec<InboxItem>, DbErr> {
    let content = content_for(db, &page).await;
    let org_slugs = org_slugs(db, &page).await?;
    let (apps, catalogues) = apps_for(db, &page).await?;
    let notices = platform_words(db, &page).await?;
    let platform = oxygen();
    let empty = super::catalog::live::from_manifest(None);
    Ok(page
        .into_iter()
        .map(|n| {
            let catalogue = match n.app_id {
                None => &platform,
                Some(app) => catalogues.get(&app).unwrap_or(&empty),
            };
            let info = catalogue.describe(&n.kind);
            if render::is_platform(&n) {
                let words = About::of(&n)
                    .and_then(|a| notices.get(&a).cloned())
                    .unwrap_or_else(|| render::fallback(&n.kind));
                return platform_item(n, words, info);
            }
            let msg = n.message_id.and_then(|id| content.get(&id));
            let app = n.app_id.and_then(|id| apps.get(&id)).cloned();
            let href = msg.and_then(|m| {
                let org = org_slugs.get(&n.org_id)?;
                inbox_href(
                    org,
                    app.as_ref().map(|a| a.slug.as_str()),
                    m.link.as_deref(),
                )
            });
            let own = own_words(&n);
            let mut item = base(n, info);
            item.title = msg.map(|m| m.title.clone());
            item.body = msg.and_then(|m| m.body.clone());
            if msg.is_none()
                && let Some((title, body)) = own
            {
                item.title = Some(title);
                item.body = body;
            }
            item.href = href;
            item.link = msg.and_then(|m| m.link.clone());
            item.app = app;
            item
        })
        .collect())
}

/// An item with its place in the tree and no words yet.
fn base(n: notifications::Model, info: KindInfo) -> InboxItem {
    InboxItem {
        id: n.id,
        org_id: n.org_id,
        kind: n.kind,
        created_at: n.created_at,
        read_at: n.read_at,
        title: None,
        body: None,
        href: None,
        link: None,
        app: None,
        category: CategoryRef {
            path: info.category,
            labels: info.category_labels,
        },
        kind_label: info.label,
        level: info.level,
        thread: n.thread_key,
        actions: info.actions.iter().map(ActionView::from).collect(),
        url: None,
    }
}

/// The row's own words, for a notice that is neither an app's nor a platform
/// kind: no app, no content row in an org store, and a title on the row
/// itself. Experiment owner notices (`server::experiment::notify`) are the one
/// writer; every other row writes the legacy columns empty, so this is `None`
/// for them.
fn own_words(n: &notifications::Model) -> Option<(String, Option<String>)> {
    if n.app_id.is_some() || n.message_id.is_some() || n.title.is_empty() {
        return None;
    }
    Some((n.title.clone(), n.body.clone()))
}

fn platform_item(n: notifications::Model, words: Rendered, info: KindInfo) -> InboxItem {
    let mut item = base(n, info);
    item.title = Some(words.title);
    item.body = words.body;
    item.href = words.href;
    item
}

/// How a request's items link out: `href` relative to its host (absolute on
/// a custom-app host), `url` absolute on the admin host.
#[derive(Debug, Clone, Default)]
pub struct Links {
    pub href_base: String,
    pub admin_base: Option<String>,
}

impl Links {
    pub fn for_request(headers: &axum::http::HeaderMap) -> Self {
        let host = headers
            .get(axum::http::header::HOST)
            .and_then(|v| v.to_str().ok());
        let admin_base = oxy_app_core::custom_apps_host_dispatch::admin_base_url();
        Self {
            href_base: href_base(host, admin_base.clone()),
            admin_base,
        }
    }

    /// Fill each item's `url`, then make its `href` absolute where the host
    /// needs it.
    pub fn apply(&self, items: &mut [InboxItem]) {
        for item in items.iter_mut() {
            item.url = absolute(item.href.as_deref(), self.admin_base.as_deref());
        }
        absolutize(items, &self.href_base);
    }
}

/// `href` on `admin_base`; an already absolute `href` as it is.
pub fn absolute(href: Option<&str>, admin_base: Option<&str>) -> Option<String> {
    let href = href?;
    if !href.starts_with('/') {
        return Some(href.to_string());
    }
    admin_base.map(|base| format!("{base}{href}"))
}

/// What to put before every `href` for a reader on `host`: the admin host's
/// origin on a custom-app subdomain, nothing elsewhere.
///
/// A custom-app host rewrites every path it does not reserve into the app it
/// serves (`custom_apps_host_dispatch`), so a bell inside one app that opened
/// another app's `/customer-apps/…` path, or a platform notice's web-app path,
/// would land inside the current app instead. There the links go to the admin
/// host absolutely. With no admin host configured (`OXY_API_URL` unset, a
/// laptop) they stay relative: a link that may not resolve beats a made-up
/// host. Same rule as the shell context's `product_url_base`.
pub fn href_base(host: Option<&str>, admin_base: Option<String>) -> String {
    let on_app_host = host
        .and_then(oxy_app_core::custom_apps_host_dispatch::parse_subdomain)
        .is_some();
    match on_app_host {
        true => admin_base.unwrap_or_default(),
        false => String::new(),
    }
}

/// Prefix every relative `href` with `base` (see [`href_base`]). `link` is
/// left alone: it is relative to the app, for the app's own router.
pub fn absolutize(items: &mut [InboxItem], base: &str) {
    if base.is_empty() {
        return;
    }
    for item in items {
        if let Some(href) = item.href.as_mut().filter(|h| h.starts_with('/')) {
            *href = format!("{base}{href}");
        }
    }
}

/// The words for every platform notice on the page.
async fn platform_words(
    db: &DatabaseConnection,
    page: &[notifications::Model],
) -> Result<HashMap<About, Rendered>, DbErr> {
    let abouts: HashSet<About> = page.iter().filter_map(About::of).collect();
    if abouts.is_empty() {
        return Ok(HashMap::new());
    }
    render::render_all(db, &abouts).await
}

/// Content for the page, keyed by message id — one read per org. An org whose
/// store fails is logged and left out; its rows read as `title: null`.
async fn content_for(
    db: &DatabaseConnection,
    page: &[notifications::Model],
) -> HashMap<Uuid, InboxMessage> {
    let mut by_org: HashMap<Uuid, HashSet<Uuid>> = HashMap::new();
    // A platform notice's `message_id` is its send, with no content row.
    for n in page.iter().filter(|n| !render::is_platform(n)) {
        if let Some(m) = n.message_id {
            by_org.entry(n.org_id).or_default().insert(m);
        }
    }
    let mut out = HashMap::new();
    for (org_id, ids) in by_org {
        let ids: Vec<Uuid> = ids.into_iter().collect();
        match inbox::read(db, org_id, &ids).await {
            Ok(msgs) => out.extend(msgs.into_iter().map(|m| (m.id, m))),
            Err(e) => tracing::warn!(%org_id, error = %e, "inbox content unavailable for this org"),
        }
    }
    out
}

async fn org_slugs(
    db: &DatabaseConnection,
    page: &[notifications::Model],
) -> Result<HashMap<Uuid, String>, DbErr> {
    let ids: HashSet<Uuid> = page.iter().map(|n| n.org_id).collect();
    Ok(organizations::Entity::find()
        .filter(organizations::Column::Id.is_in(ids))
        .all(db)
        .await?
        .into_iter()
        .map(|o| (o.id, o.slug))
        .collect())
}

/// The apps on the page, and each one's live catalogue.
async fn apps_for(
    db: &DatabaseConnection,
    page: &[notifications::Model],
) -> Result<(HashMap<Uuid, InboxApp>, HashMap<Uuid, SourceCatalogue>), DbErr> {
    let ids: HashSet<Uuid> = page.iter().filter_map(|n| n.app_id).collect();
    if ids.is_empty() {
        return Ok(Default::default());
    }
    let rows = apps::Entity::find()
        .filter(apps::Column::Id.is_in(ids))
        .all(db)
        .await?;
    let catalogues = super::catalog::live::load_many(db, &rows).await?;
    let apps = rows
        .into_iter()
        .map(|a| {
            (
                a.id,
                InboxApp {
                    id: a.id,
                    slug: a.slug,
                    name: a.name,
                },
            )
        })
        .collect();
    Ok((apps, catalogues))
}

#[cfg(test)]
#[path = "inbox_view_tests.rs"]
mod tests;
