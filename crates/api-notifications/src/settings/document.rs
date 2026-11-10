//! The settings document, `GET /api/notifications/settings`: the person's
//! own settings, every source the catalogue lists as a tree with what is set
//! at each node and each kind's `effective` answer per channel, the devices,
//! and which channels can reach the person at all.
//!
//! Every field is always present, `null` when unset. `effective` is
//! [`decide`]'s answer now, with no device: what the person's settings do,
//! before any device's own.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use entity::{org_members, slack_installations, slack_user_links, users};
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, PaginatorTrait, QueryFilter};
use serde::Serialize;
use uuid::Uuid;

use super::decide::{KindFacts, Reason, Settings, decide};
use super::model::{GlobalSettings, GlobalView, Node, NodeSettingsView, OsPermission};
use super::store;
use crate::catalog::handler::{Listed, SourceApp, listed_sources};
use crate::catalog::model::Level;
use crate::catalog::source::{KindInfo, SourceCatalogue};
use crate::catalog::tree::{Facts, children, shape};
use crate::delivery::Channel;
use crate::devices::{DeviceView, list_for};

#[derive(Debug, Serialize)]
pub struct NotificationSettings {
    /// Opaque; changes on every write. Also the `ETag`.
    pub version: String,
    pub global: GlobalView,
    pub sources: Vec<SettingsSource>,
    pub devices: Vec<DeviceView>,
    pub reach: SettingsReach,
}

#[derive(Debug, Serialize)]
pub struct SettingsSource {
    /// `oxygen` | `app`.
    pub source: &'static str,
    pub app: Option<SourceApp>,
    pub label: String,
    pub settings: NodeSettingsView,
    pub categories: Vec<SettingsCategory>,
    pub unread: u64,
}

#[derive(Debug, Serialize)]
pub struct SettingsCategory {
    pub path: String,
    pub label: String,
    pub description: Option<String>,
    pub settings: NodeSettingsView,
    pub categories: Vec<SettingsCategory>,
    pub kinds: Vec<SettingsKind>,
    pub unread: u64,
}

#[derive(Debug, Serialize)]
pub struct SettingsKind {
    pub kind: String,
    pub label: String,
    pub description: Option<String>,
    pub level: Level,
    pub critical: bool,
    /// The channels it can ever use.
    pub channels: Vec<&'static str>,
    pub settings: NodeSettingsView,
    /// Every channel, decided now.
    pub effective: BTreeMap<&'static str, Effective>,
    pub unread: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Effective {
    pub on: bool,
    pub reason: Option<Reason>,
    /// The label of the node that decided it.
    pub from: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SettingsReach {
    /// Devices push can reach: enabled, and not denied by their OS.
    pub push_devices: u64,
    pub email: bool,
    /// Linked to the Oxygen bot in an org's Slack, or findable there: an
    /// org they are a member of has it installed and they have an email.
    pub slack: bool,
}

/// The whole document for `user`, at `now`.
pub async fn document_for(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
) -> Result<NotificationSettings, DbErr> {
    let (global, version) = store::person(db, user.id).await?;
    let now = Utc::now();
    let sources = listed_sources(db, user)
        .await?
        .into_iter()
        .map(|listed| source(listed, &global, now))
        .collect();
    let devices = list_for(db, user.id).await?;
    let push_devices = devices
        .iter()
        .filter(|d| d.enabled && d.os_permission != Some(OsPermission::Denied))
        .count() as u64;
    Ok(NotificationSettings {
        version: version.to_string(),
        global: global.view(),
        sources,
        devices,
        reach: reach(db, user.id, push_devices).await?,
    })
}

/// One source's tree, with what is set at each node and each kind decided.
pub fn source(listed: Listed, global: &GlobalSettings, now: DateTime<Utc>) -> SettingsSource {
    let tree = Tree {
        catalogue: &listed.catalogue,
        facts: &listed.facts,
        global,
        source_label: &listed.label,
        now,
    };
    let (kinds, paths) = shape(&listed.catalogue, &listed.facts);
    let categories = children(&paths, None)
        .into_iter()
        .map(|p| tree.category(&paths, &kinds, &p))
        .collect();
    SettingsSource {
        source: listed.catalogue.source.as_str(),
        settings: listed.facts.at(&Node::source()).view(),
        app: listed.app,
        label: listed.label.clone(),
        categories,
        unread: listed.facts.unread.values().sum(),
    }
}

struct Tree<'a> {
    catalogue: &'a SourceCatalogue,
    facts: &'a Facts,
    global: &'a GlobalSettings,
    source_label: &'a str,
    now: DateTime<Utc>,
}

impl Tree<'_> {
    fn category(
        &self,
        paths: &BTreeSet<String>,
        kinds: &[KindInfo],
        path: &str,
    ) -> SettingsCategory {
        let categories: Vec<SettingsCategory> = children(paths, Some(path))
            .into_iter()
            .map(|p| self.category(paths, kinds, &p))
            .collect();
        let mut own: Vec<SettingsKind> = kinds
            .iter()
            .filter(|k| k.category == path)
            .map(|k| self.kind(k))
            .collect();
        own.sort_by(|a, b| a.label.cmp(&b.label).then(a.kind.cmp(&b.kind)));
        let unread = categories.iter().map(|c| c.unread).sum::<u64>()
            + own.iter().map(|k| k.unread).sum::<u64>();
        SettingsCategory {
            path: path.to_string(),
            label: self.catalogue.label_of(path),
            description: self.catalogue.description_of(path),
            settings: self.facts.at(&Node::Scope(path.to_string())).view(),
            categories,
            kinds: own,
            unread,
        }
    }

