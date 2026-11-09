//! Boundary test: background work does not creep back into the request path.
//!
//! ## Why a source-scanning test
//!
//! A `tokio::spawn` beside a handler is always the shortest path to "do this after
//! the response", and nothing objects to it at review time unless the reviewer already
//! knows what it costs on a fleet: the work dies with the pod that took the request
//! (a `Cloning` row that never finishes), or runs once per replica when it was meant
//! to run once (a seat sync, a sweep), and nothing can poll or retry it. The rule that
//! replaces it — long-running or fire-and-forget work is a `TaskSpec` on
//! `agentic_task_queue` (`crates/app/.claude/skills/oxy-task-spec-default`) — has been
//! written down since the fleet existed and was still being bypassed one reasonable
//! shortcut at a time. So the objection is made mechanical: a spawn in a file this
//! test does not list fails the build.
//!
//! ## The allowlist is the backlog, not an exemption list
//!
//! Every entry says what the spawn does and why it is still here. Most fall into
//! three kinds, and the reason names which:
//!
//! - **a queue executor's own task** — the `TaskExecutor` impl that *is* the queued
//!   path (`run_job` under a `tokio::spawn` inside `execute`). That is where a spawn
//!   belongs; the entry stays as long as the executor does.
//! - **a request's own SSE pump** — work that only feeds this request's stream and
//!   whose result reaches nobody else. The skill admits it ("per-request streaming
//!   aggregation that ends when the SSE connection closes"), and the entry has to
//!   SAY so, which is the brief's one hard requirement on the reason text.
//! - **`backlog:`** — work that outlives the request or belongs on exactly one pod.
//!   These are the entries that should disappear, each as a `TaskSpec` with an
//!   executor registered in `router/recovery.rs` and a line in
//!   `internal-docs/task-spec-registry.md`; the list going up is the signal.
//!
//! Adding an entry means editing this file and writing the reason down. That is the
//! point: it turns an invisible shortcut into a reviewable act.
//!
//! ## Scope and precision
//!
//! The walk covers `crates/app/src/server/api/**`, `crates/app/src/integrations/**`
//! (Slack and eval request paths) and every `crates/api-*/src/**`: the handler
//! layer and the surface crates `oxy-server` mounts. Test code is out:
//! files under a `tests/` directory, `tests.rs` and `*_tests.rs` modules, the few
//! `#[cfg(test)] mod x;` files named in [`TEST_ONLY_FILES`], and every
//! `#[cfg(test)] mod … { … }` block (cut on rustfmt's column-0 `}`), so a test that
//! spawns its own fixture does not flag. The rule is deliberately that narrow: a
//! production handler can be *about* testing (`test_notification.rs`,
//! `platform/self_test.rs`, `test_run.rs`) and must still be scanned. Comment lines
//! are dropped too, so prose about a spawn is not one. Only the two literal call
//! shapes are matched; `spawn_blocking` is a thread-pool hop for CPU work inside one
//! request and is not this rule's subject.

use std::fs;
use std::path::{Path, PathBuf};

/// The call shapes that put work on the runtime behind a handler's back.
const NEEDLES: &[&str] = &["tokio::spawn(", "tokio::task::spawn("];

