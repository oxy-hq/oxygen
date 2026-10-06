//! Sending a fleet route's request on to the node that holds the working
//! copy, for the one case of that route only a working copy can answer.
//!
//! A route that reads no disk is `FleetOk` and any pod serves it. Some of
//! those have a case that cannot be answered without the files — a preview of
//! a branch that was never pushed is one: GitHub does not have it, so only the
//! Factory's working copy does. Pinning the whole route `IdeOnly` for that
//! case is what made every preview need the Factory. Instead the handler
//! answers what it can and replays the rest: the same request, to the Factory,
//! under the headers `role_middleware` puts on a route it forwards itself
//! (`x-oxy-forwarded-via` this replica, `x-oxy-served-by` the Factory's), so
//! the hop stays visible.
//!
//! With no Factory to ask — this process is the one with the files, none is
//! configured, the request already came from a replica, or the Factory does
//! not answer — there is no answer, and the handler gives its own refusal.
//! `invocation_placement` does the same for a custom-app function call.

use std::convert::Infallible;

use axum::body::{Body, Bytes};
use axum::extract::{FromRequestParts, OriginalUri, Request};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method, Uri};
use axum::response::Response;

use crate::server::ide_proxy;
use crate::server::role_manifest::current_process_role;
use crate::server::role_middleware;

/// The request as it arrived, kept so it can be sent again.
pub struct Arrived {
    method: Method,
    /// The full URI the outer stack routed on. Inside a nest axum has
    /// rewritten `Uri` to the remainder, which the Factory would not route.
    uri: Uri,
    headers: HeaderMap,
}

impl<S: Send + Sync> FromRequestParts<S> for Arrived {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Infallible> {
        let uri = parts
            .extensions
            .get::<OriginalUri>()
            .map_or_else(|| parts.uri.clone(), |original| original.0.clone());
        Ok(Self {
            method: parts.method.clone(),
            uri,
            headers: parts.headers.clone(),
        })
    }
}

impl Arrived {
    /// The Factory's answer to this request carrying `body`, whatever its
    /// status. `None` when there is no Factory to ask.
    ///
    /// Call it after the caller is authorized and before anything is written:
    /// the Factory runs the whole handler again.
    pub async fn replayed_to_factory(&self, body: Bytes) -> Option<Response> {
        let upstream = factory_to_ask(
            oxy::workspace_fs_probe::process_owns_workspace_files(),
            ide_proxy::ide_upstream(),
            ide_proxy::forwarded_once(&self.headers),
        )?;
        tracing::info!(
            method = %self.method,
            path = %self.uri.path(),
            "this pod holds no working copy and the request needs one: replaying to the factory"
        );
        // `Err` is a Factory that could not be reached; `forward_to_ide_opt`
        // has logged the URL and the transport error.
        let answer = ide_proxy::forward_to_ide_opt(upstream, self.request(body))
            .await
            .ok()?;
        Some(role_middleware::stamp_forwarded_via(
            answer,
            current_process_role(),
        ))
    }

    fn request(&self, body: Bytes) -> Request {
        let mut request = Request::new(Body::from(body));
        *request.method_mut() = self.method.clone();
        *request.uri_mut() = self.uri.clone();
        *request.headers_mut() = self.headers.clone();
        request
    }
}

/// The upstream to replay to, when there is one worth asking.
fn factory_to_ask(
    holds_working_copy: bool,
    upstream: Option<&'static str>,
    already_forwarded: bool,
) -> Option<&'static str> {
    // The node with the files answers for itself, and a request a replica
    // already sent here must not be sent on again.
    if holds_working_copy || already_forwarded {
        return None;
    }
    upstream
}

#[cfg(test)]
mod tests {
    use super::*;

    const FACTORY: Option<&str> = Some("http://factory");

    #[test]
    fn a_replica_with_a_factory_configured_asks_it() {
        assert_eq!(factory_to_ask(false, FACTORY, false), FACTORY);
    }

    #[test]
    fn the_node_with_the_files_never_asks_anyone() {
        assert_eq!(factory_to_ask(true, FACTORY, false), None);
        assert_eq!(factory_to_ask(true, None, false), None);
    }

    #[test]
    fn a_request_already_replayed_is_not_replayed_again() {
        assert_eq!(factory_to_ask(false, FACTORY, true), None);
    }

    #[test]
    fn with_no_factory_configured_there_is_nobody_to_ask() {
        assert_eq!(factory_to_ask(false, None, false), None);
    }

    #[tokio::test]
    async fn the_request_is_rebuilt_on_the_uri_the_outer_stack_routed_on() {
        let mut parts = Request::builder()
            .method(Method::POST)
            .uri("/refresh?branch=feat%2Fx")
            .header("authorization", "Bearer t")
            .body(())
            .unwrap()
            .into_parts()
            .0;
        parts.extensions.insert(OriginalUri(
            "/api/ws/previews/refresh?branch=feat%2Fx".parse().unwrap(),
        ));
        let arrived = Arrived::from_request_parts(&mut parts, &()).await.unwrap();
        let request = arrived.request(Bytes::from_static(b"{}"));
        assert_eq!(request.method(), Method::POST);
        assert_eq!(
            request.uri().path_and_query().unwrap().as_str(),
            "/api/ws/previews/refresh?branch=feat%2Fx"
        );
        assert_eq!(request.headers()["authorization"], "Bearer t");
    }
}
