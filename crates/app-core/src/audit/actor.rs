//! Who acted in a request, and the one builder that turns that into an audit
//! entry (API-tokens design §3.7, layer 2: "what did this key do?").
//!
//! [`RequestActor`] is an extractor: the authenticated user, the API key or
//! token the request used (if any), and where the request came from. A handler
//! takes it instead of the bare user and builds its entry with
//! [`AuditEntry::for_request`], so a row written while a request is
//! key-authenticated always says so — `actor_type = api_key` plus
//! `metadata.token_id`, `token_name`, `token_kind` and `display_prefix`. Never
//! the token itself: the request marker does not carry it.
//!
//! The stamp is kept apart from the caller's metadata and merged when the row
//! is written, so a later `.metadata(..)` cannot drop it and cannot overwrite
//! it. `call_sites_guard.rs`, beside this file, fails the build on a bare
//! `AuditEntry::new` in request-handling code.

use std::ops::Deref;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use oxy_auth::token::CredentialContext;
use oxy_auth::types::AuthenticatedUser;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::{ActorType, AuditContext, AuditEntry};
use crate::forwarded::client_ip;

/// The request-id header the outermost middleware stamps (`oxy-shared`'s
/// `request_id::HEADER`; spelled here because this crate does not depend on it).
const REQUEST_ID_HEADER: &str = "x-oxy-request-id";
/// A user agent is client-controlled; keep the audit row bounded. The address
/// is bounded where it is read (`forwarded::MAX_CLIENT_IP_CHARS`).
const MAX_USER_AGENT_CHARS: usize = 512;

/// The four metadata keys the credential stamp owns.
pub const TOKEN_ID_KEY: &str = "token_id";
const TOKEN_NAME_KEY: &str = "token_name";
const TOKEN_KIND_KEY: &str = "token_kind";
const DISPLAY_PREFIX_KEY: &str = "display_prefix";

/// What an audit row records about the key or token that performed an action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TokenStamp {
    token_id: Uuid,
    name: String,
    kind: &'static str,
    display_prefix: String,
}

impl From<&CredentialContext> for TokenStamp {
    fn from(c: &CredentialContext) -> Self {
        Self {
            token_id: c.token_id,
            name: c.name.clone(),
            kind: c.kind.as_str(),
            display_prefix: c.display_prefix.clone(),
        }
    }
}

/// The actor of one request. Derefs to the user, so `actor.id` and
/// `actor.label()` read as they did when handlers held the bare user.
#[derive(Clone, Debug)]
pub struct RequestActor {
    pub user: AuthenticatedUser,
    /// The key or token that authenticated the request; `None` for a session.
    pub credential: Option<CredentialContext>,
    pub context: AuditContext,
}

impl RequestActor {
    /// A browser session with no request context — for tests, and for code
    /// that acts for a user outside a request.
    pub fn session(user: AuthenticatedUser) -> Self {
        Self {
            user,
            credential: None,
            context: AuditContext::default(),
        }
    }

    /// The actor of a request that authenticated nobody, where something the
    /// request carried names the user — the `oxyc login` exchange, whose
    /// redeemed code does. No credential; the request's own audit context.
    pub fn for_user(user: AuthenticatedUser, headers: &HeaderMap) -> Self {
        Self {
            user,
            credential: None,
            context: context_from_headers(headers),
        }
    }

    /// The actor of the request `parts` belongs to, once auth has run.
    pub fn from_parts(parts: &Parts) -> Option<Self> {
        let user = parts.extensions.get::<AuthenticatedUser>()?.clone();
        Some(Self {
            user,
            credential: parts.extensions.get::<CredentialContext>().cloned(),
            context: context_from_headers(&parts.headers),
        })
    }
}

impl Deref for RequestActor {
    type Target = AuthenticatedUser;

    fn deref(&self) -> &AuthenticatedUser {
        &self.user
    }
}

impl<S: Send + Sync> FromRequestParts<S> for RequestActor {
    type Rejection = StatusCode;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Self::from_parts(parts).ok_or(StatusCode::UNAUTHORIZED)
    }
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

/// The user agent, bounded.
pub fn user_agent(headers: &HeaderMap) -> Option<String> {
    header(headers, "user-agent").map(|ua| ua.chars().take(MAX_USER_AGENT_CHARS).collect())
}

/// Where the request came from. The address is the one the load balancer saw
/// (`forwarded::client_ip`), not the `X-Forwarded-For` hop the caller wrote.
fn context_from_headers(headers: &HeaderMap) -> AuditContext {
    AuditContext {
        ip: client_ip(headers),
        user_agent: user_agent(headers),
        request_id: header(headers, REQUEST_ID_HEADER).map(str::to_string),
    }
}

impl AuditEntry {
    /// The entry for an action performed in a request. Sets the actor from the
    /// authenticated user and the request context; when a key or token
    /// authenticated the request, records `actor_type = api_key` and stamps the
    /// token's id, name, kind and display prefix into the metadata.
    pub fn for_request(actor: &RequestActor, action: &'static str) -> Self {
        let mut entry = Self::new(actor.user.label().to_string(), action);
        entry.actor_user_id = Some(actor.user.id);
        entry.context = actor.context.clone();
        if let Some(credential) = &actor.credential {
            entry.actor_type = ActorType::ApiKey;
            entry.token = Some(TokenStamp::from(credential));
        }
        entry
    }

    /// Name a session actor's tier (`PartnerAdmin`). A key or token stays
    /// `ApiKey`: that it was a key is the fact a reviewer filters on.
    pub fn acting_as(mut self, actor_type: ActorType) -> Self {
        if self.token.is_none() {
            self.actor_type = actor_type;
        }
        self
    }

    /// The metadata as it is written: the caller's, plus the credential stamp.
    /// The stamp's four keys win, so no caller can overwrite them.
    pub fn effective_metadata(&self) -> Value {
        let Some(stamp) = &self.token else {
            return self.metadata.clone();
        };
        let mut map = match &self.metadata {
            Value::Object(m) => m.clone(),
            Value::Null => Map::new(),
            other => Map::from_iter([("value".to_string(), other.clone())]),
        };
        map.insert(TOKEN_ID_KEY.into(), json!(stamp.token_id));
        map.insert(TOKEN_NAME_KEY.into(), json!(stamp.name));
        map.insert(TOKEN_KIND_KEY.into(), json!(stamp.kind));
        map.insert(DISPLAY_PREFIX_KEY.into(), json!(stamp.display_prefix));
        Value::Object(map)
    }
}

#[cfg(test)]
#[path = "actor_tests.rs"]
mod tests;
