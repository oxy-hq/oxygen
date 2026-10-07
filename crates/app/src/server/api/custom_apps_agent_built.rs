//! Builds a **sandbox agent token** published
//! (`app_builds.published_token_id`) — to a sandbox of its own, or as a draft
//! to staging — and the two rules every such build is held to (sandbox agent
//! credential design, "Staging option", §10.5).
//!
//! An agent publishes builds nobody approved. So each one remembers who
//! published it, and:
//!
//! 1. **Production never falls back to it** ([`production_fallback`]). An app
//!    with no production build of its own — never promoted, or unpublished —
//!    serves staging's build on the production path, HTML and functions
//!    alike. Not when a token published that build: production then serves
//!    nothing, exactly as an app with neither build. The draft's guard that
//!    the app is live is asked when the draft is published; this is what
//!    holds once the app is unpublished afterwards. (Only a draft is ever
//!    staging's build: nothing moves staging's pointer to a sandbox build.)
//! 2. **No blind promote ships it** ([`refusing_promote`]). Promote, batch
//!    promote, promote latest and a rollback to it move production's pointer
//!    to whatever build they find, with no look at whose it is — promote
//!    latest takes an app's newest build whatever served it, and a rollback
//!    names any retained build, so a token's **sandbox** build was as
//!    shippable as its draft. Each refuses a marked build with
//!    `409 draft_published_by_agent`, naming the token, its minter and the
//!    environment the build was published to. (The code says "draft" for a
//!    sandbox build too: clients key on it.) A person who publishes the build
//!    under their own name has a build that is theirs, and that one promotes
//!    as any build does.
//!
//! The mark is set on every build a sandbox agent token publishes
//! (`custom_apps_sandboxes::agent_publish::author`) and is never updated, so
//! both rules read `NULL` — and change nothing — for every build a person or
//! a CI job published. Nothing a sandbox does with its own build reads it: a
//! sandbox serves what its row names.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};
use std::time::Instant;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use entity::{api_tokens, app_builds, app_environment_events, users};
use sea_orm::QuerySelect;
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, QueryOrder};
use uuid::Uuid;

use super::custom_apps_cache::{get_fresh, insert_with_sweep};

/// The code a promote or a rollback refuses a token-published build with.
pub const PROMOTE_REFUSED: &str = "draft_published_by_agent";

/// Whether a build is marked, per build id, for the 60 s every other step of
/// the serve chain is cached. The mark is written with the row and never
/// changed, so an entry can only ever be refreshed with the same answer.
type MarkCache = RwLock<HashMap<Uuid, (bool, Instant)>>;

fn mark_cache() -> &'static MarkCache {
    static CACHE: OnceLock<MarkCache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Whether a sandbox agent token published build `build_pk`. One primary-key
/// read of one column, then cached: only a request that takes production's
/// fallback asks, never one of an app with a production build.
pub(crate) async fn is_agent_built<C: ConnectionTrait>(
    db: &C,
    build_pk: Uuid,
) -> Result<bool, DbErr> {
    if let Some(known) = get_fresh(mark_cache(), &build_pk) {
        return Ok(known);
    }
    let token: Option<Option<Uuid>> = app_builds::Entity::find_by_id(build_pk)
        .select_only()
        .column(app_builds::Column::PublishedTokenId)
        .into_tuple()
        .one(db)
        .await?;
    let marked = token.flatten().is_some();
    insert_with_sweep(mark_cache(), build_pk, marked);
    Ok(marked)
}

/// Make a build's mark known, as a first request would have left it: for a
/// test that resolves with no database behind it.
#[cfg(test)]
pub(crate) fn remember(build_pk: Uuid, marked: bool) {
    insert_with_sweep(mark_cache(), build_pk, marked);
}

/// What production serves when it has no build of its own: `staging`'s build,
/// as it always has — unless a sandbox agent token published that build, and
/// then nothing. `Err` when the mark cannot be read: the caller serves nothing
/// rather than a build nobody approved.
pub(crate) async fn production_fallback<C: ConnectionTrait>(
    db: &C,
    staging: Option<Uuid>,
) -> Result<Option<Uuid>, DbErr> {
    match staging {
        Some(build) if is_agent_built(db, build).await? => Ok(None),
        other => Ok(other),
    }
}

