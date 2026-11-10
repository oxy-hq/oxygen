//! The `nodes` of a settings patch: each one checked against its source's
//! catalogue, then written under the person's lock.
//!
//! The rules are the old mute route's, generalised to "on" and floors:
//! setting a value needs a node the source has (a kind it names, sends or
//! the person already received or set; a declared or implied path) and a
//! channel some kind under it is delivered on; turning one off needs a kind
//! under it a mute can reach. Setting a value back to inherit (`null`) only
//! needs a well-formed node, so a setting the catalogue has since dropped can
//! still be cleared.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use oxy_auth::types::AuthenticatedUser;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, EntityTrait};
use serde::Deserialize;
use uuid::Uuid;

use super::model::Node;
use super::patch::{PatchError, present};
use super::store;
use crate::catalog::model::{Level, is_within, valid_path};
use crate::catalog::platform::oxygen;
use crate::catalog::source::{KindInfo, SourceCatalogue};
use crate::delivery::Channel;
use crate::input::{is_reserved_kind, valid_kind};
use crate::platform::kinds::is_platform_kind;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodePatch {
    /// `null` for Oxygen; required.
    #[serde(default, deserialize_with = "present")]
    pub app_id: Option<Option<Uuid>>,
    /// `""` for the whole source, or a category path. Exactly one of
    /// `scope` and `kind`.
    pub scope: Option<String>,
    pub kind: Option<String>,
    /// `null` sets a channel back to inherit.
    #[serde(default)]
    pub channels: BTreeMap<String, Option<bool>>,
    #[serde(default, deserialize_with = "present")]
    pub min_level: Option<Option<String>>,
}

/// One node's changes, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Change {
    app: Option<Uuid>,
    node: Node,
    channels: Vec<(Channel, Option<bool>)>,
    min_level: Option<Option<Level>>,
}

impl Change {
    fn parse(p: &NodePatch) -> Result<Self, PatchError> {
        let bad = PatchError::BadRequest;
        let app = p.app_id.ok_or(bad("invalid_body"))?;
        let node = match (&p.scope, &p.kind) {
            (Some(scope), None) => Node::Scope(scope.clone()),
            (None, Some(kind)) => Node::Kind(kind.clone()),
            _ => return Err(bad("unknown_node")),
        };
        let channels = p
            .channels
            .iter()
            .map(|(name, value)| Channel::parse(name).map(|c| (c, *value)))
            .collect::<Option<Vec<_>>>()
            .ok_or(bad("invalid_body"))?;
        let min_level = match &p.min_level {
            None => None,
            Some(None) => Some(None),
            Some(Some(level)) => Some(Some(Level::parse(level).ok_or(bad("bad_level"))?)),
        };
        Ok(Self {
            app,
            node,
            channels,
            min_level,
        })
    }

    /// Whether this sets anything, rather than only clearing.
    fn sets(&self) -> bool {
        self.channels.iter().any(|(_, v)| v.is_some()) || matches!(self.min_level, Some(Some(_)))
    }

    /// `later` folded in: a channel or floor it names replaces this one's.
    fn absorb(&mut self, later: Change) {
        for (channel, value) in later.channels {
            match self.channels.iter_mut().find(|(c, _)| *c == channel) {
                Some(slot) => slot.1 = value,
                None => self.channels.push((channel, value)),
            }
        }
        if later.min_level.is_some() {
            self.min_level = later.min_level;
        }
    }
}

/// One change per (source, node), in first-seen order: a node named twice
/// is the two folded together, the later winning per channel and floor.
fn merged(changes: Vec<Change>) -> Vec<Change> {
    let mut out: Vec<Change> = Vec::new();
    let mut at: HashMap<(Option<Uuid>, Node), usize> = HashMap::new();
    for change in changes {
        match at.entry((change.app, change.node.clone())) {
            Entry::Occupied(i) => out[*i.get()].absorb(change),
            Entry::Vacant(slot) => {
                slot.insert(out.len());
                out.push(change);
            }
        }
    }
    out
}

/// What a source has, for checking a node of it.
struct Source {
    catalogue: SourceCatalogue,
    /// Every kind it names, sends, or the person received or set.
    kinds: BTreeSet<String>,
}

/// Parse, merge and check every node of the patch, before any lock is
/// taken: the catalogues and the person's reach are read here, on `db`, so
/// the transaction that writes holds one connection and does no reads but
/// its own nodes.
pub(super) async fn prepare(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    patches: &[NodePatch],
) -> Result<Vec<Change>, PatchError> {
    let changes = merged(
        patches
            .iter()
            .map(Change::parse)
            .collect::<Result<_, _>>()?,
    );
    let mut sources: HashMap<Option<Uuid>, Source> = HashMap::new();
    for change in &changes {
        let source = match sources.entry(change.app) {
            Entry::Occupied(known) => known.into_mut(),
            Entry::Vacant(slot) => slot.insert(
                load(db, user, change.app)
                    .await?
                    .ok_or(PatchError::BadRequest("unknown_node"))?,
            ),
        };
        check(source, change)?;
    }
    Ok(changes)
}

