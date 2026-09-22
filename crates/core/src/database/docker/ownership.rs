//! Which oxy-managed containers THIS process created, and the shutdown cleanup
//! that removes only those.
//!
//! The containers carry fixed names (`oxy-postgres`, `oxy-clickhouse`), so a
//! by-name removal reaches whichever container holds the name at that moment. The
//! server's shutdown path used to do exactly that for every `oxy serve`: stopping
//! a second local server that ran against its own database removed the running
//! `oxy start` stack's Postgres and ClickHouse (the named volumes survived).
//!
//! Two rules close that:
//!
//! - **Ownership is recorded where a container is created**, not passed in by a
//!   caller. A plain `oxy serve` creates none, so it has nothing to claim, and no
//!   call site can assert an ownership it does not have.
//! - **A claim is the container's ID, never its name.** A name outlives the
//!   container behind it: a second `oxy start` force-removes the first one's
//!   containers on startup and creates its own under the same names, so a claim
//!   by name would have the first process remove the second one's containers on
//!   shutdown. An ID names one container for good; a stale claim 404s, which
//!   `remove_container` already treats as nothing to do.
//!
//! So a second server on one machine is safe to stop — only `oxy start` removes
//! containers on shutdown, and only the ones it created.

use std::future::Future;
use std::sync::{Mutex, PoisonError};

use bollard::Docker;
use oxy_shared::errors::OxyError;
use tracing::warn;

use super::{get_docker_client, remove_container};

/// The IDs of the oxy-managed containers this process created, in creation order.
pub(super) struct OwnedContainers(Mutex<Vec<String>>);

/// The one registry for this process. Tests build their own instance so they
/// never claim a container on behalf of the test binary.
pub(super) static OWNED: OwnedContainers = OwnedContainers::new();

impl OwnedContainers {
    pub(super) const fn new() -> Self {
        Self(Mutex::new(Vec::new()))
    }

    /// Claim the container the runtime just created, by the `id` in its create
    /// response — never by its name (see the module docs). Called right after
    /// the create is accepted, because from then on the container exists on this
    /// process's account whether or not it goes on to start.
    pub(super) fn record(&self, id: String) {
        let mut owned = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if !owned.contains(&id) {
            owned.push(id);
        }
    }

    fn snapshot(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// The one thing shutdown asks of a container runtime. [`Docker`] is the shipped
/// implementation; tests substitute a recorder, so none of them reaches a runtime.
trait RemoveContainer {
    /// Force-remove the container with this ID, best effort.
    async fn remove(&self, id: &str);
}

impl RemoveContainer for Docker {
    async fn remove(&self, id: &str) {
        remove_container(self, id).await;
    }
}

/// What [`cleanup_owned_containers`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShutdownCleanup {
    /// This process created no oxy-managed container — a plain `oxy serve`. No
    /// Docker client was built and nothing was removed.
    NotOwned,
    /// This process created the containers with these IDs, and a force-removal
    /// was ATTEMPTED for each, in creation order. Best effort: `remove_container`
    /// logs a failure and carries on, and a claim that went stale is a no-op, so
    /// this does not say that any of them was actually removed.
    Removed(Vec<String>),
    /// This process owns containers but the container runtime was unreachable,
    /// so they were left behind; the next `oxy start` removes them on startup.
    DockerUnavailable,
}

/// Shutdown cleanup: stop and remove the containers THIS process created
/// through `oxy start`, and nothing else.
///
/// A plain `oxy serve` created none, so it gets [`ShutdownCleanup::NotOwned`]
/// back without a Docker client ever being built — stopping it cannot touch
/// another server's database containers. Removal is by container ID, so a claim
/// on a container that something else has since replaced under the same name
/// removes nothing. Volumes and networks are never removed here. Errors are
/// logged, not propagated: cleanup must not block shutdown.
pub async fn cleanup_owned_containers() -> ShutdownCleanup {
    cleanup_owned_with(OWNED.snapshot(), get_docker_client).await
}

/// [`cleanup_owned_containers`] with the owned IDs and the runtime connection
/// injected, so a test can prove the not-owned path never connects and that the
/// owned path asks for exactly the recorded IDs.
async fn cleanup_owned_with<R, F, Fut>(owned: Vec<String>, connect: F) -> ShutdownCleanup
where
    R: RemoveContainer,
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<R, OxyError>>,
{
    if owned.is_empty() {
        return ShutdownCleanup::NotOwned;
    }

    let runtime = match connect().await {
        Ok(runtime) => runtime,
        Err(e) => {
            warn!("Could not connect to Docker during cleanup: {}", e);
            return ShutdownCleanup::DockerUnavailable;
        }
    };

    for id in &owned {
        runtime.remove(id).await;
    }
    ShutdownCleanup::Removed(owned)
}

#[cfg(test)]
mod tests {
    use super::super::{CLICKHOUSE_CONTAINER_NAME, POSTGRES_CONTAINER_NAME};
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// The ID process A's `oxy start` was given for the `oxy-postgres` it created.
    const A_POSTGRES_ID: &str = "a1f0c3d2e4b5";
    /// The ID of the `oxy-postgres` process B created later, under the same name.
    const B_POSTGRES_ID: &str = "b7e9d8c6a5f4";

