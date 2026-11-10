//! `GET /api/notifications/catalog` — every source that can tell this person
//! something, as a tree: source → categories → kinds, with their mutes at
//! every node and unread counts per subtree.
//!
//! Self-scoped like the inbox. Oxygen is listed once the person has received
//! one of its notices or set something on it; an app once it has notified
//! them, when they set something on it, or when its live build declares a
//! catalogue and they are in its audience. Catalogues are config read from
//! Oxy's database; no org store is touched.
//!
//! The settings document (`GET /api/notifications/settings`) lists the same
//! sources ([`listed_sources`]), and both read what was set from
//! `notification_node_settings`: a mute here is a channel set off there.

use std::collections::{BTreeSet, HashMap, HashSet};

use axum::Json;
use axum::http::StatusCode;
use entity::{apps, notifications, organizations};
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QuerySelect};
use serde::Serialize;
use tracing::instrument;
use uuid::Uuid;

use super::platform::oxygen;
use super::source::SourceCatalogue;
use super::tree::{self, CategoryNode, Facts};
use crate::preferences::{Reachability, reachability};
use crate::settings::model::Node;
use crate::settings::store::{self, UserNodes};
use oxy_app_core::audience::PersonReach;

#[derive(Debug, Serialize)]
pub struct SourceApp {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub org_slug: String,
}

#[derive(Debug, Serialize)]
pub struct SourceNode {
    /// `oxygen` | `app`.
    pub source: &'static str,
    /// `None` for Oxygen.
    pub app: Option<SourceApp>,
    pub label: String,
    /// Mutes on the whole source.
    pub muted: Vec<&'static str>,
    pub unread: u64,
    pub categories: Vec<CategoryNode>,
}

#[derive(Debug, Serialize)]
pub struct CatalogResponse {
    pub sources: Vec<SourceNode>,
    pub reach: Reachability,
}