    fn kind(&self, info: &KindInfo) -> SettingsKind {
        let settings = Settings::for_kind(
            self.global,
            Some(&self.facts.nodes),
            info,
            self.source_label,
        );
        let facts = KindFacts::of(info, false);
        let effective = Channel::ALL
            .into_iter()
            .map(|channel| {
                let d = decide(&settings, &facts, channel, None, self.now);
                let effective = Effective {
                    on: d.deliver,
                    reason: d.reason,
                    from: d.from,
                };
                (channel.as_str(), effective)
            })
            .collect();
        SettingsKind {
            kind: info.kind.clone(),
            label: info.label.clone(),
            description: info.description.clone(),
            level: info.level,
            critical: info.critical(),
            channels: info.channels.clone(),
            settings: self.facts.at(&Node::Kind(info.kind.clone())).view(),
            effective,
            unread: self.facts.unread.get(&info.kind).copied().unwrap_or(0),
        }
    }
}

/// Whether email and Slack can reach `user_id` at all.
async fn reach(
    db: &DatabaseConnection,
    user_id: Uuid,
    push_devices: u64,
) -> Result<SettingsReach, DbErr> {
    let email = users::Entity::find_by_id(user_id)
        .one(db)
        .await?
        .filter(|u| u.status == users::UserStatus::Active)
        .and_then(|u| u.email)
        .is_some_and(|e| !e.trim().is_empty());
    let installations: Vec<Uuid> = slack_user_links::Entity::find()
        .filter(slack_user_links::Column::OxyUserId.eq(user_id))
        .all(db)
        .await?
        .into_iter()
        .map(|l| l.installation_id)
        .collect();
    let orgs: Vec<Uuid> = match email {
        true => org_members::Entity::find()
            .filter(org_members::Column::UserId.eq(user_id))
            .all(db)
            .await?
            .into_iter()
            .map(|m| m.org_id)
            .collect(),
        false => Vec::new(),
    };
    let slack = (!installations.is_empty() || !orgs.is_empty())
        && slack_installations::Entity::find()
            .filter(slack_installations::Column::RevokedAt.is_null())
            .filter(
                sea_orm::Condition::any()
                    .add(slack_installations::Column::Id.is_in(installations))
                    .add(slack_installations::Column::OrgId.is_in(orgs)),
            )
            .count(db)
            .await?
            > 0;
    Ok(SettingsReach {
        push_devices,
        email,
        slack,
    })
}
