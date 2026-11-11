//! Source scans of the long-lived commands' boot paths. Each asserts a call
//! that must sit on a boot path, which needs a database to run but which the
//! source states directly. They lived in `oxy-app` beside what they guard
//! until the commands moved here.
//!
//! # The span channel
//!
//! `main.rs` installs the `SpanCollectorLayer` for its server commands and
//! parks the receiver of an UNBOUNDED channel; only `oxy_app::observability_boot::finalize` ever
//! reads it. A command in that list without a `finalize` keeps every span it
//! closes for as long as it runs. `oxy worker` shipped exactly that way: its
//! latency worker closes a `driver_tick` span every poll, and prod worker pods
//! grew ~10 MiB/h until the 1 GiB limit OOM-killed them — while every custom-app
//! function it ran recorded no event and no log line, because `finalize` is
//! also what installs those sinks.
//!
//! Source scans, like `every_long_running_entry_point_declares_its_role`: the
//! entry points need a database to run, and the property is "this call is on
//! the boot path", which the source states directly.

/// The server commands, and where each one's boot reaches `finalize`.
///
/// `start` has no call of its own: it boots Postgres (and ClickHouse) and then
/// runs `oxy serve`'s boot in-process, so its row asserts that delegation and
/// the `serve` row covers the call.
const DRAINS: &[(&str, &str, &str, &str)] = &[
    (
        "serve",
        include_str!("serve.rs"),
        "fn start_server_and_web_app(",
        "observability_boot::finalize().await",
    ),
    (
        "start",
        include_str!("start.rs"),
        "fn start_database_and_server(",
        "start_server_and_web_app(",
    ),
    (
        "worker",
        include_str!("worker.rs"),
        "fn run_worker(",
        "observability_boot::finalize().await",
    ),
];

const MAIN_RS: &str = include_str!("../main.rs");

#[test]
fn every_command_that_collects_spans_drains_them() {
    let collecting = server_commands(MAIN_RS);
    let mut draining: Vec<String> = DRAINS.iter().map(|(cmd, ..)| cmd.to_string()).collect();
    draining.sort_unstable();
    assert_eq!(
        collecting, draining,
        "`main.rs`'s `server_command` decides which commands get the span layer; \
         every one of them needs a row here naming where its boot calls \
         `observability_boot::finalize`, or its span channel is never read"
    );

    for (command, src, entry, call) in DRAINS {
        let body = fn_body(src, entry)
            .unwrap_or_else(|| panic!("`oxy {command}`: `{entry}` not found — update DRAINS"));
        assert!(
            body.contains(call),
            "`oxy {command}` installs the span layer but its boot (`{entry}`) never \
             reaches `{call}`: the channel is unbounded and nothing drains it, so the \
             process holds every span it closes, and custom-app events and logs from \
             it are dropped"
        );
    }
}

/// Where in `run_worker` matters too: `finalize` installs the custom-app sinks,
/// and `record_event` is a silent no-op until it has. The runtime and the run
/// drivers are what execute function invocations on a worker.
#[test]
fn the_worker_drains_before_it_can_run_anything() {
    let body = fn_body(include_str!("worker.rs"), "fn run_worker(")
        .expect("worker.rs has no `fn run_worker(`");
    let at = |needle: &str| {
        body.find(needle)
            .unwrap_or_else(|| panic!("`{needle}` not in run_worker"))
    };
    let finalize = at("observability_boot::finalize().await");
    assert!(
        finalize < at("WorkerRuntime::start(") && finalize < at("spawn_run_drivers("),
        "`run_worker` must call `observability_boot::finalize` before it starts the \
         runtime and the run drivers, or the first invocations record nothing"
    );
}

/// The commands `server_command` matches, sorted: the string literals of its
/// `matches!(*a, "serve" | …)`.
fn server_commands(main_rs: &str) -> Vec<String> {
    let body = fn_body(main_rs, "fn server_command(").expect("main.rs has no `fn server_command(`");
    let arm = body
        .split_once("matches!(")
        .and_then(|(_, rest)| rest.split_once(')'))
        .map(|(arm, _)| arm)
        .expect("`server_command` no longer uses `matches!` — update this parser");
    let mut commands: Vec<String> = arm
        .split('"')
        .skip(1)
        .step_by(2)
        .map(String::from)
        .collect();
    assert!(!commands.is_empty(), "parsed no commands from `{arm}`");
    commands.sort_unstable();
    commands
}