    /// A stand-in runtime that resolves a removal target the way Docker does — by
    /// container ID **or** by name — so a by-name removal takes whatever holds
    /// the name. It records every target it was asked to remove.
    #[derive(Clone, Default)]
    struct FakeRuntime(Arc<Mutex<FakeState>>);

    #[derive(Default)]
    struct FakeState {
        /// `(name, id)` of each running container.
        running: Vec<(&'static str, &'static str)>,
        asked_to_remove: Vec<String>,
    }

    impl FakeRuntime {
        fn running(containers: &[(&'static str, &'static str)]) -> Self {
            let runtime = Self::default();
            runtime.0.lock().unwrap().running = containers.to_vec();
            runtime
        }

        fn asked_to_remove(&self) -> Vec<String> {
            self.0.lock().unwrap().asked_to_remove.clone()
        }

        fn still_running(&self) -> Vec<&'static str> {
            let state = self.0.lock().unwrap();
            state.running.iter().map(|(_, id)| *id).collect()
        }
    }

    impl RemoveContainer for FakeRuntime {
        async fn remove(&self, target: &str) {
            let mut state = self.0.lock().unwrap();
            state.asked_to_remove.push(target.to_string());
            state
                .running
                .retain(|(name, id)| *name != target && *id != target);
        }
    }

    /// A connector that records that it was asked and then fails, so the test
    /// never gets as far as a runtime, fake or real.
    async fn refuse(asked: &AtomicBool) -> Result<FakeRuntime, OxyError> {
        asked.store(true, Ordering::SeqCst);
        Err(OxyError::InitializationError(
            "no container runtime in unit tests".to_string(),
        ))
    }

    #[tokio::test]
    async fn shutdown_cleanup_that_owns_nothing_reports_not_owned_and_never_connects() {
        let asked = AtomicBool::new(false);

        let outcome = cleanup_owned_with(Vec::new(), || refuse(&asked)).await;

        assert_eq!(outcome, ShutdownCleanup::NotOwned);
        assert!(
            !asked.load(Ordering::SeqCst),
            "a process that created no container built a Docker client on shutdown — that \
             is how stopping a plain `oxy serve` removed another server's oxy-postgres"
        );
    }

    /// The control for the test above: the injected connector IS what the owned
    /// path calls, so "never connects" is a statement about the early return and
    /// not about a connector nobody uses.
    #[tokio::test]
    async fn shutdown_cleanup_that_owns_a_container_does_reach_for_docker() {
        let asked = AtomicBool::new(false);

        let outcome = cleanup_owned_with(vec![A_POSTGRES_ID.to_string()], || refuse(&asked)).await;

        assert!(asked.load(Ordering::SeqCst));
        assert_eq!(outcome, ShutdownCleanup::DockerUnavailable);
    }

    /// The shipped entry point in a process that started nothing — the plain
    /// `oxy serve` case. It reads the process-wide [`OWNED`], so the precondition
    /// is asserted rather than assumed.
    #[tokio::test]
    async fn shutdown_cleanup_in_a_process_that_started_no_container_is_not_owned() {
        assert!(
            OWNED.snapshot().is_empty(),
            "the process-wide registry already holds {:?}: another test in this binary \
             created a container through `start_postgres_container` / \
             `start_clickhouse_container`. This test is only meaningful in a process that \
             started none — and a lib unit test must not need Docker at all.",
            OWNED.snapshot()
        );

        assert_eq!(cleanup_owned_containers().await, ShutdownCleanup::NotOwned);
    }

    #[test]
    fn a_container_is_owned_once_recorded_and_only_once() {
        let owned = OwnedContainers::new();
        assert!(owned.snapshot().is_empty());

        owned.record("postgres-id".to_string());
        owned.record("clickhouse-id".to_string());
        owned.record("postgres-id".to_string());

        assert_eq!(owned.snapshot(), vec!["postgres-id", "clickhouse-id"]);
    }

    /// Start versus start. Process A's `oxy start` created `oxy-postgres`; process
    /// B's `oxy start` then force-removed it on startup and created its own under
    /// the same fixed NAME. When A shuts down, its claim is stale: it must ask for
    /// the ID it was given — a 404 no-op in the real runtime — and never for the
    /// name, which now belongs to B.
    #[tokio::test]
    async fn a_stale_claim_cannot_remove_the_container_that_now_holds_the_name() {
        let owned_by_a = OwnedContainers::new();
        owned_by_a.record(A_POSTGRES_ID.to_string());
        let runtime = FakeRuntime::running(&[(POSTGRES_CONTAINER_NAME, B_POSTGRES_ID)]);

        let outcome =
            cleanup_owned_with(owned_by_a.snapshot(), || async { Ok(runtime.clone()) }).await;

        assert_eq!(runtime.asked_to_remove(), vec![A_POSTGRES_ID]);
        for name in [POSTGRES_CONTAINER_NAME, CLICKHOUSE_CONTAINER_NAME] {
            assert!(
                !runtime.asked_to_remove().iter().any(|asked| asked == name),
                "shutdown asked the runtime to remove `{name}` BY NAME — that takes whatever \
                 container holds the name now, not the one this process created"
            );
        }
        assert_eq!(
            runtime.still_running(),
            vec![B_POSTGRES_ID],
            "process A's shutdown removed process B's container"
        );
        assert_eq!(
            outcome,
            ShutdownCleanup::Removed(vec![A_POSTGRES_ID.to_string()])
        );
    }

