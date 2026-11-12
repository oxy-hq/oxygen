use chrono::Utc;
use entity::prelude::SlackInstallations;
use entity::slack_installations;
use oxy::adapters::secrets::OrgSecretsService;
use oxy::database::client::establish_connection;
use oxy_shared::errors::OxyError;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter, SqlErr};
use uuid::Uuid;

pub struct InstallationsService;

pub struct UpsertInstallation {
    pub org_id: Uuid,
    pub team_id: String,
    pub team_name: String,
    pub enterprise_id: Option<String>,
    pub bot_user_id: String,
    pub bot_token: String,
    pub scopes: String,
    pub installed_by_user_id: Uuid,
    pub installed_by_slack_user_id: String,
}

impl InstallationsService {
    /// Insert or re-activate an install. Enforces the 1:1 team↔org and org↔team
    /// conflict rules at the application layer so the error message is friendly.
    pub async fn upsert(input: UpsertInstallation) -> Result<slack_installations::Model, OxyError> {
        let conn = establish_connection().await?;

        // Find any active install with this team_id.
        let by_team = SlackInstallations::find()
            .filter(slack_installations::Column::SlackTeamId.eq(&input.team_id))
            .filter(slack_installations::Column::RevokedAt.is_null())
            .one(&conn)
            .await
            .map_err(|e| OxyError::DBError(e.to_string()))?;

        if let Some(ref existing) = by_team
            && existing.org_id != input.org_id
        {
            return Err(OxyError::ValidationError(format!(
                "Slack workspace {} is already connected to a different Oxygen org. \
                     Disconnect it there first.",
                input.team_id
            )));
        }

        // Find any active install for this org.
        let by_org = SlackInstallations::find()
            .filter(slack_installations::Column::OrgId.eq(input.org_id))
            .filter(slack_installations::Column::RevokedAt.is_null())
            .one(&conn)
            .await
            .map_err(|e| OxyError::DBError(e.to_string()))?;

        if let Some(ref existing) = by_org
            && existing.slack_team_id != input.team_id
        {
            return Err(OxyError::ValidationError(format!(
                "Your org is already connected to Slack workspace {}. Disconnect first.",
                existing.slack_team_name
            )));
        }

        let secret_name = format!("slack_bot_token:{}", input.org_id);
        let secret_id =
            OrgSecretsService::upsert(input.org_id, &secret_name, &input.bot_token).await?;

        let row = if let Some(existing) = by_org {
            // Re-install for same org/team: update token + metadata.
            let mut active: slack_installations::ActiveModel = existing.into();
            active.slack_team_name = ActiveValue::Set(input.team_name);
            active.slack_enterprise_id = ActiveValue::Set(input.enterprise_id);
            active.bot_user_id = ActiveValue::Set(input.bot_user_id);
            active.bot_token_secret_id = ActiveValue::Set(secret_id);
            active.bot_scopes = ActiveValue::Set(input.scopes);
            active.installed_by_user_id = ActiveValue::Set(input.installed_by_user_id);
            active.installed_by_slack_user_id = ActiveValue::Set(input.installed_by_slack_user_id);
            active.installed_at = ActiveValue::Set(Utc::now().into());
            active.revoked_at = ActiveValue::Set(None);
            active
                .update(&conn)
                .await
                .map_err(|e| translate_unique_violation(e, &input.team_id, ""))?
        } else {
            slack_installations::ActiveModel {
                id: ActiveValue::Set(Uuid::new_v4()),
                org_id: ActiveValue::Set(input.org_id),
                slack_team_id: ActiveValue::Set(input.team_id.clone()),
                slack_team_name: ActiveValue::Set(input.team_name),
                slack_enterprise_id: ActiveValue::Set(input.enterprise_id),
                bot_user_id: ActiveValue::Set(input.bot_user_id),
                bot_token_secret_id: ActiveValue::Set(secret_id),
                bot_scopes: ActiveValue::Set(input.scopes),
                installed_by_user_id: ActiveValue::Set(input.installed_by_user_id),
                installed_by_slack_user_id: ActiveValue::Set(input.installed_by_slack_user_id),
                installed_at: ActiveValue::NotSet,
                revoked_at: ActiveValue::Set(None),
            }
            .insert(&conn)
            .await
            .map_err(|e| translate_unique_violation(e, &input.team_id, ""))?
        };
        Ok(row)
    }

    pub async fn find_active_by_team(
        team_id: &str,
    ) -> Result<Option<slack_installations::Model>, OxyError> {
        let conn = establish_connection().await?;
        SlackInstallations::find()
            .filter(slack_installations::Column::SlackTeamId.eq(team_id))
            .filter(slack_installations::Column::RevokedAt.is_null())
            .one(&conn)
            .await
            .map_err(|e| OxyError::DBError(e.to_string()))
    }

    pub async fn find_active_by_org(
        org_id: Uuid,
    ) -> Result<Option<slack_installations::Model>, OxyError> {
        let conn = establish_connection().await?;
        SlackInstallations::find()
            .filter(slack_installations::Column::OrgId.eq(org_id))
            .filter(slack_installations::Column::RevokedAt.is_null())
            .one(&conn)
            .await
            .map_err(|e| OxyError::DBError(e.to_string()))
    }

    pub async fn revoke(id: Uuid) -> Result<(), OxyError> {
        let conn = establish_connection().await?;
        let row = SlackInstallations::find_by_id(id)
            .one(&conn)
            .await
            .map_err(|e| OxyError::DBError(e.to_string()))?
            .ok_or_else(|| OxyError::DBError("installation not found".into()))?;

        // Idempotent — already revoked.
        if row.revoked_at.is_some() {
            return Ok(());
        }

        // Zero-overwrite the bot token secret so the credential is unrecoverable.
        // We can't delete the org_secrets row because the FK from slack_installations
        // is RESTRICT + NOT NULL, and we keep the install row for audit. Overwriting
        // with an empty string achieves the same security goal.
        let org_id = row.org_id;
        let secret_name = format!("slack_bot_token:{org_id}");
        OrgSecretsService::upsert(org_id, &secret_name, "").await?;

        let mut active: slack_installations::ActiveModel = row.into();
        active.revoked_at = ActiveValue::Set(Some(Utc::now().into()));
        active
            .update(&conn)
            .await
            .map_err(|e| OxyError::DBError(e.to_string()))?;
        Ok(())
    }

    pub async fn decrypt_bot_token(inst: &slack_installations::Model) -> Result<String, OxyError> {
        OrgSecretsService::get_by_id(inst.bot_token_secret_id).await
    }
}

/// Translate a unique-constraint DB error into a user-friendly `ValidationError`.
///
/// A TOCTOU race between two concurrent installs can let both pass the
/// application-level SELECT checks and then collide on the partial unique
/// indexes (`uniq_slack_installations_team_active` / `uniq_slack_installations_org_active`).
/// Catching the error here produces the same friendly message as the pre-checks.
fn translate_unique_violation(err: sea_orm::DbErr, team_id: &str, _team_name: &str) -> OxyError {
    if matches!(err.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) {
        OxyError::ValidationError(format!(
            "Slack workspace {} is already connected to an Oxygen org. \
             Disconnect it there first (concurrent install race).",
            team_id
        ))
    } else {
        OxyError::DBError(err.to_string())
    }
}