/// The text of the top-level `fn` whose signature starts with `signature`, up to
/// its closing brace in column 0 — comments stripped, so a guard cannot be
/// satisfied by prose about the call (both `worker.rs` and `serve.rs` explain
/// `finalize` in comments right next to it).
fn fn_body(src: &str, signature: &str) -> Option<String> {
    let code = strip_line_comments(src);
    let start = code.find(signature)?;
    let len = code[start..].find("\n}").unwrap_or(code.len() - start);
    Some(code[start..start + len].to_string())
}

/// Drop `//` comments. Crude — it does not understand strings or block
/// comments — and the failure direction is safe: stripping too much can only
/// make a guard stricter.
fn strip_line_comments(src: &str) -> String {
    src.lines()
        .map(|l| l.find("//").map_or(l, |i| &l[..i]))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every process that serves requests or drains work must declare its role.
///
/// The failure mode is the reason this is a source scan rather than a call
/// test: `current_process_role()` falls back to `Role::All` when nothing set
/// the `OnceLock`, and `All` is the value that means "this node owns the
/// workspace files". So omitting the call is silent — the process comes up and
/// claims a filesystem it does not have.
///
/// `oxy worker` did exactly that. Three separate reads take the wrong branch on
/// a standalone worker as a result: `OxyProjectContext::context_root` globs an
/// absent working copy instead of materialising the boundary (the "no databases
/// configured" failure its own comment names), the workspace middleware
/// publishes the `WorkingCopy` extension, and a missing root reads as
/// "materializing" rather than as a worker's normal state.
///
/// A new long-running entry point belongs in this list.
#[test]
fn every_long_running_entry_point_declares_its_role() {
    // A prefix, so `_with_default` counts: `oxy worker` declares `Worker` when
    // `OXY_ROLE` is unset, and matching the bare `()` form would have made this
    // guard fail for a call that is MORE correct than the one it was written
    // against.
    const INIT: &str = "init_process_role_from_env";
    for (command, src) in [
        ("oxy serve", include_str!("serve.rs")),
        ("oxy worker", include_str!("worker.rs")),
    ] {
        // Comments stripped first. `worker.rs` carries a NOTE block explaining
        // why a SECOND bare init must never come back, and that prose contains
        // this very literal — so without stripping, deleting the real call
        // would leave this guard green. A test satisfied by a comment about
        // the thing is not a test of the thing.
        let src = &strip_line_comments(src);
        assert!(
            src.contains(INIT),
            "`{command}` never calls `{INIT}`, so `current_process_role()` \
             answers `All` for it — the role that owns the workspace files. \
             Nothing fails loudly; the pod just starts describing itself wrong.",
        );
    }
}

/// `oxy worker` must seed the role EXACTLY once, and with an explicit default.
///
/// The bug this pins actually shipped: main's #2822 added
/// `_with_default(Worker)` while this branch had already added a bare
/// `init_process_role_from_env()` below it. They do not conflict textually, so
/// the merge kept both — and the second is not idempotent. `PROCESS_ROLE` is a
/// `OnceLock` so the role stays `Worker`, but `set_process_owns_workspace_files`
/// is an `AtomicBool::store`, and the bare form resolves `Role::All` when
/// `OXY_ROLE` is unset, storing `true` over the `false` written moments before.
///
/// The pod then answers `process_can_compile() == false` (reads the `OnceLock`)
/// and `process_owns_workspace_files() == true` (reads the atomic) — the exact
/// split-brain the role manifest exists to prevent, and worse than either end
/// alone because the two disagree.
#[test]
fn oxy_worker_seeds_the_role_once_and_never_with_the_bare_default() {
    let src = strip_line_comments(include_str!("worker.rs"));
    assert!(
        src.contains("init_process_role_from_env_with_default("),
        "`oxy worker` must declare its role explicitly; running the command IS \
         the declaration."
    );
    // The bare form, as a call. `_with_default(` does not match this because
    // the next char after the fn name is `_`, not `(`.
    assert!(
        !src.contains("init_process_role_from_env()"),
        "`oxy worker` calls the bare `init_process_role_from_env()` as well as \
         `_with_default`. That is not a harmless duplicate: the bare form \
         resolves `Role::All` when `OXY_ROLE` is unset, and its \
         `set_process_owns_workspace_files(true)` overwrites the `false` the \
         explicit call just stored — leaving the process claiming a workspace \
         working copy it does not have."
    );
}