/// Files permitted to spawn, and why. Paths are relative to the workspace `crates/`
/// root and so are crate-qualified (`app/src/…`, `api-tenancy/src/…`), one entry per
/// file. A reason that begins `backlog:` is work that should become a `TaskSpec`.
///
/// **Shrinking this list is the migration.** Before adding to it, check you are not
/// taking the shortcut this test exists to object to.
const ALLOWED: &[(&str, &str)] = &[
    // ---- oxy-app: integrations (Slack, evals) — request paths outside src/server ----
    (
        "app/src/integrations/eval/builders/mod.rs",
        "the CLI eval runner (`oxy test`): runs the eval and drains its event \
         buffer to the terminal handler, both awaited before it returns. No HTTP \
         request and no fleet; the work ends with the command",
    ),
    (
        "app/src/integrations/eval/builders/target_agentic.rs",
        "only pumps this run's own event channel: tallies token usage off the \
         agent's events and is awaited before the target returns",
    ),
    (
        "app/src/integrations/slack/events/agentic_bridge.rs",
        "only pumps this run's own event channel: translates the agent's events \
         into renderer items and is awaited before the run returns",
    ),
    (
        "app/src/integrations/slack/events/execution.rs",
        "drives one Slack agent run beside the renderer that drains its channel, \
         awaited in the same function. The run reads its agent by ref from the \
         compiled revision, so it is pod-agnostic; the backlog is the caller's \
         detach (below), not this",
    ),
    (
        "app/src/integrations/slack/webhooks/events.rs",
        "backlog: answers Slack's 3-second ack and runs the event (an agent run) \
         after the response. A pod that dies mid-run loses that reply; a \
         TaskSpec would make it durable. `seen_events` dedups Slack's retries",
    ),
    (
        "app/src/integrations/slack/webhooks/handlers/pick_workspace.rs",
        "backlog: the workspace-picker button acks, then runs the held question \
         after the response. Same shape and fix as `webhooks/events.rs`",
    ),
    (
        "app/src/integrations/slack/webhooks/handlers/submit_workspace_picker.rs",
        "backlog: the picker modal's submit acks, then dispatches the held \
         question after the response. Same shape and fix as `webhooks/events.rs`",
    ),
    (
        "app/src/integrations/slack/webhooks/handlers/view_sql_artifacts.rs",
        "backlog: the View-SQL button acks, then uploads the run's SQL files to \
         the thread. Best effort, re-clickable; a TaskSpec would make it durable",
    ),
    (
        "app/src/integrations/slack/socket_mode/client.rs",
        "backlog: socket mode (local development) acks each envelope on the \
         socket, then dispatches it. Per-process by nature, like the socket; \
         the dispatched run is the same backlog as `webhooks/events.rs`",
    ),
    // ---- oxy-app: the handler layer ----
    (
        "app/src/server/api/auth/ops.rs",
        "backlog: sends the magic-link email after the response; a pod that dies here \
         loses one email and the person requests another. A TaskSpec would make the \
         send durable and retryable",
    ),
    (
        "app/src/server/api/billing/notify.rs",
        "backlog: sends the past-due email off the billing tick. Same shape as the \
         magic link: one email, best effort, should be a durable delivery task",
    ),
    (
        "app/src/server/api/chat/delivery.rs",
        "process-wide chat LISTEN/NOTIFY listener, started once per process behind a \
         OnceLock, plus the tokio_postgres connection driver its notifications arrive \
         through. Per-process by nature (it feeds this pod's in-memory subscribers), \
         not a request's work and not queueable",
    ),
    (
        "app/src/server/api/database.rs",
        "only pumps this request's own SSE stream: the Add-database connection probe \
         (and its two Snowflake SSO-URL relays) and the onboarding schema inspection \
         send progress and a final result over the response's channel and persist \
         nothing; a dropped stream wastes at most one probe. The temporary secret is \
         cleaned up by a guard inside the same task",
    ),
    (
        "app/src/server/api/metric_anomalies/scan.rs",
        "backlog: a manually fired monitor scan runs in the request's process, writes \
         its run row's final status itself and answers over a oneshot the handler \
         waits ~55 s on. The run row exists already; the scan should be the \
         scheduled monitor-scan TaskSpec enqueued with trigger=manual",
    ),
    (
        "app/src/server/api/modeling.rs",
        "only pumps this request's own SSE stream (an Airform run/compile/test's \
         progress events). The whole modeling surface is being deleted by a sibling \
         PR; the entry goes with the file — see the missing-file rule in \
         `every_allowlist_entry_still_covers_a_spawn`",
    ),
    (
        "app/src/server/api/notifications/settings/summary_task.rs",
        "a queue executor's own task: `QuietSummaryExecutor::execute` runs the summary \
         under a spawn and reports through the task's channels",
    ),
    (
        "app/src/server/api/notifications/task.rs",
        "a queue executor's own task: `NotificationDeliverExecutor::execute` delivers \
         under a spawn and reports through the task's channels",
    ),
    (
        "app/src/server/api/projects/agent_ask/start.rs",
        "drives a custom-app ask in the request's process when `AskAdmission` refuses \
         the queue (an app's staging host); every other ask is queued. Documented in \
         worker-fleet.md § Agent asks; the staging case is the remaining backlog",
    ),
    (
        "app/src/server/api/projects/automation_run.rs",
        "backlog: `spawn_periodic_sweep`, a ten-minute cleanup loop started at boot on \
         every pod. Idempotent, so duplicates only race, but it is a racy singleton \
         (worker-fleet.md) that belongs on the scheduled queue",
    ),
    (
        "app/src/server/api/projects/automation_run/executor.rs",
        "a queue executor's own task: `custom_app_procedure_run` runs its job under a \
         spawn bound to the custom-app Sentry hub",
    ),
    (
        "app/src/server/api/projects/automation_run/executor/heartbeat.rs",
        "a claimed task's heartbeat ticker, beside the executor that owns the claim. \
         Lives exactly as long as the claim does; stopping it is the executor's job",
    ),
    (
        "app/src/server/api/schedules.rs",
        "backlog: a manual `run_now` of a monitor scan or a workspace health check \
         drives the run in this process, registered in the handler's RuntimeState for \
         cancel and SSE. The scheduled firing of the same work is already a queued \
         task; the manual fire should enqueue the same TaskSpec",
    ),
    (
        "app/src/server/api/user_tokens/device_login.rs",
        "sub-second fire-and-forget off the request's clock: `tell_approver` counts \
         the pending device logins and queues the notification through \
         `ctx.notify`'s delivery task — the durable part is already on the queue; the \
         spawn only keeps the start's timing constant",
    ),
    (
        "app/src/server/api/video.rs",
        "backlog: the UniFi camera watcher, a polling loop started at boot on every \
         pod with the API key. Runs once per replica when it should run once; a \
         scheduled TaskSpec on the singleton role is the fix",
    ),
    (
        "app/src/server/api/world_model.rs",
        "process-lifetime loops started once by `api_router` behind OnceLocks, not a \
         request's work: the outbox writer draining this pod's publish channel into \
         `world_model_events`, the tailer fanning new rows onto this pod's in-memory \
         buses (per pod by definition), and the retention reaper. The reaper is \
         idempotent so duplicates only race, but it is the one of the three that \
         could be a scheduled TaskSpec: backlog",
    ),
    (
        "app/src/server/api/world_model_graph/handlers.rs",
        "only pumps this request's own SSE stream: filter counts, instance detail and \
         measure breakdown each run their warehouse queries concurrently inside one \
         spawned task and send every result over the response's channel; nothing is \
         persisted and the task ends with the last query",
    ),
    // ---- api-tenancy ----
    (
        "api-tenancy/src/admin/orgs.rs",
        "backlog: sends the first owner's invitation email after the org is created. \
         The invitation row is committed; the send is best effort and should be a \
         durable delivery task",
    ),
    (
        "api-tenancy/src/onboarding/handlers.rs",
        "backlog: clones the repository after `create_workspace_from_repo` answers, \
         because a large clone outlives the request timeout. A pod that dies mid-clone \
         leaves a permanent `Cloning` row — the failure worker-fleet.md § Racy \
         singletons names. Should be a TaskSpec with the workspace id as its key",
    ),
    (
        "api-tenancy/src/organizations/invitation_handlers.rs",
        "backlog: two invitation-email sends and a Stripe `sync_seats` after accepting \
         an invitation. `sync_seats` is on worker-fleet.md's racy-singletons list; the \
         reconciliation loop is its safety net today",
    ),
    (
        "api-tenancy/src/organizations/member_handlers.rs",
        "backlog: Stripe `sync_seats` after removing a member — the same best-effort \
         pattern as `accept_invitation`, same fix",
    ),
    (
        "api-tenancy/src/partner_console/orgs.rs",
        "backlog: sends the invitation email for the org a partner just created; the \
         row is committed and a send failure never fails the request",
    ),
    (
        "api-tenancy/src/partner_console/write.rs",
        "backlog: sends an invitation email from the partner console, same shape as \
         `partner_console/orgs.rs`",
    ),
    (
        "api-tenancy/src/workspace_provisioning.rs",
        "detaches the Airhouse tenant provision from the request's timeout so the \
         remote create and the local `airhouse_tenants` insert always finish together \
         (a half-done pair leaves a tenant no later `ensure_tenant` can adopt). The \
         request waits on it, so it is not fire-and-forget; a timeout hands the rest \
         to first-need provisioning",
    ),
    // ---- api-custom-apps ----
    (
        "api-custom-apps/src/admin/apps/storage.rs",
        "backlog: the storage sweep an admin triggers, guarded by a process-local \
         SWEEP_IN_FLIGHT so one pod runs one at a time — which is exactly the \
         per-process guard a queue's claim would replace",
    ),
    (
        "api-custom-apps/src/app_function_executor.rs",
        "a queue executor's own task: the function job runs under a spawn bound to \
         the custom-app Sentry hub",
    ),
    (
        "api-custom-apps/src/custom_apps_env_branch/queue.rs",
        "a queue executor's own task: `EnvBranchExecutor::execute`",
    ),
    (
        "api-custom-apps/src/custom_apps_functions/failure_page.rs",
        "a deliberate, documented departure at the site: one capped post whose \
         delivery the claim row already tracks; a replica that dies here leaves the \
         claim unsent, it goes stale and the next failure sends it",
    ),
    (
        "api-custom-apps/src/custom_apps_functions/host/held_log.rs",
        "detached write of a staging invocation's held side effects, so the record \
         lands even when the caller's future is dropped mid-await. Sub-second, one row",
    ),
    (
        "api-custom-apps/src/custom_apps_functions/mod.rs",
        "the invocation's cancellation watchdog: polls `cancel_requested_at` for the \
         life of one function invocation and ends with it",
    ),
    (
        "api-custom-apps/src/custom_apps_functions/runtime/host_call_task.rs",
        "one host call dispatched off the isolate thread, for the invocation that made \
         it; ends when the call answers",
    ),
    (
        "api-custom-apps/src/custom_apps_migrations/airhouse.rs",
        "the tokio_postgres connection driver for the migration's own client; ends \
         when the client is dropped at the end of the apply",
    ),
    (
        "api-custom-apps/src/custom_apps_nonproduction/staging_task_executor.rs",
        "a queue executor's own task: `StagingMigrationsExecutor::execute`",
    ),
    (
        "api-custom-apps/src/custom_apps_sandboxes/maintenance.rs",
        "backlog: the detached sandbox-maintenance loop started at boot; it enqueues \
         the teardowns (which are queued tasks) but the tick itself runs on every pod",
    ),
    (
        "api-custom-apps/src/custom_apps_sandboxes/migrations_task.rs",
        "a queue executor's own task: `SandboxMigrationsExecutor::execute`",
    ),
    (
        "api-custom-apps/src/custom_apps_sandboxes/oltp_task.rs",
        "a queue executor's own task: `SandboxOltpExecutor::execute`",
    ),
    (
        "api-custom-apps/src/custom_apps_sandboxes/teardown_executor.rs",
        "a queue executor's own task: `SandboxTeardownExecutor::execute`",
    ),
    (
        "api-custom-apps/src/custom_apps_tail/listener.rs",
        "process-wide LISTEN connection for the function log tail, started once, plus \
         its connection driver. Per-process by nature: it feeds this pod's streams",
    ),
    (
        "api-custom-apps/src/custom_apps_tail/producer.rs",
        "only pumps this request's own SSE stream: follows one invocation's log for \
         the stream that opened it, under that request's hub",
    ),
    (
        "api-custom-apps/src/custom_apps_tail/watchers.rs",
        "holds the watcher row's lease for as long as the request's stream is open; \
         the renewals belong to that request and stop with it",
    ),
    (
        "api-custom-apps/src/custom_apps_usage_backfill.rs",
        "`spawn_sweep` runs only on the singleton role and only queues tasks; the \
         second spawn is the queue executor's own task (`UsageBackfillExecutor`)",
    ),
    // ---- api-experiment ----
    (
        "api-experiment/src/experiment/check/executor.rs",
        "a queue executor's own task: `ExperimentCheckExecutor::execute`",
    ),
    (
        "api-experiment/src/experiment/estimate/mod.rs",
        "a queue executor's own task: `ExperimentEstimateExecutor::execute`",
    ),
    (
        "api-experiment/src/experiment/executor.rs",
        "a queue executor's own task: `ExperimentPriceExecutor::execute`",
    ),
    (
        "api-experiment/src/experiment/interim/mod.rs",
        "a queue executor's own task: `ExperimentInterimExecutor::execute`",
    ),
    (
        "api-experiment/src/simulation/mod.rs",
        "a queue executor's own task: `SimulationTaskExecutor::execute`",
    ),
];

