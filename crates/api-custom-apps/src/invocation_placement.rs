//! Which pod runs a custom-app function's route invocation.
//!
//! `POST /customer-apps/<org>/<app>/fn/<name>` is `FleetOk`: a function reads
//! its workspace through the compile boundary — `config.yml`, the databases
//! and the semantic model all come from the promoted revision — so any replica
//! runs it, and a custom app does not go down with the Factory.
//!
//! One workspace a replica cannot serve: **nothing promoted.** The only
//! `config.yml` is the one on disk. Building a context anyway reads a
//! directory that is not on the node and hands the function an empty config,
//! so `ctx.query` answers "no databases configured" for a workspace that has
//! three — absent reported as empty, the shape behind the Toast outage
//! (#2816). That invocation keeps the path every invocation took while the
//! route was `IdeOnly`: replayed to a configured Factory, which reads its
//! disk, and answered under the same headers `role_middleware` puts on a route
//! it forwards itself (`x-oxy-forwarded-via` this replica, `x-oxy-served-by`
//! the Factory's), so the hop stays visible. A compile is enqueued on the way,
//! so the workspace stops needing the Factory once it promotes. With no
//! Factory to ask — none configured (every deployment since the ide went to
//! zero replicas), one that does not answer because it is stopped or
//! restarting, or a request that already came from a replica — the answer is
//! a 503 to retry, never a run against an empty config and never the generic
//! ide-down 502 that would hide the retry contract.
//!
//! **A database is never a reason to place a call.** A compiled workspace runs
//! here whatever its databases are, a DuckDB file in the checkout or a key
//! file included. The host fails a query against a database this pod cannot
//! open, naming it (`ConfigManager<ReadOnly>::try_resolve_file`), so only that
//! query fails. A file database used to send every call on its workspace to
//! the Factory; with the Factory gone, every function on such a workspace
//! answered 503, though none of them read that database (INC-174, INC-175).
//! `Servability` has no variant for a database, so that cannot come back
//! through here.
//!
//! Lives here, not in `custom_apps_functions`, because every fact it weighs is
//! the fleet's — this process's role, the Factory upstream, the compile queue.
//! The function handler asks one question after it has authorized the caller,
//! and that is the whole of the custom-apps surface's dependency on any of it
//! (reached through `oxy_app_core::pods`).

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use futures::future::BoxFuture;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use oxy_app_core::pods::role_manifest::current_process_role;
use oxy_app_core::pods::servability::{self, Servability};
use oxy_app_core::pods::{ide_proxy, role_middleware};

/// The header a client retries on — the one `/api/projects/*` answers with
/// when the boundary has nothing for a workspace.
const HEADER_NEEDS_RECOMPILE: &str = "x-oxy-needs-recompile";

/// The request as it arrived, borrowed so it can be replayed to the Factory.
pub(crate) struct Arrived<'a> {
    pub method: &'a Method,
    pub uri: &'a Uri,
    pub headers: &'a HeaderMap,
    pub body: &'a Bytes,
}

/// Where an invocation goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placement {
    /// This process runs it: it holds the working copy, or the workspace is
    /// compiled, whatever its databases are.
    Here,
    /// Nothing promoted, and a Factory is configured: the Factory reads its
    /// disk. If it does not answer, the call is refused as
    /// [`Placement::Refuse`] would.
    Factory,
    /// Nothing promoted, and no Factory is configured — or this request
    /// already came from a replica, so forwarding again would loop.
    Refuse,
}

fn place(holds_working_copy: bool, servability: Servability, factory_reachable: bool) -> Placement {
    if holds_working_copy || servability == Servability::Anywhere {
        Placement::Here
    } else if factory_reachable {
        Placement::Factory
    } else {
        Placement::Refuse
    }
}

