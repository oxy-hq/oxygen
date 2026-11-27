//! The usage count a sandbox agent token's request gets on the two paths that
//! authenticate inline, `/fn` and `/logs`, and so sit outside `/api`'s usage
//! layer. Beside [`crate::agent`], which holds the token's admission rules.
//! It names `http` types only, no web framework: [`UsageProbe::counted`] is
//! generic over the response body, so the surface hands it an axum handler
//! as before. Up from `oxy_app_core::usage_probe` (D6, 2026-11-27), whose
//! charter named this crate its owner.

use std::future::Future;

use oxy_auth::token::CredentialContext;
use oxy_auth::token::usage::{self, UsageSample};
use oxy_telemetry::http_trace::RequestTokenId;
use uuid::Uuid;

tokio::task_local! {
    /// The API token the request in flight authenticated with, for
    /// [`UsageProbe::counted`]. Set by [`seen`], inside that scope only.
    static SEEN: std::cell::Cell<Option<Uuid>>;
}

/// Note the token a request authenticated with, for the count taken when the
/// request ends. A no-op for a session, and outside [`UsageProbe::counted`].
pub fn seen(credential: Option<&CredentialContext>) {
    if let Some(credential) = credential {
        let _ = SEEN.try_with(|slot| slot.set(Some(credential.token_id)));
    }
}

/// Counts one request against the **new-format API token** it used — personal,
/// service-account, `ci` or sandbox agent — on the two paths that authenticate
/// inline and so sit outside `/api`'s usage layer: `/fn` and `/logs`. What is
/// recorded is what that layer records — the token id, the response status,
/// the client address and user agent, and the route template — and the token
/// id is put on the response for the request span.
///
/// Free for everyone else, and cheap for the token: whether to count is read
/// off the presented prefix (`presents_api_token`, one header look and no
/// database read), the token's id is the one authentication resolved anyway,
/// and the sample goes to the in-memory accumulator the `/api` layer feeds. A
/// session, an anonymous request and a legacy key present no such prefix and
/// run their handler exactly as before — the serving chain stays fully cached
/// for them, and a legacy key is counted here no more than it ever was.
pub struct UsageProbe(Option<(Option<String>, Option<String>)>);

impl UsageProbe {
    pub fn of(headers: &http::HeaderMap) -> Self {
        Self(oxy_auth::token::presents_api_token(headers).then(|| {
            (
                oxy_app_core::forwarded::client_ip(headers),
                oxy_app_core::audit::user_agent(headers),
            )
        }))
    }

    /// Run `handler`; count its response if the token authenticated.
    pub async fn counted<B, F>(self, route: &'static str, handler: F) -> http::Response<B>
    where
        F: Future<Output = http::Response<B>>,
    {
        let Some((ip, user_agent)) = self.0 else {
            return handler.await;
        };
        SEEN.scope(std::cell::Cell::new(None), async move {
            let mut response = handler.await;
            let Some(token_id) = SEEN.with(std::cell::Cell::get) else {
                return response;
            };
            response
                .extensions_mut()
                .insert(RequestTokenId(token_id.to_string()));
            usage::record(UsageSample {
                token_id,
                status: response.status().as_u16(),
                ip,
                user_agent,
                route: Some(route.to_string()),
                at: chrono::Utc::now(),
            });
            response
        })
        .await
    }
}