/// The `crates/` root. `CARGO_MANIFEST_DIR` is `crates/app`.
fn crates_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/app has a parent")
}

/// The trees this rule covers: the handler layer and every surface crate.
fn scan_roots() -> Vec<PathBuf> {
    let crates = crates_root();
    // `app/src/integrations` holds the Slack and eval request paths, which
    // sat outside the handler layer and so outside this rule.
    let mut roots = vec![
        crates.join("app/src/server/api"),
        crates.join("app/src/integrations"),
    ];
    let Ok(entries) = fs::read_dir(crates) else {
        return roots;
    };
    let mut surfaces: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("api-"))
        })
        .map(|p| p.join("src"))
        .filter(|p| p.is_dir())
        .collect();
    surfaces.sort();
    roots.extend(surfaces);
    roots
}

/// Every `*.rs` under `dir`, recursively.
fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// `path` relative to the `crates/` root, `/`-separated — the form `ALLOWED` is written in.
fn rel_of(path: &Path) -> String {
    path.strip_prefix(crates_root())
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}

/// `source` with every `#[cfg(test)] mod … { … }` block and every comment line
/// blanked (not removed, so line numbers in a report still point at the file).
/// rustfmt closes a top-level module with a `}` in column 0, and that is the cut.
fn production_lines(source: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut lines = source.lines().peekable();
    while let Some(line) = lines.next() {
        if line == "#[cfg(test)]"
            && lines
                .peek()
                .is_some_and(|next| next.starts_with("mod ") && next.ends_with('{'))
        {
            out.push("");
            for inner in lines.by_ref() {
                out.push("");
                if inner == "}" {
                    break;
                }
            }
            continue;
        }
        out.push(if line.trim_start().starts_with("//") {
            ""
        } else {
            line
        });
    }
    out
}