/// A build no blind promote ships, and who published it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentBuilt {
    /// The build's label, as the publish named it.
    pub build_id: String,
    /// The environment the token published it to: a sandbox of its own
    /// (`dev-<handle>`), or `staging`. `None` when no pointer event records
    /// one.
    pub environment: Option<String>,
    pub token_id: Uuid,
    /// `None` when the token's row is gone with its minter.
    pub token_name: Option<String>,
    /// The address of the person who minted the token.
    pub minter: Option<String>,
}

impl AgentBuilt {
    /// Who published it, where to, and what to do: sentences a person can
    /// act on, whether the build was a draft or a sandbox's.
    pub fn message(&self) -> String {
        let token = match &self.token_name {
            Some(name) => format!("sandbox agent token {name:?}"),
            None => format!(
                "a sandbox agent token that no longer exists ({})",
                self.token_id
            ),
        };
        let minter = match &self.minter {
            Some(email) => format!(", minted by {email}"),
            None => String::new(),
        };
        let to = match &self.environment {
            Some(environment) => format!(" to {environment}"),
            None => String::new(),
        };
        format!(
            "build {:?} was published{to} by {token}{minter}, and nobody has approved it, so it \
             is not made live as it is. Publish the build under your own name (`oxyc publish`): \
             that build is yours, and is promoted as usual. Or approve a promote request for it",
            self.build_id
        )
    }

    /// `409`, as JSON a client can branch on.
    pub fn into_response(self) -> Response {
        let body = serde_json::json!({
            "code": PROMOTE_REFUSED,
            "error": PROMOTE_REFUSED,
            "message": self.message(),
            "build_id": self.build_id,
            "environment": self.environment,
            "token_id": self.token_id,
            "token_name": self.token_name,
            "minter": self.minter,
        });
        (StatusCode::CONFLICT, Json(body)).into_response()
    }
}

/// What a promote or a rollback route refuses with: the bare status it has
/// always answered, or — for a build a sandbox agent token published — the
/// coded `409`. One type, so a route gains the second without its other
/// answers changing by a byte.
#[derive(Debug)]
pub enum PromoteRefusal {
    Status(StatusCode),
    AgentBuilt(Box<AgentBuilt>),
}

impl From<StatusCode> for PromoteRefusal {
    fn from(status: StatusCode) -> Self {
        Self::Status(status)
    }
}

impl From<AgentBuilt> for PromoteRefusal {
    fn from(built: AgentBuilt) -> Self {
        Self::AgentBuilt(Box::new(built))
    }
}

impl IntoResponse for PromoteRefusal {
    fn into_response(self) -> Response {
        match self {
            Self::Status(status) => status.into_response(),
            Self::AgentBuilt(built) => built.into_response(),
        }
    }
}

/// `Some` when `build` was published by a sandbox agent token, and so may not
/// be promoted or rolled back to: who published it and where to, for the
/// refusal. `None` for every other build, with no read.
pub(crate) async fn refusing_promote<C: ConnectionTrait>(
    db: &C,
    build: &app_builds::Model,
) -> Result<Option<AgentBuilt>, DbErr> {
    let Some(token_id) = build.published_token_id else {
        return Ok(None);
    };
    let token = api_tokens::Entity::find_by_id(token_id).one(db).await?;
    let minter = match &token {
        Some(token) => users::Entity::find_by_id(token.principal_user_id)
            .one(db)
            .await?
            .and_then(|user| user.email),
        None => None,
    };
    Ok(Some(AgentBuilt {
        build_id: build.build_id.clone(),
        environment: published_to(db, build.id).await?,
        token_id,
        token_name: token.map(|token| token.name),
        minter,
    }))
}

