//! What the Apps tab shows of one app, and the pure computation of each
//! action's and each pipeline's status from rows already read.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use entity::app_platform_grants;
use oxy_authz::Action;
use oxy_custom_apps::platform_access::{Asked, declared, describe};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// One action of one app, as the page shows it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AppActionDto {
    /// The action's stable id (`frontline_device_manage`).
    pub action: String,
    /// What it lets the app do, in the platform's words.
    pub description: String,
    /// The live build's functions that ask for it. Empty for an approved
    /// action the live build no longer asks for.
    pub functions: Vec<String>,
    /// `pending` — asked for, not approved (for an upload: any pipeline it
    /// lists is not); `approved` — asked for and approved; `approved_unused` —
    /// approved, no longer asked for (no function can use it until a build
    /// asks again).
    pub status: &'static str,
    /// The most recent approval of it.
    pub approved_at: Option<String>,
    pub approved_by: Option<String>,
    /// What it is asked for and approved on: an upload's pipelines, by name,
    /// each with its own status. Empty for an action on the whole org.
    pub targets: Vec<TargetDto>,
}

/// One pipeline an upload names, as the page shows it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TargetDto {
    pub name: String,
    /// `pending`, `approved` or `approved_unused`, as for an action.
    pub status: &'static str,
    pub approved_at: Option<String>,
    pub approved_by: Option<String>,
}

/// One app's access in this org.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AppAccessDto {
    pub app_id: Uuid,
    pub app_slug: String,
    pub app_name: String,
    /// The live build the asks were read from; `None` for an app with none.
    pub live_build: Option<LiveBuildDto>,
    pub actions: Vec<AppActionDto>,
    /// The app's row in the inventory while it holds a live approval.
    pub token_id: Option<Uuid>,
    /// The service account the app acts as, once an approval made one.
    pub service_account: Option<String>,
    pub last_used_at: Option<String>,
}

/// Who put the live build there, which is who the approver is trusting.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LiveBuildDto {
    pub build_id: String,
    pub published_at: String,
    /// The person who published it, when a person did.
    pub published_by: Option<String>,
    /// How it was published (`oxyc publish`, a GitHub Actions run …).
    pub published_via: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AppAccessList {
    pub apps: Vec<AppAccessDto>,
}

/// `POST …/approve`: the action ids to approve. Absent or empty approves every
/// action the live build asks for.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApproveBody {
    #[serde(default)]
    pub actions: Option<Vec<String>>,
}

pub(super) fn rfc3339(at: chrono::DateTime<chrono::FixedOffset>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

const PENDING: &str = "pending";

/// `pending`, `approved` or `approved_unused`, from whether the live build asks
/// for something and whether it is approved.
fn status(asked: bool, approved: bool) -> &'static str {
    match (asked, approved) {
        (true, false) => PENDING,
        (true, true) => "approved",
        (false, _) => "approved_unused",
    }
}

/// Each action the live build's `manifest` asks for or the live row holds, with
/// its status and, for an upload, each pipeline's. `grants` are the app's live
/// approvals; only those under `token_id`, the app's live row, count.
pub(super) fn actions_of(
    manifest: Option<&Value>,
    grants: &[app_platform_grants::Model],
    token_id: Option<Uuid>,
    people: &HashMap<Uuid, String>,
) -> Vec<AppActionDto> {
    let asked = declared(manifest);
    let live: Vec<&app_platform_grants::Model> = grants
        .iter()
        .filter(|g| token_id == Some(g.token_id) && g.revoked_at.is_none())
        .collect();
    let mut every: BTreeSet<Action> = asked.keys().copied().collect();
    every.extend(
        live.iter()
            .filter_map(|g| Action::from_app_action_id(&g.action)),
    );
    every
        .into_iter()
        .map(|action| action_row(action, &asked, &live, people))
        .collect()
}

fn action_row(
    action: Action,
    asked: &BTreeMap<Action, Asked>,
    live: &[&app_platform_grants::Model],
    people: &HashMap<Uuid, String>,
) -> AppActionDto {
    let id = action.app_action_id().unwrap_or_default();
    let none = Asked::default();
    let asks = asked.get(&action).unwrap_or(&none);
    let grants: Vec<&app_platform_grants::Model> =
        live.iter().copied().filter(|g| g.action == id).collect();
    let targets = target_rows(asks, &grants, people);
    let approved = if asks.targets.is_empty() {
        grants.iter().any(|g| g.target.is_none())
    } else {
        targets.iter().all(|t| t.status != PENDING)
    };
    let latest = grants.iter().max_by_key(|g| g.created_at);
    AppActionDto {
        action: id.to_string(),
        description: describe(action).to_string(),
        functions: asks.functions.clone(),
        status: status(asked.contains_key(&action), approved),
        approved_at: latest.map(|g| rfc3339(g.created_at)),
        approved_by: latest.and_then(|g| approver(people, g)),
        targets,
    }
}

/// One row per pipeline asked for or approved, with its own status.
fn target_rows(
    asks: &Asked,
    grants: &[&app_platform_grants::Model],
    people: &HashMap<Uuid, String>,
) -> Vec<TargetDto> {
    let mut names: BTreeSet<String> = asks.targets.clone();
    names.extend(grants.iter().filter_map(|g| g.target.clone()));
    names
        .into_iter()
        .map(|name| {
            let approval = grants
                .iter()
                .filter(|g| g.target.as_deref() == Some(name.as_str()))
                .max_by_key(|g| g.created_at);
            TargetDto {
                status: status(asks.targets.contains(&name), approval.is_some()),
                approved_at: approval.map(|g| rfc3339(g.created_at)),
                approved_by: approval.and_then(|g| approver(people, g)),
                name,
            }
        })
        .collect()
}

fn approver(people: &HashMap<Uuid, String>, grant: &app_platform_grants::Model) -> Option<String> {
    grant.created_by.and_then(|id| people.get(&id).cloned())
}