/// Production spawn sites in `path`: `(line number, line text)`.
fn spawn_sites(path: &Path) -> Vec<(usize, String)> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    production_lines(&text)
        .iter()
        .enumerate()
        .filter(|(_, line)| NEEDLES.iter().any(|n| line.contains(*n)))
        .map(|(i, line)| (i + 1, line.trim().to_string()))
        .collect()
}

/// Files that compile only under `cfg(test)` without saying so in their name: each
/// is declared `#[cfg(test)] mod x;` by its parent. Named here rather than matched
/// by a looser pattern, so a production file that merely mentions testing in its
/// name (`test_notification.rs`, `self_test.rs`) stays in scope.
const TEST_ONLY_FILES: &[&str] = &[
    "api-custom-apps/src/custom_apps_functions/runtime/test_calls.rs",
    "api-custom-apps/src/custom_apps_functions/sdk_testing_drift.rs",
];

/// A file that only ever compiles under `cfg(test)`: anything under a `tests/`
/// directory, a `tests.rs` / `*_tests.rs` module, or one of [`TEST_ONLY_FILES`].
fn is_test_file(rel: &str) -> bool {
    let mut parts = rel.split('/').peekable();
    let mut name = "";
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            name = part;
        } else if part == "tests" {
            return true;
        }
    }
    let stem = name.trim_end_matches(".rs");
    stem == "tests" || stem.ends_with("_tests") || TEST_ONLY_FILES.contains(&rel)
}