/// The environment build `build_pk` was published to: the one its first
/// pointer event names. A publish moves exactly one pointer to its new build
/// — a sandbox's, or staging's — before anything else can point at it.
async fn published_to<C: ConnectionTrait>(db: &C, build_pk: Uuid) -> Result<Option<String>, DbErr> {
    let first = app_environment_events::Entity::find()
        .filter(app_environment_events::Column::BuildId.eq(build_pk))
        .order_by_asc(app_environment_events::Column::At)
        .one(db)
        .await?;
    Ok(first.map(|event| event.environment))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn built(token_name: Option<&str>, minter: Option<&str>) -> AgentBuilt {
        AgentBuilt {
            build_id: "agent-draft-1".into(),
            environment: Some("staging".into()),
            token_id: Uuid::from_u128(0x70),
            token_name: token_name.map(str::to_string),
            minter: minter.map(str::to_string),
        }
    }

    /// The refusal names the build, where it was published, the token and
    /// its minter, and says both ways forward.
    #[test]
    fn the_message_says_who_published_it_and_what_to_do() {
        let message = built(Some("nightly agent"), Some("minter@oxy.tech")).message();
        for said in [
            "\"agent-draft-1\"",
            "published to staging by",
            "sandbox agent token \"nightly agent\"",
            "minted by minter@oxy.tech",
            "Publish the build under your own name",
            "approve a promote request",
        ] {
            assert!(message.contains(said), "{said}: {message}");
        }
        // A token whose row is gone is still named, by id.
        let message = built(None, None).message();
        assert!(message.contains("no longer exists"), "{message}");
        assert!(!message.contains("minted by"), "{message}");
    }

    /// A sandbox build is refused under the same code, and the message says
    /// it was a sandbox's: the environment it names is the one thing that
    /// tells the two apart.
    #[tokio::test]
    async fn a_sandbox_build_is_named_by_the_sandbox_it_was_published_to() {
        let sandbox = AgentBuilt {
            build_id: "sbx-7".into(),
            environment: Some("dev-a1".into()),
            ..built(Some("nightly agent"), Some("minter@oxy.tech"))
        };
        let message = sandbox.message();
        assert!(
            message.contains("\"sbx-7\" was published to dev-a1 by"),
            "{message}"
        );
        assert!(!message.contains("staging"), "{message}");
        let response = sandbox.into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("read the body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("a JSON body");
        assert_eq!(body["code"], "draft_published_by_agent");
        assert_eq!(body["environment"], "dev-a1");

        // A build no pointer event records is still refused, and says no place.
        let nowhere = AgentBuilt {
            environment: None,
            ..built(Some("nightly agent"), None)
        };
        let message = nowhere.message();
        assert!(
            message.contains("was published by sandbox agent token"),
            "{message}"
        );
    }

    #[tokio::test]
    async fn the_refusal_is_a_409_with_the_code_a_client_branches_on() {
        let refused = built(Some("nightly agent"), Some("minter@oxy.tech"));
        let message = refused.message();
        let response = refused.into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("read the body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("a JSON body");
        assert_eq!(body["code"], "draft_published_by_agent");
        assert_eq!(body["error"], "draft_published_by_agent");
        assert_eq!(body["message"], message.as_str());
        assert_eq!(body["build_id"], "agent-draft-1");
        assert_eq!(body["environment"], "staging");
        assert_eq!(body["token_name"], "nightly agent");
        assert_eq!(body["minter"], "minter@oxy.tech");
    }

    /// The mark is read from the cache once it is known: a build is asked of
    /// the database at most once a minute, whatever serves it.
    #[test]
    fn a_known_mark_is_answered_from_the_cache() {
        let (marked, plain) = (Uuid::new_v4(), Uuid::new_v4());
        assert_eq!(get_fresh(mark_cache(), &marked), None);
        insert_with_sweep(mark_cache(), marked, true);
        insert_with_sweep(mark_cache(), plain, false);
        assert_eq!(get_fresh(mark_cache(), &marked), Some(true));
        assert_eq!(get_fresh(mark_cache(), &plain), Some(false));
    }
}
