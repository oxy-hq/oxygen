//! Every command that installs the span layer must drain it.
//!
//! `main.rs` installs the `SpanCollectorLayer` for its server commands and
//! parks the receiver of an UNBOUNDED channel; only [`super::finalize`] ever
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
        include_str!("../cli/commands/serve.rs"),
        "fn start_server_and_web_app(",
        "observability_boot::finalize().await",
    ),
    (
        "start",
        include_str!("../cli/commands/start.rs"),
        "fn start_database_and_server(",
        "start_server_and_web_app(",
    ),
    (
        "worker",
        include_str!("../cli/commands/worker.rs"),
        "fn run_worker(",
        "observability_boot::finalize().await",
    ),
];

const MAIN_RS: &str = include_str!("../../../server/src/main.rs");

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
    let body = fn_body(include_str!("../cli/commands/worker.rs"), "fn run_worker(")
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
/// make a guard stricter. Same shape as `role_manifest_tests::strip_line_comments`.
fn strip_line_comments(src: &str) -> String {
    src.lines()
        .map(|l| l.find("//").map_or(l, |i| &l[..i]))
        .collect::<Vec<_>>()
        .join("\n")
}
