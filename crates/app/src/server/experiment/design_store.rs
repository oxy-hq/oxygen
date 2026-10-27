//! The only place `experiments` and `experiment_designs` are written.
//!
//! An experiment is a name in a workspace with exactly one design: a draft,
//! overwritten in place, until Register freezes it (`frozen_at`). Every write
//! locks the design row first, so an edit and a registration of the same
//! experiment serialize, and an editor who read an older draft (`base_hash`)
//! is refused rather than silently overwriting someone else's save. The
//! database refuses any change to a frozen row too (a trigger), so nothing a run
//! is pinned to can move under it. A frozen design is duplicated, never edited.

use chrono::{DateTime, Utc};
use entity::{experiment_designs, experiments};
use oxy_experiment::{ExperimentConfig, validate_static};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseConnection,
    DatabaseTransaction, DbErr, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, QuerySelect,
    TransactionTrait,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum DesignError {
    #[error("no experiment named `{0}`")]
    NotFound(String),
    #[error("an experiment named `{0}` already exists")]
    Duplicate(String),
    #[error("the design is not valid: {}", .0.join("; "))]
    Invalid(Vec<String>),
    /// The caller edited a draft that has been saved since it was read.
    #[error(
        "the design was changed since you read it (you read {given}, it is now {current}); reload and edit again"
    )]
    Stale { given: String, current: String },
    #[error(
        "experiment `{0}` is registered, so its design is frozen and never changes; duplicate it as a new draft instead"
    )]
    Frozen(String),
    #[error("database: {0}")]
    Db(#[from] DbErr),
}

/// The stored design, decoded.
#[derive(Debug, Clone)]
pub struct Design {
    pub row: experiment_designs::Model,
    pub config: ExperimentConfig,
}

/// An edit of the draft.
pub struct Edit {
    pub config: ExperimentConfig,
    /// The `content_hash` the editor read: `None` = whatever it is now. A
    /// `Some` that is no longer current is [`DesignError::Stale`].
    pub base_hash: Option<String>,
    pub updated_by: Uuid,
    /// The editor holds `ExperimentRegister` (workspace admin): only then may an
    /// edit reopen a refused retrospective, which deletes that admin-created run.
    pub may_reopen: bool,
}

/// sha256 of `config` as canonical JSON: object keys sorted at every depth, no
/// whitespace. Independent of serde_json's map order.
pub fn content_hash(config: &ExperimentConfig) -> Result<String, DbErr> {
    let v = serde_json::to_value(config).map_err(|e| DbErr::Custom(e.to_string()))?;
    let mut s = String::new();
    canonical(&v, &mut s);
    Ok(hex::encode(Sha256::digest(s.as_bytes())))
}