/// Write `changes` for `user_id`, under the person's lock held by `txn`.
pub(super) async fn write(
    txn: &impl ConnectionTrait,
    user_id: Uuid,
    changes: &[Change],
) -> Result<(), DbErr> {
    for change in changes {
        let mut next = store::node(txn, user_id, change.app, &change.node).await?;
        for (channel, value) in &change.channels {
            next.set_channel(*channel, *value);
        }
        if let Some(level) = change.min_level {
            next.min_level = level;
        }
        store::put_node(txn, user_id, change.app, &change.node, &next).await?;
    }
    Ok(())
}

/// Source `app` (`None`: Oxygen) as `user` may set it; `None` for an app
/// they cannot be told by, one in an org that blocked the token, or one
/// that does not exist — the same answer for each.
async fn load(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    app: Option<Uuid>,
) -> Result<Option<Source>, DbErr> {
    let catalogue = match app {
        None => oxygen(),
        Some(id) => {
            let Some(app) = entity::apps::Entity::find_by_id(id)
                .one(db)
                .await?
                .filter(|a| user.reaches_org(a.org_id))
                .filter(|a| !user.blocked_orgs().contains(&a.org_id))
            else {
                return Ok(None);
            };
            if !crate::preferences::may_be_told_by(db, user.id, &app).await? {
                return Ok(None);
            }
            crate::catalog::live::load(db, &app).await?
        }
    };
    let mut known = crate::scope_mutes::received_kinds(db, user.id, app).await?;
    let set = store::nodes_for_user(db, user.id).await?;
    for node in set.get(&app).into_iter().flat_map(|n| n.keys()) {
        if let Node::Kind(kind) = node {
            known.push(kind.clone());
        }
    }
    Ok(Some(Source {
        kinds: catalogue.kinds_with(&known),
        catalogue,
    }))
}

fn check(source: &Source, change: &Change) -> Result<(), PatchError> {
    match &change.node {
        Node::Kind(kind) => check_kind(source, change, kind),
        Node::Scope(scope) => check_scope(source, change, scope),
    }
}

fn check_kind(source: &Source, change: &Change, kind: &str) -> Result<(), PatchError> {
    let unknown = PatchError::BadRequest("unknown_node");
    let well_formed = match change.app {
        Some(_) => valid_kind(kind) && !is_reserved_kind(kind),
        None => is_platform_kind(kind),
    };
    if !well_formed {
        return Err(unknown);
    }
    if !change.sets() {
        return Ok(());
    }
    let info = source.catalogue.describe(kind);
    if !source.kinds.contains(kind) || !info.has_switch() {
        return Err(unknown);
    }
    for (channel, value) in &change.channels {
        verdict(std::slice::from_ref(&info), *channel, *value)?;
    }
    Ok(())
}

fn check_scope(source: &Source, change: &Change, scope: &str) -> Result<(), PatchError> {
    let unknown = PatchError::BadRequest("unknown_node");
    if !(scope.is_empty() || valid_path(scope)) {
        return Err(unknown);
    }
    if !change.sets() {
        return Ok(());
    }
    if !(scope.is_empty() || source.catalogue.knows_path(scope)) {
        return Err(unknown);
    }
    let inside: Vec<KindInfo> = source
        .kinds
        .iter()
        .map(|k| source.catalogue.describe(k))
        .filter(|k| is_within(&k.category, scope))
        .collect();
    // A declared category with no kind yet can be set ahead of them.
    if inside.is_empty() {
        return Ok(());
    }
    for (channel, value) in &change.channels {
        verdict(&inside, *channel, *value)?;
    }
    Ok(())
}

/// Whether setting `channel` to `value` over `kinds` can mean anything:
/// some kind is delivered on it, and for "off", one a mute reaches.
fn verdict(kinds: &[KindInfo], channel: Channel, value: Option<bool>) -> Result<(), PatchError> {
    let Some(value) = value else {
        return Ok(());
    };
    let uses = |k: &&KindInfo| k.channels.contains(&channel.as_str());
    if !kinds.iter().any(|k| uses(&k)) {
        return Err(PatchError::BadRequest("channel_not_used"));
    }
    if !value && !kinds.iter().filter(uses).any(|k| k.mutable) {
        return Err(PatchError::BadRequest("critical_only"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "patch_nodes_tests.rs"]
mod tests;
