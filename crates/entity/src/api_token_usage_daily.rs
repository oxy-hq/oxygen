//! `SeaORM` Entity for `api_token_usage_daily` — one row per API token per UTC
//! day (API-tokens design §3.7). Counters plus the last IP, user agent and
//! route template seen; never request content. Written by the in-process
//! flusher in `oxy_auth::token::usage`, read by the key's Activity endpoint.

use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "api_token_usage_daily")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub token_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub day: Date,
    pub requests: i64,
    #[sea_orm(column_name = "errors_4xx")]
    pub errors_4xx: i64,
    #[sea_orm(column_name = "errors_5xx")]
    pub errors_5xx: i64,
    pub last_ip: Option<String>,
    pub last_user_agent: Option<String>,
    /// The matched route template (`/api/{workspace_id}/sql/query`), never the
    /// raw path.
    pub last_route: Option<String>,
    pub last_seen_at: DateTimeWithTimeZone,
}

impl ActiveModelBehavior for ActiveModel {}
