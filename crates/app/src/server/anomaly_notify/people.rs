//! The `notify.people` destination: an `oxy.anomaly` inbox notice per event to
//! the org members a `.monitor.yml` names.
//!
//! It claims under its own ledger channel (`inbox`), so it and the Slack post
//! succeed or fail apart, and each event is announced to the people once. The
//! notice itself is also at most once per (person, event), so a claim lost to
//! a crash and taken again tells nobody twice.
//!
//! `people` is resolved when the task runs, not when the file was compiled: a
//! new admin hears about the next event, and a removed member does not.

use std::collections::{BTreeSet, HashSet};

use async_trait::async_trait;
use entity::org_members::OrgRole;
use entity::{org_team_members, org_teams, users};
use oxy_metric_monitoring::NotifyConfig;
use oxy_metric_monitoring::notify::ledger::CHANNEL_INBOX;
use oxy_metric_monitoring::notify::{Bucket, Destination, Heading, OrgRoleTarget, Person};
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QuerySelect};
use uuid::Uuid;

use oxy_app_core::notifications::platform::people::org_members_from;
use oxy_app_core::notifications::platform::{PlatformNotice, notify};

/// The org members one workspace's `notify.people` names.
pub struct PeopleInbox {
    db: DatabaseConnection,
    org_id: Uuid,
    workspace_id: Uuid,
    people: Vec<Uuid>,
}

impl PeopleInbox {
    /// Resolve `notify.people` against the org now. Entries that name nobody
    /// are logged; a list that names nobody at all is still a destination,
    /// and refuses, so the run says why.
    pub async fn resolve(
        db: &DatabaseConnection,
        org_id: Uuid,
        workspace_id: Uuid,
        notify: &NotifyConfig,
    ) -> Result<Self, String> {
        let targets = notify.people()?;
        let (people, nobody) = named_in_monitor(db, org_id, &targets)
            .await
            .map_err(|e| format!("resolve `notify.people`: {e}"))?;
        for entry in nobody {
            tracing::info!(
                target: "metric_anomalies",
                %workspace_id,
                entry = %entry,
                "notify.people entry names no org member"
            );
        }
        Ok(Self {
            db: db.clone(),
            org_id,
            workspace_id,
            people,
        })
    }

    /// How many people it reaches.
    pub fn reach(&self) -> usize {
        self.people.len()
    }
}

#[async_trait]
impl Destination for PeopleInbox {
    fn id(&self) -> &str {
        "people"
    }

    fn channel(&self) -> &'static str {
        CHANNEL_INBOX
    }

    async fn deliver(
        &self,
        _heading: Heading<'_>,
        buckets: &[Bucket],
    ) -> Result<Option<usize>, String> {
        let events: BTreeSet<Uuid> = buckets.iter().map(|b| b.event_id).collect();
        if events.is_empty() {
            return Ok(None);
        }
        if self.people.is_empty() {
            return Err(
                "`notify.people` names no member of this org: check the emails, roles and team \
                 names in .monitor.yml"
                    .to_string(),
            );
        }
        for event_id in &events {
            let notice = PlatformNotice::Anomaly {
                org_id: self.org_id,
                workspace_id: self.workspace_id,
                event_id: *event_id,
            };
            notify(&self.db, &notice, &self.people)
                .await
                .map_err(|e| format!("write the inbox notices: {e}"))?;
        }
        Ok(Some(events.len()))
    }
}

// Who a `.monitor.yml` names. Moved here from the platform notices' `people`
// (now `oxy_app_core::notifications::platform::people`): `notify.people` is
// the anomaly destination's vocabulary, and its only caller is this file.

/// The org members a `.monitor.yml` `notify.people` list names, and the
/// entries that named nobody (for the log).
async fn named_in_monitor(
    db: &DatabaseConnection,
    org_id: Uuid,
    people: &[Person],
) -> Result<(Vec<Uuid>, Vec<String>), DbErr> {
    let members: HashSet<Uuid> = org_members_from(db, org_id, OrgRole::Member)
        .await?
        .into_iter()
        .collect();
    let mut found = Vec::new();
    let mut nobody = Vec::new();
    for person in people {
        let ids = match person {
            Person::Email(email) => by_email(db, email).await?,
            Person::Role(role) => org_members_from(db, org_id, org_role(*role)).await?,
            Person::Team(name) => team(db, org_id, name).await?,
        };
        let ids: Vec<Uuid> = ids.into_iter().filter(|u| members.contains(u)).collect();
        if ids.is_empty() {
            nobody.push(format!("{person:?}"));
        }
        for id in ids {
            if !found.contains(&id) {
                found.push(id);
            }
        }
    }
    Ok((found, nobody))
}

fn org_role(role: OrgRoleTarget) -> OrgRole {
    match role {
        OrgRoleTarget::Owner => OrgRole::Owner,
        OrgRoleTarget::Admin => OrgRole::Admin,
        OrgRoleTarget::Member => OrgRole::Member,
    }
}

async fn by_email(db: &DatabaseConnection, email: &str) -> Result<Vec<Uuid>, DbErr> {
    use sea_orm::sea_query::{Expr, ExprTrait, Func};
    users::Entity::find()
        .select_only()
        .column(users::Column::Id)
        .filter(
            Expr::expr(Func::lower(Expr::col(users::Column::Email))).is_in([email.to_lowercase()]),
        )
        .into_tuple::<Uuid>()
        .all(db)
        .await
}

async fn team(db: &DatabaseConnection, org_id: Uuid, name: &str) -> Result<Vec<Uuid>, DbErr> {
    use sea_orm::sea_query::{Expr, ExprTrait, Func};
    let Some(team) = org_teams::Entity::find()
        .filter(org_teams::Column::OrgId.eq(org_id))
        .filter(
            Expr::expr(Func::lower(Expr::col(org_teams::Column::Name)))
                .is_in([name.to_lowercase()]),
        )
        .one(db)
        .await?
    else {
        return Ok(Vec::new());
    };
    org_team_members::Entity::find()
        .select_only()
        .column(org_team_members::Column::UserId)
        .filter(org_team_members::Column::TeamId.eq(team.id))
        .into_tuple::<Uuid>()
        .all(db)
        .await
}