/// The files this rule reads: production sources under the scan roots.
fn scanned_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for root in scan_roots() {
        rust_sources(&root, &mut files);
    }
    files.retain(|f| !is_test_file(&rel_of(f)));
    files.sort();
    files
}

/// An entry nothing under it still needs is not backlog, it is an open door: the
/// file keeps its exemption for whatever is written there next, and the list stops
/// saying what is left to migrate. So every entry must still cover a spawn, and one
/// that does not is removed in the change that made it unnecessary.
///
/// A listed file that no longer exists is tolerated, and said so on stderr: an
/// entry's file can be deleted by one PR while the entry lives in another, and a
/// build that fails on the merge order of two unrelated PRs protects nothing. The
/// entry is dead until someone removes it; a new file at that exact path would
/// inherit it, which is why this is a note and not silence.
#[test]
fn every_allowlist_entry_still_covers_a_spawn() {
    let mut stale = Vec::new();
    for (allowed, _) in ALLOWED {
        let path = crates_root().join(allowed);
        if !path.exists() {
            eprintln!(
                "handler_spawns: ALLOWED lists crates/{allowed}, which no longer exists; \
                 remove the entry"
            );
            continue;
        }
        if spawn_sites(&path).is_empty() {
            stale.push(*allowed);
        }
    }
    assert!(
        stale.is_empty(),
        "ALLOWED names {} entr(ies) whose file no longer spawns anything: {stale:?}. \
         Remove them — an exemption nothing needs is one the next shortcut inherits.",
        stale.len()
    );
}

