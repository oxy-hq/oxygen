use chrono::Utc;
use entity::prelude::SlackUserLinks;
use entity::slack_user_links;
use oxy::database::client::establish_connection;
use oxy_shared::errors::OxyError;
use sea_orm::prelude::Expr;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

pub struct UserLinksService;

pub struct CreateLink {
    pub installation_id: Uuid,
    pub slack_user_id: String,
    pub oxy_user_id: Uuid,
    pub link_method: LinkMethod,
}

#[derive(Copy, Clone, Debug)]
pub enum LinkMethod {
    /// A Slack user who messaged the bot, matched by the email Slack holds.
    EmailAuto,
    /// An Oxygen member a notification had to reach, found by the email
    /// Oxygen holds (`linking::by_email`).
    EmailLookup,
    MagicLink,
}

impl LinkMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EmailAuto => "email_auto",
            Self::EmailLookup => "email_lookup",
            Self::MagicLink => "magic_link",
        }
    }
}

impl UserLinksService {
    pub async fn find(
        installation_id: Uuid,
        slack_user_id: &str,
    ) -> Result<Option<slack_user_links::Model>, OxyError> {
        let conn = establish_connection().await?;
        SlackUserLinks::find()
            .filter(slack_user_links::Column::InstallationId.eq(installation_id))
            .filter(slack_user_links::Column::SlackUserId.eq(slack_user_id))
            .one(&conn)
            .await
            .map_err(|e| OxyError::DBError(e.to_string()))
    }

    pub async fn touch_last_seen(id: Uuid) -> Result<(), OxyError> {
        let conn = establish_connection().await?;
        SlackUserLinks::update_many()
            .col_expr(
                slack_user_links::Column::LastSeenAt,
                Expr::value(Utc::now()),
            )
            .filter(slack_user_links::Column::Id.eq(id))
            .exec(&conn)
            .await
            .map_err(|e| OxyError::DBError(e.to_string()))?;
        Ok(())
    }

    pub async fn create(input: CreateLink) -> Result<slack_user_links::Model, OxyError> {
        let conn = establish_connection().await?;
        slack_user_links::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            installation_id: ActiveValue::Set(input.installation_id),
            slack_user_id: ActiveValue::Set(input.slack_user_id),
            oxy_user_id: ActiveValue::Set(input.oxy_user_id),
            link_method: ActiveValue::Set(input.link_method.as_str().into()),
            linked_at: ActiveValue::NotSet,
            last_seen_at: ActiveValue::NotSet,
        }
        .insert(&conn)
        .await
        .map_err(|e| OxyError::DBError(e.to_string()))
    }

    pub async fn delete(id: Uuid) -> Result<(), OxyError> {
        let conn = establish_connection().await?;
        SlackUserLinks::delete_by_id(id)
            .exec(&conn)
            .await
            .map_err(|e| OxyError::DBError(e.to_string()))?;
        Ok(())
    }
}
