//! Which widgets a person sees: the apps they can open, read from each app's
//! production build.

use std::collections::{HashMap, HashSet};

use entity::prelude::{AppBuilds, Apps, Organizations};
use entity::{app_builds, apps, organizations};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_app_core::custom_apps_host_dispatch::environment_url_for;
use oxy_app_core::widgets::layout::CatalogEntry;
use oxy_app_core::widgets::model::WidgetDef;
use oxy_app_core::widgets::parse::widgets;
use oxy_authz::PrincipalFacts;
use oxy_shared::utils::custom_app_url::build_pretty_url;
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QuerySelect};
use serde::Serialize;
use uuid::Uuid;

use crate::me_apps::{candidate_orgs, keep_openable};

/// The app a widget belongs to, as the home names and links it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WidgetApp {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub org_slug: String,
    /// Production: the app's own host when the deployment has a custom-apps
    /// zone, else its path route. Widget links resolve against it.
    pub url: String,
}

/// One app and the widgets its production build declares.
#[derive(Debug, Clone)]
pub struct AppWidgets {
    pub app: WidgetApp,
    pub widgets: Vec<WidgetDef>,
}

/// The id of the org with slug `org_slug`, when the caller belongs to it. An
/// org they do not belong to is `None`, however wide their staff reach: the
/// home is "my apps", as `GET /api/me/apps` is, and its layout is the org's own
/// business.
pub async fn member_org_id(
    db: &DatabaseConnection,
    facts: &PrincipalFacts,
    org_slug: &str,
) -> Result<Option<Uuid>, DbErr> {
    let org_id = Organizations::find()
        .select_only()
        .column(organizations::Column::Id)
        .filter(organizations::Column::Slug.eq(org_slug))
        .into_tuple::<Uuid>()
        .one(db)
        .await?;
    Ok(org_id.filter(|id| candidate_orgs(facts).contains(id)))
}

/// The published apps in org `org_id` that `facts` may open.
pub async fn openable_in_org(
    db: &DatabaseConnection,
    facts: &PrincipalFacts,
    org_id: Uuid,
) -> Result<Vec<apps::Model>, DbErr> {
    let rows = Apps::find()
        .filter(apps::Column::OrgId.eq(org_id))
        .filter(apps::Column::PublishedAt.is_not_null())
        .all(db)
        .await?;
    Ok(keep_openable(facts, rows))
}

/// Every published app in `org_id`, whoever asks: the layout is the org's, so
/// it is written and described against all of them.
pub async fn org_apps(db: &DatabaseConnection, org_id: Uuid) -> Result<Vec<apps::Model>, DbErr> {
    Apps::find()
        .filter(apps::Column::OrgId.eq(org_id))
        .filter(apps::Column::PublishedAt.is_not_null())
        .all(db)
        .await
}

/// The widgets a layout names, `<app>/<widget>`, in default order (app name,
/// then manifest order), each with its app.
pub fn catalog(apps: &[AppWidgets]) -> Vec<(CatalogEntry, &WidgetApp)> {
    apps.iter()
        .flat_map(|a| {
            a.widgets
                .iter()
                .map(move |w| (CatalogEntry::new(&a.app.slug, w.clone()), &a.app))
        })
        .collect()
}

/// App `app_id`, when it is published and `facts` may open it.
pub async fn openable_app(
    db: &DatabaseConnection,
    facts: &PrincipalFacts,
    app_id: Uuid,
) -> Result<Option<apps::Model>, DbErr> {
    let Some(app) = Apps::find_by_id(app_id).one(db).await? else {
        return Ok(None);
    };
    if app.published_at.is_none() || !candidate_orgs(facts).contains(&app.org_id) {
        return Ok(None);
    }
    Ok(keep_openable(facts, vec![app]).pop())
}

/// `rows` with the widgets their production builds declare, apps with none
/// left out, sorted by app name. Three queries whatever the number of apps.
pub async fn with_widgets(
    db: &DatabaseConnection,
    rows: Vec<apps::Model>,
) -> Result<Vec<AppWidgets>, DbErr> {
    if rows.is_empty() {
        return Ok(vec![]);
    }
    let environments =
        crate::custom_apps_env_resolve::load_environment_builds_batch(db, &rows).await?;
    // Production's build only: the home links to production, and an action is a
    // write that runs nowhere else.
    let production: HashMap<Uuid, Uuid> = environments
        .iter()
        .filter_map(|(app, env)| Some((*app, env.production?)))
        .collect();
    let manifests = manifests(db, production.values().copied().collect()).await?;
    let orgs = org_slugs(db, rows.iter().map(|a| a.org_id).collect()).await?;
    let mut out: Vec<AppWidgets> = rows
        .into_iter()
        .filter_map(|app| {
            let manifest = manifests.get(production.get(&app.id)?)?;
            let widgets = widgets(Some(manifest));
            let org_slug = orgs.get(&app.org_id)?.clone();
            (!widgets.is_empty()).then(|| AppWidgets {
                app: widget_app(&app, org_slug),
                widgets,
            })
        })
        .collect();
    out.sort_by_cached_key(|a| (a.app.name.to_lowercase(), a.app.id));
    Ok(out)
}

fn widget_app(app: &apps::Model, org_slug: String) -> WidgetApp {
    let url = environment_url_for(&AppEnvironment::Production, &org_slug, &app.slug)
        .unwrap_or_else(|| build_pretty_url(&org_slug, &app.slug));
    WidgetApp {
        id: app.id,
        slug: app.slug.clone(),
        name: app.name.clone(),
        org_slug,
        url,
    }
}

async fn manifests(
    db: &DatabaseConnection,
    builds: HashSet<Uuid>,
) -> Result<HashMap<Uuid, serde_json::Value>, DbErr> {
    if builds.is_empty() {
        return Ok(HashMap::new());
    }
    Ok(AppBuilds::find()
        .select_only()
        .column(app_builds::Column::Id)
        .column(app_builds::Column::ManifestJson)
        .filter(app_builds::Column::Id.is_in(builds))
        .into_tuple::<(Uuid, Option<serde_json::Value>)>()
        .all(db)
        .await?
        .into_iter()
        .filter_map(|(id, manifest)| Some((id, manifest?)))
        .collect())
}

async fn org_slugs(
    db: &DatabaseConnection,
    ids: HashSet<Uuid>,
) -> Result<HashMap<Uuid, String>, DbErr> {
    Ok(Organizations::find()
        .select_only()
        .column(organizations::Column::Id)
        .column(organizations::Column::Slug)
        .filter(organizations::Column::Id.is_in(ids))
        .into_tuple::<(Uuid, String)>()
        .all(db)
        .await?
        .into_iter()
        .collect())
}