/// `None` when this process runs the invocation. Otherwise the response to
/// return in its place: the Factory's answer, or a 503.
///
/// Called after authorization, so an unauthenticated caller learns nothing
/// about a workspace from it, and before the rate-limit bucket and the
/// idempotency row are touched — the Factory takes both for the replayed call,
/// and a row left `running` here would refuse it as a concurrent duplicate.
///
/// Boxed, by a plain `fn`, so the caller holds a pointer. An inline `.await`
/// of an `async fn` reserves the callee's whole future in the caller's poll
/// frame in a debug build, and the function handler's frame
/// (`custom_apps_functions::invoke_function`) stays on the stack for the
/// entire run, nested under everything that awaits it. Awaited inline, this
/// one (the relay is a reqwest send) grew that frame by 21 KB and pushed a
/// test thread that runs a function past its 2 MiB stack. A `Box::pin` at the
/// call site is not enough: the future is still built in the caller's frame
/// before it moves into the box.
pub(crate) fn elsewhere<'a>(
    db: &'a DatabaseConnection,
    project_id: Uuid,
    arrived: Arrived<'a>,
) -> BoxFuture<'a, Option<Response>> {
    Box::pin(placed_elsewhere(db, project_id, arrived))
}

async fn placed_elsewhere(
    db: &DatabaseConnection,
    project_id: Uuid,
    arrived: Arrived<'_>,
) -> Option<Response> {
    // A pod with the working copy serves any workspace from it, as it always
    // has, so it does not ask.
    if oxy::workspace_fs_probe::process_owns_workspace_files() {
        return None;
    }
    let servability = servability::servability(project_id).await;
    let upstream = ide_proxy::ide_upstream();
    // Configured, not proven up: whether it answers is learned by asking. A
    // request that already crossed from a replica is not forwarded again.
    let factory_reachable = upstream.is_some() && !ide_proxy::forwarded_once(arrived.headers);
    let placement = place(false, servability, factory_reachable);
    if placement == Placement::Here {
        return None;
    }
    // Only a workspace with nothing compiled reaches this line. Deduped across
    // replicas and backed off on repeated failure, so a hot function on an
    // uncompilable workspace does not queue a compile per call.
    oxy_compile_runtime::reconcile::enqueue::enqueue_lazy_compile(db, project_id).await;
    tracing::info!(
        %project_id,
        ?placement,
        "function invocation: this pod holds no working copy and the workspace has nothing compiled"
    );
    Some(match (placement, upstream) {
        (Placement::Factory, Some(upstream)) => relayed(
            ide_proxy::forward_to_ide_opt(
                upstream,
                replay(&arrived),
                ide_proxy::ForwardReason::Function,
            )
            .await,
            project_id,
        ),
        _ => refusal(project_id),
    })
}

/// The Factory's answer, or — when it could not be reached — the refusal a
/// replica with no Factory gives, so a stopped or restarting Factory still
/// yields the 503 a client retries on rather than `forward_to_ide`'s 502.
///
/// Only an answer is stamped as `role_middleware` stamps the routes it
/// forwards itself (`x-oxy-forwarded-via: serve@…`, the Factory's
/// `x-oxy-served-by`) — the only trace of the hop a caller or
/// `fleet-assert.sh` has. A refusal was produced here and reads as such.
fn relayed(forwarded: Result<Response, Request<Body>>, project_id: Uuid) -> Response {
    match forwarded {
        Ok(answer) => role_middleware::stamp_forwarded_via(answer, current_process_role()),
        // `forward_to_ide_opt` has logged the URL and the transport error.
        Err(_unreachable) => refusal(project_id),
    }
}

/// The request rebuilt from its parts. The URI is the one the outer stack
/// routed on — already `/customer-apps/<org>/<app>/fn/<name>` whatever host
/// shape the caller used — which is what `enforce_role` forwarded verbatim
/// while this route was `IdeOnly`.
fn replay(arrived: &Arrived<'_>) -> Request<Body> {
    let mut request = Request::new(Body::from(arrived.body.clone()));
    *request.method_mut() = arrived.method.clone();
    *request.uri_mut() = arrived.uri.clone();
    *request.headers_mut() = arrived.headers.clone();
    request
}

