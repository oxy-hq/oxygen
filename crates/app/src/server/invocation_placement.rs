//! Which pod runs a custom-app function's route invocation.
//!
//! `POST /customer-apps/<org>/<app>/fn/<name>` is `FleetOk`: a function reads
//! its workspace through the compile boundary — `config.yml`, the databases
//! and the semantic model all come from the promoted revision — so any replica
//! runs it, and a custom app does not go down with the Factory.
//!
//! Two workspaces a replica cannot serve, both because what the function needs
//! exists only in the working copy, which a replica does not hold:
//!
//! - **Nothing promoted.** The only `config.yml` is the one on disk. Building a
//!   context anyway reads a directory that is not on the node and hands the
//!   function an empty config, so `ctx.query` answers "no databases configured"
//!   for a workspace that has three — absent reported as empty, the shape
//!   behind the Toast outage (#2816).
//! - **A database that is a file in the checkout** — a local DuckDB the compiler
//!   could not mirror to S3, a BigQuery key file. The config is known; the data
//!   is not here.
//!
//! Those invocations keep the path every invocation took while the route was
//! `IdeOnly`: replayed to the Factory, which reads its disk, and answered
//! under the same headers `role_middleware` puts on a route it forwards
//! itself (`x-oxy-forwarded-via` this replica, `x-oxy-served-by` the
//! Factory's), so the hop stays visible. For the first, a compile is enqueued
//! on the way, so the workspace stops needing the Factory once it promotes.
//! With no Factory to ask — none configured, or one that does not answer
//! because it is stopped or restarting — the answer is a 503 that says which
//! of the two it is, never a run against an empty config and never the
//! generic ide-down 502 that would hide the retry contract.
//!
//! The judgement is `serve_safety`'s, the allowlist the `/analytics` un-pin
//! already uses: anything it cannot prove file-free goes to the Factory, so the
//! worst case is a hop that was not needed.
//!
//! Lives here, not in `custom_apps_functions`, because every fact it weighs is
//! the fleet's — this process's role, the Factory upstream, the compile queue.
//! The function handler asks one question after it has authorized the caller,
//! and that is the whole of the custom-apps surface's dependency on any of it
//! (one entry in `tests/custom_apps/custom_apps_boundary.rs`).

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use futures::future::BoxFuture;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use crate::server::ide_proxy;
use crate::server::role_manifest::current_process_role;
use crate::server::role_middleware;
use crate::server::serve_safety::{self, Servability};

