use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "feature_flags")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub key: String,
    /// The legacy global value. While `targeting` is set it holds the registry
    /// default instead, for a binary that predates targeting; the kill switch is
    /// then inside `targeting` (see `server::feature_flags::store`).
    pub enabled: bool,
    pub updated_at: DateTimeWithTimeZone,
    /// The flag's rules, or `None` for a plain global on/off flag.
    pub targeting: Option<Json>,
}

impl ActiveModelBehavior for ActiveModel {}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{DbBackend, EntityTrait, QueryFilter, QueryTrait};

    /// Guards the `#[sea_orm::model]` annotation that every entity in this
    /// crate carries.
    ///
    /// That attribute is the *only* thing that emits the strongly-typed
    /// `COLUMN` constant — `DeriveEntityModel` alone does not, so dropping the
    /// annotation from an entity silently takes `COLUMN` away from it and every
    /// `COLUMN.foo` call site fails to resolve. This asserts the two spellings
    /// still lower to identical SQL, which is what makes them safe to mix: the
    /// migration was additive, and `Column::Enabled` remains valid everywhere.
    ///
    /// Scope: one entity is enough. The annotation expands the same way for all
    /// of them, so this catches a workspace-wide regression (a `sea-orm` bump
    /// that renames or drops the attribute), not this table's shape.
    #[test]
    fn typed_column_and_legacy_column_lower_to_identical_sql() {
        let typed = Entity::find()
            .filter(COLUMN.enabled.eq(true))
            .build(DbBackend::Postgres)
            .to_string();
        let legacy = Entity::find()
            .filter(Column::Enabled.eq(true))
            .build(DbBackend::Postgres)
            .to_string();

        assert_eq!(
            typed, legacy,
            "typed `COLUMN` must lower to the same SQL as `Column::` — if this \
             diverges, the two spellings are no longer interchangeable and the \
             mixed call sites across the workspace are not equivalent"
        );
    }
}