/// JSON, not an SSE frame: the client throws on a non-2xx before it reads the
/// event stream, so this is the shape every other pre-stream refusal has.
///
/// Retryable: a compile has been enqueued, and the next call after it promotes
/// runs here.
fn refusal(project_id: Uuid) -> Response {
    let mut response = (
        StatusCode::SERVICE_UNAVAILABLE,
        axum::Json(serde_json::json!({
            "error": "WorkspaceNotCompiled",
            "message": "this app's workspace has no compiled revision on this replica; a \
                        compile has been enqueued — retry shortly",
        })),
    )
        .into_response();
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(&project_id.to_string()) {
        headers.insert(HEADER_NEEDS_RECOMPILE, value);
    }
    headers.insert(header::RETRY_AFTER, HeaderValue::from_static("5"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVERY: [Servability; 2] = [Servability::Anywhere, Servability::NotCompiled];

    async fn error_of(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("a buffered body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("a JSON refusal");
        body["error"].as_str().expect("an error name").to_string()
    }

    #[test]
    fn a_compiled_workspace_runs_on_any_pod() {
        // The case the route is `FleetOk` for: no working copy, and none
        // needed — whether or not a Factory exists, and whatever the
        // workspace's databases are (`Servability` cannot say).
        assert_eq!(place(false, Servability::Anywhere, true), Placement::Here);
        assert_eq!(place(false, Servability::Anywhere, false), Placement::Here);
    }

    #[test]
    fn a_pod_holding_the_working_copy_runs_every_workspace() {
        // The ide, and every single-process deployment: unchanged.
        for servability in EVERY {
            assert_eq!(place(true, servability, true), Placement::Here);
            assert_eq!(place(true, servability, false), Placement::Here);
        }
    }

    #[test]
    fn an_uncompiled_workspace_goes_to_a_reachable_factory() {
        assert_eq!(
            place(false, Servability::NotCompiled, true),
            Placement::Factory
        );
    }

    #[test]
    fn with_no_factory_an_uncompiled_workspace_is_refused_not_run() {
        // Nothing promoted would run against an empty config. Also the loop
        // guard: a request already forwarded counts as no Factory.
        assert_eq!(
            place(false, Servability::NotCompiled, false),
            Placement::Refuse
        );
    }

    #[tokio::test]
    async fn a_factory_that_does_not_answer_is_refused_with_the_documented_503() {
        // Configured but stopped or restarting: `forward_to_ide_opt` hands the
        // request back. The caller must see the 503 it retries on, not the
        // generic ide-down 502, and nothing may claim a hop that did not happen.
        let id = Uuid::new_v4();
        let response = relayed(Err(Request::new(Body::empty())), id);
        let expected = refusal(id);
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(!response.headers().contains_key("x-oxy-forwarded-via"));
        for name in [HEADER_NEEDS_RECOMPILE, "retry-after"] {
            assert_eq!(
                response.headers().get(name),
                expected.headers().get(name),
                "{name}"
            );
        }
        assert_eq!(error_of(response).await, error_of(expected).await);
    }

    #[test]
    fn a_factory_that_answers_is_relayed_under_this_replicas_name() {
        let mut answer = Response::new(Body::from("from the factory"));
        answer
            .headers_mut()
            .insert("x-oxy-served-by", HeaderValue::from_static("ide@factory#1"));
        let response = relayed(Ok(answer), Uuid::new_v4());
        assert_eq!(response.status(), StatusCode::OK, "the Factory's status");
        assert_eq!(
            response.headers().get("x-oxy-served-by").unwrap(),
            "ide@factory#1"
        );
        assert!(response.headers().contains_key("x-oxy-forwarded-via"));
    }

    #[tokio::test]
    async fn an_uncompiled_workspace_is_refused_as_retryable() {
        let id = Uuid::new_v4();
        let response = refusal(id);
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response.headers().get(HEADER_NEEDS_RECOMPILE).unwrap(),
            id.to_string().as_str()
        );
        assert_eq!(response.headers().get(header::RETRY_AFTER).unwrap(), "5");
        assert_eq!(error_of(response).await, "WorkspaceNotCompiled");
    }

    #[test]
    fn the_replayed_request_is_the_one_that_arrived() {
        let method = Method::POST;
        let uri: Uri = "/customer-apps/acme/store/fn/orders?refresh=1"
            .parse()
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("idempotency-key", HeaderValue::from_static("k-1"));
        headers.insert(
            header::HOST,
            HeaderValue::from_static("acme--store.customer-apps.oxygen-hq.com"),
        );
        let body = Bytes::from_static(b"{\"a\":1}");
        let request = replay(&Arrived {
            method: &method,
            uri: &uri,
            headers: &headers,
            body: &body,
        });
        assert_eq!(request.method(), Method::POST);
        assert_eq!(request.uri(), &uri);
        assert_eq!(request.headers(), &headers);
    }
}