fn canonical(v: &Value, out: &mut String) {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                canonical(&m[k], out);
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(x, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// Every refusal decidable from the design alone. A draft passes; what makes it
/// registrable is registration's business.
fn validate(config: &ExperimentConfig, name: &str) -> Result<(), DesignError> {
    let mut problems: Vec<String> = validate_static(config)
        .iter()
        .map(ToString::to_string)
        .collect();
    if config.experiment != name {
        problems.push(format!(
            "`experiment` is `{}`, but this is experiment `{name}`: a design cannot rename its experiment",
            config.experiment
        ));
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(DesignError::Invalid(problems))
    }
}

pub fn decode(row: experiment_designs::Model) -> Result<Design, String> {
    let config = serde_json::from_value(row.config.clone())
        .map_err(|e| format!("design {} does not read as an experiment: {e}", row.id))?;
    Ok(Design { row, config })
}

pub async fn experiment_by_name(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    name: &str,
) -> Result<Option<experiments::Model>, DbErr> {
    experiments::Entity::find()
        .filter(experiments::Column::WorkspaceId.eq(workspace_id))
        .filter(experiments::Column::Name.eq(name))
        .one(db)
        .await
}

pub async fn experiment_by_id(
    db: &impl ConnectionTrait,
    id: Uuid,
) -> Result<Option<experiments::Model>, DbErr> {
    experiments::Entity::find_by_id(id).one(db).await
}

/// The workspace's experiments, by name.
pub async fn list_experiments(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Vec<experiments::Model>, DbErr> {
    experiments::Entity::find()
        .filter(experiments::Column::WorkspaceId.eq(workspace_id))
        .order_by_asc(experiments::Column::Name)
        .all(db)
        .await
}

/// A new experiment and its draft, in one transaction. The name, owner and
/// hypothesis are the design's own.
pub async fn create_experiment(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    config: ExperimentConfig,
    created_by: Uuid,
) -> Result<(experiments::Model, Design), DesignError> {
    let name = config.experiment.clone();
    validate(&config, &name)?;
    let txn = db.begin().await?;
    let created = insert_experiment(&txn, workspace_id, config, created_by).await?;
    txn.commit().await?;
    Ok(created)
}

async fn insert_experiment(
    txn: &DatabaseTransaction,
    workspace_id: Uuid,
    config: ExperimentConfig,
    created_by: Uuid,
) -> Result<(experiments::Model, Design), DesignError> {
    let name = config.experiment.clone();
    if experiment_by_name(txn, workspace_id, &name)
        .await?
        .is_some()
    {
        return Err(DesignError::Duplicate(name));
    }
    let now = Utc::now().fixed_offset();
    let experiment = experiments::ActiveModel {
        id: Set(Uuid::new_v4()),
        workspace_id: Set(workspace_id),
        name: Set(name.clone()),
        hypothesis: Set(config.hypothesis.clone()),
        owner: Set(config.owner.clone()),
        created_by: Set(created_by),
        created_at: Set(now),
    }
    .insert(txn)
    .await
    .map_err(|e| duplicate_or(e, &name))?;
    let row = experiment_designs::ActiveModel {
        id: Set(Uuid::new_v4()),
        experiment_id: Set(experiment.id),
        from_proposal_id: Set(None),
        config: Set(to_json(&config)?),
        content_hash: Set(content_hash(&config)?),
        created_by: Set(created_by),
        created_at: Set(now),
        updated_by: Set(created_by),
        updated_at: Set(now),
        frozen_at: Set(None),
    }
    .insert(txn)
    .await?;
    Ok((experiment, Design { row, config }))
}

fn to_json(config: &ExperimentConfig) -> Result<Value, DbErr> {
    serde_json::to_value(config).map_err(|e| DbErr::Custom(e.to_string()))
}

/// The unique `(workspace_id, name)` raced a concurrent create.
fn duplicate_or(e: DbErr, name: &str) -> DesignError {
    if e.to_string().contains("experiments_workspace_name_key") {
        DesignError::Duplicate(name.to_string())
    } else {
        DesignError::Db(e)
    }
}

/// Overwrite `name`'s draft with `edit`. The draft's exact content again is no
/// write, and the draft is returned as it is.
pub async fn update_draft(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    name: &str,
    edit: Edit,
) -> Result<Design, DesignError> {
    validate(&edit.config, name)?;
    let txn = db.begin().await?;
    let (experiment, row) = lock_design(&txn, workspace_id, name).await?;
    // Without `may_reopen` a refused retrospective stays frozen: a 409, not a deleted run.
    let row = if edit.may_reopen {
        super::reopen::reopen_refused(&txn, experiment.id, &row)
            .await?
            .unwrap_or(row)
    } else {
        row
    };
    let row = editable(row, name, edit.base_hash.as_deref())?;
    let design = write_draft(&txn, row, edit.config, edit.updated_by, None).await?;
    txn.commit().await?;
    Ok(design)
}

/// `row`, if it is a draft read at `base_hash` (or any hash when `None`).
pub(super) fn editable(
    row: experiment_designs::Model,
    name: &str,
    base_hash: Option<&str>,
) -> Result<experiment_designs::Model, DesignError> {
    if row.frozen_at.is_some() {
        return Err(DesignError::Frozen(name.to_string()));
    }
    if let Some(given) = base_hash.filter(|h| *h != row.content_hash) {
        return Err(DesignError::Stale {
            given: given.to_string(),
            current: row.content_hash,
        });
    }
    Ok(row)
}

/// The experiment and its design, the design row locked for the rest of `txn`:
/// edits, adoptions and registration of one experiment run one at a time.
pub(super) async fn lock_design(
    txn: &DatabaseTransaction,
    workspace_id: Uuid,
    name: &str,
) -> Result<(experiments::Model, experiment_designs::Model), DesignError> {
    let not_found = || DesignError::NotFound(name.to_string());
    let experiment = experiment_by_name(txn, workspace_id, name)
        .await?
        .ok_or_else(not_found)?;
    let row = experiment_designs::Entity::find()
        .filter(experiment_designs::Column::ExperimentId.eq(experiment.id))
        .lock_exclusive()
        .one(txn)
        .await?
        .ok_or_else(not_found)?;
    Ok((experiment, row))
}

/// Overwrite the draft `row` (locked by [`lock_design`]) with `config`.
pub(super) async fn write_draft(
    txn: &DatabaseTransaction,
    row: experiment_designs::Model,
    config: ExperimentConfig,
    updated_by: Uuid,
    from_proposal_id: Option<Uuid>,
) -> Result<Design, DesignError> {
    let hash = content_hash(&config)?;
    if from_proposal_id.is_none() && hash == row.content_hash {
        return decode(row).map_err(|e| DesignError::Db(DbErr::Custom(e)));
    }
    // A hand edit of the drawn split leaves a split no proposal drew.
    let hand_edited_split = from_proposal_id.is_none()
        && row.from_proposal_id.is_some()
        && decode(row.clone())
            .map(|d| super::split_edit::split_changed(&d.config, &config))
            .unwrap_or(true);
    let mut active = row.into_active_model();
    active.config = Set(to_json(&config)?);
    active.content_hash = Set(hash);
    active.updated_by = Set(updated_by);
    active.updated_at = Set(Utc::now().fixed_offset());
    if from_proposal_id.is_some() || hand_edited_split {
        active.from_proposal_id = Set(from_proposal_id);
    }
    let row = active.update(txn).await?;
    Ok(Design { row, config })
}

/// Freeze the locked `row` at `at`: the pre-registration proof. After this the
/// database refuses any change to it.
pub(super) async fn freeze(
    txn: &DatabaseTransaction,
    row: experiment_designs::Model,
    at: DateTime<Utc>,
) -> Result<experiment_designs::Model, DbErr> {
    let mut active = row.into_active_model();
    active.frozen_at = Set(Some(at.fixed_offset()));
    active.update(txn).await
}

/// A new experiment `new_name` whose draft is `name`'s design, renamed. The way
/// to change a registered experiment: it stays as it was, the copy is editable.
pub async fn duplicate(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    name: &str,
    new_name: &str,
    created_by: Uuid,
) -> Result<(experiments::Model, Design), DesignError> {
    let txn = db.begin().await?;
    let experiment = experiment_by_name(&txn, workspace_id, name)
        .await?
        .ok_or_else(|| DesignError::NotFound(name.to_string()))?;
    let row = design_of(&txn, experiment.id)
        .await?
        .ok_or_else(|| DesignError::NotFound(name.to_string()))?;
    let mut config = decode(row)
        .map_err(|e| DesignError::Db(DbErr::Custom(e)))?
        .config;
    config.experiment = new_name.to_string();
    validate(&config, new_name)?;
    let created = insert_experiment(&txn, workspace_id, config, created_by).await?;
    txn.commit().await?;
    Ok(created)
}

/// The experiment's design.
pub async fn design_of(
    db: &impl ConnectionTrait,
    experiment_id: Uuid,
) -> Result<Option<experiment_designs::Model>, DbErr> {
    experiment_designs::Entity::find()
        .filter(experiment_designs::Column::ExperimentId.eq(experiment_id))
        .one(db)
        .await
}

/// The designs of `experiment_ids` — each experiment has one — by id: one
/// query for a list.
pub async fn designs_of(
    db: &impl ConnectionTrait,
    experiment_ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, experiment_designs::Model>, DbErr> {
    if experiment_ids.is_empty() {
        return Ok(Default::default());
    }
    Ok(experiment_designs::Entity::find()
        .filter(experiment_designs::Column::ExperimentId.is_in(experiment_ids.to_vec()))
        .all(db)
        .await?
        .into_iter()
        .map(|d| (d.id, d))
        .collect())
}

pub async fn design_by_id(
    db: &impl ConnectionTrait,
    id: Uuid,
) -> Result<Option<experiment_designs::Model>, DbErr> {
    experiment_designs::Entity::find_by_id(id).one(db).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::experiment::test_design::design;

    #[test]
    fn the_hash_ignores_key_order() {
        let a: Value =
            serde_json::from_str(r#"{"b":1,"a":{"y":[1,{"q":2,"p":3}],"x":null}}"#).unwrap();
        let b: Value =
            serde_json::from_str(r#"{"a":{"x":null,"y":[1,{"p":3,"q":2}]},"b":1}"#).unwrap();
        let (mut sa, mut sb) = (String::new(), String::new());
        canonical(&a, &mut sa);
        canonical(&b, &mut sb);
        assert_eq!(sa, sb);
        assert_eq!(sa, r#"{"a":{"x":null,"y":[1,{"p":3,"q":2}]},"b":1}"#);
    }

    #[test]
    fn a_changed_design_hashes_differently() {
        let c = design();
        let mut d = c.clone();
        d.design.seed = Some(1);
        assert_eq!(content_hash(&c).unwrap(), content_hash(&c).unwrap());
        assert_ne!(content_hash(&c).unwrap(), content_hash(&d).unwrap());
    }

    #[test]
    fn a_design_cannot_rename_its_experiment() {
        let Err(DesignError::Invalid(problems)) = validate(&design(), "other") else {
            panic!("a mismatched name is refused");
        };
        assert!(problems[0].contains("cannot rename"), "{problems:?}");
        assert!(validate(&design(), "beverage_promo_pilot").is_ok());
    }
}