/// `GET /api/notifications/catalog`
#[instrument(skip_all)]
pub async fn catalog(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
) -> Result<Json<CatalogResponse>, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    catalog_for(&db, &user).await.map(Json).map_err(|e| {
        tracing::warn!(error = %e, "notification catalogue failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

/// Everything `user` has, per source (`None` = Oxygen).
struct Had {
    kinds: HashMap<Option<Uuid>, Facts>,
}

/// One source as both the catalogue and the settings document list it.
pub struct Listed {
    /// `None` for Oxygen.
    pub app: Option<SourceApp>,
    pub label: String,
    pub catalogue: SourceCatalogue,
    pub facts: Facts,
}

/// The catalogue as `user` sees it. Separate from the route so it can be
/// driven against a test database.
pub async fn catalog_for(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
) -> Result<CatalogResponse, DbErr> {
    let sources = listed_sources(db, user)
        .await?
        .into_iter()
        .map(source_node)
        .collect();
    Ok(CatalogResponse {
        sources,
        reach: reachability(db, user.id).await?,
    })
}

/// Every source `user` is listed, in order: Oxygen first, then apps by name.
pub async fn listed_sources(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
) -> Result<Vec<Listed>, DbErr> {
    let blocked: HashSet<Uuid> = user.blocked_orgs().iter().copied().collect();
    let mut had = had(db, user.id, &blocked).await?;
    let mut sources = Vec::new();
    let oxygen_facts = had.facts(None);
    if !oxygen_facts.kinds.is_empty() || !oxygen_facts.nodes.is_empty() {
        sources.push(Listed {
            app: None,
            label: "Oxygen".into(),
            catalogue: oxygen(),
            facts: oxygen_facts,
        });
    }
    for (app, org_slug, catalogue) in apps_for(db, user, &blocked, &had).await? {
        let facts = had.facts(Some(app.id));
        sources.push(Listed {
            label: app.name.clone(),
            app: Some(SourceApp {
                id: app.id,
                slug: app.slug,
                name: app.name,
                org_slug,
            }),
            catalogue,
            facts,
        });
    }
    Ok(sources)
}

fn source_node(listed: Listed) -> SourceNode {
    let (categories, unread) = tree::build(&listed.catalogue, &listed.facts);
    SourceNode {
        source: listed.catalogue.source.as_str(),
        app: listed.app,
        label: listed.label,
        muted: listed.facts.scope_muted(""),
        unread,
        categories,
    }
}

impl Had {
    /// This source's facts.
    fn facts(&mut self, source: Option<Uuid>) -> Facts {
        self.kinds.remove(&source).unwrap_or_default()
    }

    fn sources(&self) -> BTreeSet<Uuid> {
        self.kinds.keys().filter_map(|s| *s).collect()
    }
}

/// What `user_id` received (with unread counts) and set, per source.
async fn had(
    db: &DatabaseConnection,
    user_id: Uuid,
    blocked: &HashSet<Uuid>,
) -> Result<Had, DbErr> {
    let mut kinds: HashMap<Option<Uuid>, Facts> = HashMap::new();
    let received: Vec<(Option<Uuid>, String, i64)> = notifications::Entity::find()
        .select_only()
        .column(notifications::Column::AppId)
        .column(notifications::Column::Kind)
        .column_as(
            Expr::cust("COUNT(*) FILTER (WHERE read_at IS NULL)"),
            "unread",
        )
        .filter(notifications::Column::UserId.eq(user_id))
        .filter(notifications::Column::OrgId.is_not_in(blocked.iter().copied()))
        .group_by(notifications::Column::AppId)
        .group_by(notifications::Column::Kind)
        .into_tuple()
        .all(db)
        .await?;
    for (app, kind, unread) in received {
        let facts = kinds.entry(app).or_default();
        facts
            .unread
            .insert(kind.clone(), u64::try_from(unread).unwrap_or(0));
        facts.kinds.insert(kind);
    }
    add_settings(&mut kinds, store::nodes_for_user(db, user_id).await?);
    Ok(Had { kinds })
}

/// Each source's settings into its facts; a kind set is a kind listed.
fn add_settings(kinds: &mut HashMap<Option<Uuid>, Facts>, set: UserNodes) {
    for (source, nodes) in set {
        let facts = kinds.entry(source).or_default();
        for node in nodes.keys() {
            if let Node::Kind(kind) = node {
                facts.kinds.insert(kind.clone());
            }
        }
        facts.nodes = nodes;
    }
}

/// The app sources to list, by name, with their org's slug and catalogue.
///
/// A bounded number of queries however many apps or org members there are:
/// the person's own memberships once, the candidates in one, who may tell
/// them decided for every candidate together (`audience::told_by_many`), and
/// every live catalogue in one (`live::load_many`). Never an org's roster per
/// app: this is re-read on every visit to the page and to Settings.
async fn apps_for(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    blocked: &HashSet<Uuid>,
    had: &Had,
) -> Result<Vec<(apps::Model, String, SourceCatalogue)>, DbErr> {
    let had_sources = had.sources();
    let reach = PersonReach::load(db, user.id).await?;
    let rows: Vec<apps::Model> = super::audience::candidates(db, &reach, &had_sources)
        .await?
        .into_iter()
        .filter(|a| !blocked.contains(&a.org_id) && user.reaches_org(a.org_id))
        .collect();
    // Only an app that can still tell this person something is listed.
    let told = super::audience::told_by_many(db, user.id, &reach, &rows).await?;
    let rows: Vec<apps::Model> = rows.into_iter().filter(|a| told.contains(&a.id)).collect();
    let mut catalogues = super::live::load_many(db, &rows).await?;
    // Received from or set: listed. Otherwise only for a catalogue to show.
    let kept: Vec<(apps::Model, SourceCatalogue)> = rows
        .into_iter()
        .filter_map(|app| {
            let catalogue = catalogues
                .remove(&app.id)
                .unwrap_or_else(|| super::live::from_manifest(None));
            let listed = had_sources.contains(&app.id) || !catalogue.catalogue.is_empty();
            listed.then_some((app, catalogue))
        })
        .collect();
    let slugs = org_slugs(db, kept.iter().map(|(a, _)| a.org_id)).await?;
    let mut out: Vec<_> = kept
        .into_iter()
        .map(|(app, catalogue)| {
            let slug = slugs.get(&app.org_id).cloned().unwrap_or_default();
            (app, slug, catalogue)
        })
        .collect();
    out.sort_by(|a, b| a.0.name.cmp(&b.0.name).then(a.0.id.cmp(&b.0.id)));
    Ok(out)
}

async fn org_slugs(
    db: &DatabaseConnection,
    orgs: impl Iterator<Item = Uuid>,
) -> Result<HashMap<Uuid, String>, DbErr> {
    let ids: BTreeSet<Uuid> = orgs.collect();
    Ok(organizations::Entity::find()
        .filter(organizations::Column::Id.is_in(ids))
        .all(db)
        .await?
        .into_iter()
        .map(|o| (o.id, o.slug))
        .collect())
}