    /// `oxy start`'s teardown order is unchanged: what it created, in the order it
    /// created them — Postgres, then ClickHouse.
    #[tokio::test]
    async fn owned_containers_are_removed_in_the_order_they_were_created() {
        let owned = OwnedContainers::new();
        owned.record("postgres-id".to_string());
        owned.record("clickhouse-id".to_string());
        let runtime = FakeRuntime::running(&[
            (POSTGRES_CONTAINER_NAME, "postgres-id"),
            (CLICKHOUSE_CONTAINER_NAME, "clickhouse-id"),
        ]);

        cleanup_owned_with(owned.snapshot(), || async { Ok(runtime.clone()) }).await;

        assert_eq!(
            runtime.asked_to_remove(),
            vec!["postgres-id", "clickhouse-id"]
        );
        assert!(runtime.still_running().is_empty());
    }

    /// `src` with `//` comments — whole-line and trailing — and every whitespace
    /// character removed. A `//` inside a string literal (a URL) is kept: string
    /// state is tracked across lines, since a message can open on one line and
    /// carry `postgresql://…` on the next. Assumes what holds for the file scanned
    /// here: no block comments and no raw string ending in a backslash. Twin of
    /// `code` in `crates/app/src/cli/commands/serve_shutdown_tests.rs`.
    fn code(src: &str) -> String {
        let chars: Vec<char> = src.chars().collect();
        let mut out = String::with_capacity(src.len());
        let (mut i, mut in_string) = (0, false);
        while i < chars.len() {
            let (c, next) = (chars[i], chars.get(i + 1).copied());
            if in_string && c == '\\' {
                // An escape: keep both characters, so `\"` cannot close the string.
                out.push(c);
                out.extend(next.filter(|n| !n.is_whitespace()));
                i += 2;
            } else if !in_string && c == '/' && next == Some('/') {
                // A comment runs to the end of its line, quotes and apostrophes included.
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            } else if !in_string && c == '\'' && next == Some('"') {
                // The char literal `'"'` is not a string delimiter.
                out.push_str("'\"");
                i += 2;
            } else {
                in_string ^= c == '"';
                if !c.is_whitespace() {
                    out.push(c);
                }
                i += 1;
            }
        }
        assert!(!in_string, "the scan lost track of string literals");
        out
    }

    #[test]
    fn the_scan_drops_comments_but_keeps_a_double_slash_inside_a_string() {
        assert_eq!(code("a(); // b()\n// c()\nd();"), "a();d();");
        assert_eq!(code("x(\"http://h\"); // y()"), "x(\"http://h\");");
        assert_eq!(
            code("m(\"one\\n\\\n   pg://u\"); z();"),
            "m(\"one\\n\\pg://u\");z();"
        );
        assert_eq!(code("k = 1; // \"\\0xy\" don't\nn();"), "k=1;n();");
        assert_eq!(code("q('\"'); // r()\ns();"), "q('\"');s();");
    }

    /// The other half of the contract, which no Docker-free test can run:
    /// `oxy start` still removes its containers on Ctrl-C only because both start
    /// functions claim what they create, and that claim is only safe because it
    /// is the ID from the create response. Drop a `record` and that container
    /// silently outlives every `oxy start`; record the name constant instead and
    /// a stale claim removes another process's container.
    #[test]
    fn both_start_functions_claim_the_container_they_create() {
        let docker = code(include_str!("../docker.rs"));
        const CLAIM: &str = "ownership::OWNED.record(created.id);";

        for start_fn in [
            "fnstart_postgres_container(",
            "fnstart_clickhouse_container(",
        ] {
            let body = &docker[docker.find(start_fn).expect(start_fn)..];
            let created = body
                .find("letcreated=docker.create_container(")
                .unwrap_or_else(|| panic!("`{start_fn}…)` no longer keeps its create response"));
            let started = body.find(".start_container(").expect("a start call");
            assert!(
                created < started && body[created..started].contains(CLAIM),
                "`{start_fn}…)` must claim the container right after creating it, with \
                 `{CLAIM}` — the ID from the create response, not the container's name"
            );
        }

        for name in ["POSTGRES_CONTAINER_NAME", "CLICKHOUSE_CONTAINER_NAME"] {
            assert!(
                !docker.contains(&format!("OWNED.record({name}")),
                "`docker.rs` claims `{name}` by NAME again. A name outlives its container: \
                 after a second `oxy start` replaces it, this process's shutdown would remove \
                 the other process's container"
            );
        }
    }
}