#[test]
fn allowlist_entries_are_files_named_once() {
    let mut seen = std::collections::HashSet::new();
    for (allowed, reason) in ALLOWED {
        assert!(seen.insert(*allowed), "ALLOWED names {allowed} twice");
        assert!(
            allowed.ends_with(".rs"),
            "ALLOWED entries are files, one per file, not prefixes: {allowed}"
        );
        assert!(
            reason.len() > 40,
            "the reason for {allowed} is too short to be one: say what the spawn does \
             and why it is still here"
        );
    }
}

#[test]
fn handlers_do_not_spawn_outside_the_allowlist() {
    let files = scanned_files();
    // 300 is a floor, not a target: ~740 today. It only has to stay below the real
    // count, so a surface that moves out of this workspace lowers it rather than
    // chasing it upward.
    assert!(
        files.len() > 300,
        "expected to scan the handler layer and every api-* crate, found only {} files — \
         the walk is broken, and a boundary test that silently scans nothing is worse \
         than no test",
        files.len()
    );
    assert!(
        files
            .iter()
            .any(|f| rel_of(f).starts_with("app/src/server/api/")),
        "walk did not cover oxy-app's handler layer"
    );
    assert!(
        files.iter().any(|f| rel_of(f).starts_with("api-")),
        "walk did not cover any api-* surface crate"
    );
    assert!(
        files
            .iter()
            .any(|f| rel_of(f).starts_with("app/src/integrations/slack/")),
        "walk did not cover the Slack request path"
    );

    let mut violations = Vec::new();
    for path in &files {
        let rel = rel_of(path);
        if ALLOWED.iter().any(|(allowed, _)| *allowed == rel) {
            continue;
        }
        for (line, text) in spawn_sites(path) {
            violations.push(format!("  crates/{rel}:{line}\n    found: {text}"));
        }
    }

    assert!(
        violations.is_empty(),
        "\n{} spawn site(s) in the request path outside the allowlist:\n\n{}\n\n\
         Long-running or fire-and-forget work in a handler is a `TaskSpec` on \
         `agentic_task_queue`, not a `tokio::spawn`: a spawn dies with the pod that \
         took the request, runs once per replica when it was meant to run once, and \
         nothing can poll or retry it. Invoke the `oxy-task-spec-default` skill \
         (`crates/app/.claude/skills/`): add the kind, its executor (registered in \
         `router/recovery.rs`), an idempotency key, and its line in \
         `internal-docs/task-spec-registry.md`, then enqueue from the handler.\n\n\
         A spawn that only drives this request's own SSE stream — feeds the response's \
         channel, persists nothing, ends when the queries do — is admitted, but its \
         ALLOWED entry in tests/routing/handler_spawns.rs must SAY so. A queue \
         executor's own task says that too. Writing the reason down is the cost of \
         admission.\n",
        violations.len(),
        violations.join("\n\n")
    );
}

#[cfg(test)]
mod scanner_tests {
    use super::*;

    #[test]
    fn a_test_module_and_comments_are_cut_but_lines_keep_their_numbers() {
        let src = "fn a() {\n    tokio::spawn(x);\n}\n// tokio::spawn(in a comment)\n\
                   #[cfg(test)]\nmod tests {\n    fn t() { tokio::spawn(y); }\n}\n\
                   fn b() { tokio::task::spawn(z); }\n";
        let lines = production_lines(src);
        assert_eq!(lines.len(), src.lines().count());
        let hits: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| NEEDLES.iter().any(|n| l.contains(*n)))
            .map(|(i, _)| i + 1)
            .collect();
        assert_eq!(hits, vec![2, 9]);
    }

    #[test]
    fn only_real_test_files_are_out_of_scope() {
        for test in [
            "app/src/server/api/foo/tests.rs",
            "app/src/server/api/foo/bar_tests.rs",
            "app/src/server/api/foo/tests/cases.rs",
            "api-custom-apps/src/custom_apps_functions/runtime/test_calls.rs",
        ] {
            assert!(is_test_file(test), "{test} is test code");
        }
        // A handler that is *about* testing is production code and stays scanned.
        for production in [
            "api-custom-apps/src/admin/apps/test_notification.rs",
            "app/src/server/api/notifications/platform/self_test.rs",
            "app/src/server/api/test_run.rs",
            "app/src/server/api/contests.rs",
        ] {
            assert!(!is_test_file(production), "{production} is production code");
        }
    }
}