/// The header a client retries on — the one `/api/projects/*` answers with
/// when the boundary has nothing for a workspace.
const HEADER_NEEDS_RECOMPILE: &str = "x-oxy-needs-recompile";
/// The header every refusal that only the Factory can serve carries.
const HEADER_REQUIRED_ROLE: &str = "x-oxy-required-role";

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
    /// This process can run it.
    Here,
    /// This pod cannot; the Factory reads its disk. If it does not answer,
    /// the call is refused as [`Placement::Refuse`] would.
    Factory,
    /// This pod cannot, and no Factory is configured — or this request
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
    let servability = serve_safety::servability(project_id).await;
    let upstream = ide_proxy::ide_upstream();
    // Configured, not proven up: whether it answers is learned by asking.
    let factory_reachable = upstream.is_some() && !ide_proxy::forwarded_once(arrived.headers);
    let placement = place(false, servability, factory_reachable);
    if placement == Placement::Here {
        return None;
    }
    if servability == Servability::NotCompiled {
        // Deduped across replicas and backed off on repeated failure, so a hot
        // function on an uncompilable workspace does not queue a compile per
        // call. Not for `NeedsWorkingCopy`: that workspace IS compiled, and
        // recompiling it on every call would change nothing.
        crate::server::api::middlewares::workspace_context::enqueue_lazy_compile(db, project_id)
            .await;
    }
    tracing::info!(
        %project_id,
        ?servability,
        ?placement,
        "function invocation: this pod holds no working copy and the workspace needs one"
    );
    Some(match (placement, upstream) {
        (Placement::Factory, Some(upstream)) => relayed(
            ide_proxy::forward_to_ide_opt(upstream, replay(&arrived)).await,
            project_id,
            servability,
        ),
        _ => refusal(project_id, servability),
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
fn relayed(
    forwarded: Result<Response, Request<Body>>,
    project_id: Uuid,
    servability: Servability,
) -> Response {
    match forwarded {
        Ok(answer) => role_middleware::stamp_forwarded_via(answer, current_process_role()),
        // `forward_to_ide_opt` has logged the URL and the transport error.
        Err(_unreachable) => refusal(project_id, servability),
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
/// Three refusals, because the caller's next move differs: an uncompiled
/// workspace is worth retrying once the queued compile lands; one whose config
/// could not be read just now is worth retrying as it is; one whose database
/// is a file in the checkout is not, and says which pod it needs.
fn refusal(project_id: Uuid, servability: Servability) -> Response {
    let (error, message) = match servability {
        Servability::NotCompiled => (
            "WorkspaceNotCompiled",
            "this app's workspace has no compiled revision on this replica; a compile has \
             been enqueued — retry shortly",
        ),
        Servability::Unknown => (
            "WorkspaceConfigUnavailable",
            "this app's workspace config could not be read on this replica just now, and no \
             Factory is reachable to run it — retry shortly",
        ),
        // `Anywhere` never reaches a refusal; named so a new variant must be.
        Servability::NeedsWorkingCopy | Servability::Anywhere => (
            "WorkspaceNeedsWorkingCopy",
            "this app's workspace cannot be served from this replica: a database in its \
             config is a file in the working copy (a local DuckDB or a key file); no \
             Factory is reachable to run it",
        ),
    };
    let mut response = (
        StatusCode::SERVICE_UNAVAILABLE,
        axum::Json(serde_json::json!({ "error": error, "message": message })),
    )
        .into_response();
    let headers = response.headers_mut();
    match servability {
        Servability::NotCompiled => {
            if let Ok(value) = HeaderValue::from_str(&project_id.to_string()) {
                headers.insert(HEADER_NEEDS_RECOMPILE, value);
            }
            headers.insert(header::RETRY_AFTER, HeaderValue::from_static("5"));
        }
        // Transient: retry, and no compile — one would not fix a database blip.
        Servability::Unknown => {
            headers.insert(header::RETRY_AFTER, HeaderValue::from_static("5"));
        }
        Servability::NeedsWorkingCopy | Servability::Anywhere => {
            headers.insert(HEADER_REQUIRED_ROLE, HeaderValue::from_static("ide"));
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVERY: [Servability; 4] = [
        Servability::Anywhere,
        Servability::NeedsWorkingCopy,
        Servability::NotCompiled,
        Servability::Unknown,
    ];
    /// Every workspace a replica cannot prove it can serve.
    const NOT_HERE: [Servability; 3] = [
        Servability::NeedsWorkingCopy,
        Servability::NotCompiled,
        Servability::Unknown,
    ];

    async fn error_of(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("a buffered body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("a JSON refusal");
        body["error"].as_str().expect("an error name").to_string()
    }

    #[test]
    fn a_servable_workspace_runs_on_any_pod() {
        // The case the route is `FleetOk` for: no working copy, and none
        // needed — whether or not a Factory exists.
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
    fn what_only_the_working_copy_can_answer_goes_to_the_factory() {
        // `Unknown` too: what cannot be proven file-free costs a hop at worst.
        for servability in NOT_HERE {
            assert_eq!(place(false, servability, true), Placement::Factory);
        }
    }

    #[test]
    fn with_no_factory_to_ask_it_is_refused_not_run() {
        for servability in NOT_HERE {
            assert_eq!(place(false, servability, false), Placement::Refuse);
        }
    }

    #[tokio::test]
    async fn a_factory_that_does_not_answer_gets_the_same_refusal_as_none() {
        // Configured but stopped or restarting: `forward_to_ide_opt` hands the
        // request back. The caller must see the 503 it retries on, not the
        // generic ide-down 502, and nothing may claim a hop that did not happen.
        let id = Uuid::new_v4();
        for servability in NOT_HERE {
            let unreachable = Err(Request::new(Body::empty()));
            let response = relayed(unreachable, id, servability);
            let expected = refusal(id, servability);
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert!(!response.headers().contains_key("x-oxy-forwarded-via"));
            for name in [HEADER_NEEDS_RECOMPILE, HEADER_REQUIRED_ROLE, "retry-after"] {
                assert_eq!(
                    response.headers().get(name),
                    expected.headers().get(name),
                    "{servability:?}: {name}"
                );
            }
            assert_eq!(error_of(response).await, error_of(expected).await);
        }
    }

    #[test]
    fn a_factory_that_answers_is_relayed_under_this_replicas_name() {
        let mut answer = Response::new(Body::from("from the factory"));
        answer
            .headers_mut()
            .insert("x-oxy-served-by", HeaderValue::from_static("ide@factory#1"));
        let response = relayed(Ok(answer), Uuid::new_v4(), Servability::NotCompiled);
        assert_eq!(response.status(), StatusCode::OK, "the Factory's status");
        assert_eq!(
            response.headers().get("x-oxy-served-by").unwrap(),
            "ide@factory#1"
        );
        assert!(response.headers().contains_key("x-oxy-forwarded-via"));
    }

    #[test]
    fn an_uncompiled_workspace_is_refused_as_retryable() {
        let id = Uuid::new_v4();
        let response = refusal(id, Servability::NotCompiled);
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response.headers().get(HEADER_NEEDS_RECOMPILE).unwrap(),
            id.to_string().as_str()
        );
        assert!(response.headers().contains_key(header::RETRY_AFTER));
    }

    #[test]
    fn a_working_copy_database_is_refused_as_needing_the_factory() {
        // Not retryable: no compile makes a file appear on a replica, so it
        // must not carry the header a client retries on.
        let response = refusal(Uuid::new_v4(), Servability::NeedsWorkingCopy);
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(!response.headers().contains_key(HEADER_NEEDS_RECOMPILE));
        assert_eq!(response.headers().get(HEADER_REQUIRED_ROLE).unwrap(), "ide");
    }

    #[tokio::test]
    async fn a_config_that_could_not_be_read_is_refused_as_transient() {
        // A database blip, not a property of the workspace: retry as it is.
        // Not the working-copy refusal (whose headers say "do not retry, find
        // the ide"), and not the recompile one (a compile would change nothing).
        let response = refusal(Uuid::new_v4(), Servability::Unknown);
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers().get(header::RETRY_AFTER).unwrap(), "5");
        assert!(!response.headers().contains_key(HEADER_REQUIRED_ROLE));
        assert!(!response.headers().contains_key(HEADER_NEEDS_RECOMPILE));
        assert_eq!(error_of(response).await, "WorkspaceConfigUnavailable");
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
