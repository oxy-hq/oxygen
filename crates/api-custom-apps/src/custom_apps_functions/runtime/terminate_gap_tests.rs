//! The V8 terminate gap, closed: an optimised allocating loop in a function
//! stops like any other.
//!
//! On V8 15.0 (deno_core 0.412/0.413) and 15.2, when Turboshaft unrolls
//! `for (;;) keep.push(new Array(64).fill(i))` the optimised code carries no
//! interrupt check, so neither the wall-clock `terminate_execution` (broker)
//! nor the near-heap-limit callback (`engine/deno.rs`) lands. Without
//! `--no-turboshaft-loop-unrolling` (set process-wide in
//! `engine/deno/v8_platform.rs`) the caller still got `Timeout`, but the
//! isolate thread was abandoned *spinning a core*, still counted live and
//! without its admission permit, and:
//!
//! - **Bounded memory** — it spun until the process exited.
//! - **Unbounded memory** — seconds later, at the default 128 MiB cap, V8
//!   aborted the whole process (`Fatal JavaScript out of memory`): every
//!   tenant on the replica went down.
//!
//! The tests below assert the contract under the production flags; a V8 bump
//! that makes the flag unnecessary must still pass them without it. Found in
//! the decision isolate (`haitrr/decision-engine-phase-2`).
//!
//! Each case runs in a **child process** — this test binary re-invoked on
//! [`terminate_gap_child`] — because a regression either aborts the process or
//! leaves a thread spinning, and V8 flags are process-wide.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use tokio::sync::oneshot;

use super::isolate_limits::TIMEOUT_GRACE;
use super::tests::{MockHost, test_ctx};
use super::warm_pool::WarmPool;
use super::*;

/// The loop, with memory bounded so the heap never nears its ceiling.
const BOUNDED: &str = r#"export default async () => {
    const keep = [];
    for (let i = 0;; i++) {
        keep.push(new Array(64).fill(i));
        if (keep.length > 20000) keep.length = 0;
    }
};"#;

/// The loop as found: it keeps everything, so it runs into the heap ceiling.
const UNBOUNDED: &str = r#"export default async () => {
    const keep = [];
    for (let i = 0;; i++) keep.push(new Array(64).fill(i));
};"#;

const TRIVIAL: &str = "export default async () => Response.json({ ok: true });";

/// What a typical function does with a query result: materialise rows, a JSON
/// round trip, a group-by and a sort.
const ROWS: &str = r#"export default async () => {
    const rows = [];
    for (let i = 0; i < 50000; i++) rows.push({ id: i, store: "s" + (i % 40),
        amount: ((i * 7919) % 1000) / 10, day: "2026-10-" + String(1 + (i % 28)).padStart(2, "0") });
    const parsed = JSON.parse(JSON.stringify(rows));
    const groups = new Map();
    for (const r of parsed) {
        const k = r.store + "|" + r.day;
        const a = groups.get(k) || { n: 0, sum: 0 };
        a.n++; a.sum += r.amount; groups.set(k, a);
    }
    const out = [...groups].map(([k, a]) => ({ k, avg: a.sum / a.n }))
        .sort((x, y) => y.avg - x.avg).slice(0, 50);
    return Response.json(out);
};"#;

/// Tight numeric code, TurboFan's best case and so `--no-turbofan`'s worst.
const COMPUTE: &str = r#"export default async () => {
    let seed = 1, inside = 0;
    for (let i = 0; i < 5000000; i++) {
        seed = (seed * 1103515245 + 12345) % 2147483648; const x = seed / 2147483648;
        seed = (seed * 1103515245 + 12345) % 2147483648; const y = seed / 2147483648;
        if (x * x + y * y < 1) inside++;
    }
    return Response.json({ pi: (4 * inside) / 5000000 });
};"#;

/// The benchmark workloads, by name.
const BENCH: [(&str, &str); 3] = [("trivial", TRIVIAL), ("rows", ROWS), ("compute", COMPUTE)];
const BENCH_RUNS: usize = 15;

/// Selects the child's artifact (`bounded` / `unbounded`); unset in a normal run.
const CHILD_ENV: &str = "OXY_TERMINATE_GAP_CHILD";
/// Space-separated V8 flags the child sets before its first isolate.
const FLAGS_ENV: &str = "OXY_TERMINATE_GAP_V8_FLAGS";
/// Seconds the child stays alive after its report, the stuck isolate with it.
const LINGER_ENV: &str = "OXY_TERMINATE_GAP_LINGER_SECS";
/// Prefix of every line the child reports on stdout.
const REPORT: &str = "TERMINATE_GAP";

const TIMEOUT: Duration = Duration::from_secs(1);
/// Past `TIMEOUT + TIMEOUT_GRACE` with room for the child's own startup.
const CHILD_DEADLINE: Duration = Duration::from_secs(40);

/// What a child process did.
struct Child {
    /// `None` when it was still running at [`CHILD_DEADLINE`] and was killed.
    status: Option<std::process::ExitStatus>,
    stdout: String,
    stderr: String,
}

impl Child {
    /// The value of a `TERMINATE_GAP key=value` line.
    fn get(&self, key: &str) -> Option<&str> {
        let prefix = format!("{REPORT} {key}=");
        // The last report wins: `alive_s` is reported once a second.
        self.stdout
            .lines()
            .rev()
            .find_map(|l| l.strip_prefix(prefix.as_str()))
    }

    fn describe(&self) -> String {
        let tail: String = self
            .stderr
            .lines()
            .rev()
            .take(15)
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "status={:?}\n--- stdout ---\n{}\n--- stderr (last lines, reversed) ---\n{tail}",
            self.status, self.stdout
        )
    }
}

/// The child's libtest name: this module's path without the crate segment.
fn child_test_name() -> String {
    let module = module_path!().split_once("::").map_or("", |(_, rest)| rest);
    format!("{module}::terminate_gap_child")
}

fn run_child(case: &str, heap_mb: &str, v8_flags: Option<&str>) -> Child {
    run_child_lingering(case, heap_mb, v8_flags, 0)
}

fn run_child_lingering(case: &str, heap_mb: &str, v8_flags: Option<&str>, linger_s: u64) -> Child {
    let mut cmd = Command::new(std::env::current_exe().expect("test binary path"));
    cmd.args([&child_test_name(), "--exact", "--ignored", "--nocapture"])
        .env(CHILD_ENV, case)
        .env(HEAP_LIMIT_MB_ENV, heap_mb)
        .env(LINGER_ENV, linger_s.to_string())
        // One permit per org, so the follow-up call on the same org proves
        // whether the stuck isolate still holds one.
        .env(super::super::limits::MAX_ORG_CONCURRENCY_ENV, "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match v8_flags {
        Some(flags) => cmd.env(FLAGS_ENV, flags),
        None => cmd.env_remove(FLAGS_ENV),
    };
    let mut child = cmd.spawn().expect("spawn the child test process");
    let deadline = Instant::now() + CHILD_DEADLINE;
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait on child") {
            break Some(status);
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let (mut stdout, mut stderr) = (String::new(), String::new());
    let _ = child
        .stdout
        .take()
        .map(|mut s| s.read_to_string(&mut stdout));
    let _ = child
        .stderr
        .take()
        .map(|mut s| s.read_to_string(&mut stderr));
    Child {
        status,
        stdout,
        stderr,
    }
}

/// Case (i): with memory bounded, the loop stops at its timeout — the isolate
/// thread exits, nothing is abandoned, and the org's one permit is free again.
#[test]
fn bounded_alloc_loop_stops_at_its_timeout() {
    let child = run_child("bounded", "128", None);
    let why = child.describe();
    assert!(
        child.status.is_some_and(|s| s.success()),
        "child did not finish: {why}"
    );
    assert_eq!(child.get("result"), Some("Err(Timeout)"), "{why}");
    let elapsed: u64 = child
        .get("elapsed_ms")
        .and_then(|v| v.parse().ok())
        .expect(&why);
    assert!(
        elapsed < (TIMEOUT + TIMEOUT_GRACE).as_millis() as u64,
        "Timeout only came back once the grace expired, so the isolate did not \
         stop ({elapsed} ms): {why}"
    );
    assert_eq!(child.get("abandoned"), Some("0"), "{why}");
    assert_eq!(
        child.get("live_after_grace"),
        Some("0"),
        "an isolate thread is still running: {why}"
    );
    assert_eq!(child.get("followup"), Some("Ok(200)"), "{why}");
}

/// Case (ii): at the production default cap (128 MiB) the loop that keeps
/// everything is refused as out of memory, and the process lives on.
#[test]
fn unbounded_alloc_loop_is_refused_without_aborting_the_process() {
    let child = run_child_lingering("unbounded", "128", None, 10);
    let why = child.describe();
    assert!(
        !child.stderr.contains("Fatal JavaScript out of memory"),
        "V8 aborted the process: {why}"
    );
    assert!(
        child.status.is_some_and(|s| s.success()),
        "child did not finish: {why}"
    );
    assert_eq!(child.get("result"), Some("Err(OutOfMemory)"), "{why}");
    assert_eq!(child.get("abandoned"), Some("0"), "{why}");
    assert_eq!(child.get("live_after_grace"), Some("0"), "{why}");
    assert_eq!(child.get("alive_s"), Some("10"), "{why}");
}

/// The flags that close the gap: TurboFan off, or only Turboshaft's loop
/// unrolling off (the one production sets).
const CLOSING_FLAGS: [&str; 2] = ["--no-turbofan", "--no-turboshaft-loop-unrolling"];

/// The control: either closing flag, set explicitly on top of the production
/// flags, stops the same two loops — so `--no-turbofan` remains a fallback if a
/// V8 bump drops the narrower flag.
#[test]
fn both_loops_stop_under_the_closing_flags() {
    for flags in CLOSING_FLAGS {
        let bounded = run_child("bounded", "128", Some(flags));
        let why = format!("{flags}: {}", bounded.describe());
        assert_eq!(bounded.get("result"), Some("Err(Timeout)"), "{why}");
        assert_eq!(bounded.get("abandoned"), Some("0"), "{why}");
        assert_eq!(bounded.get("live_after_grace"), Some("0"), "{why}");

        let unbounded = run_child("unbounded", "32", Some(flags));
        let why = format!("{flags}: {}", unbounded.describe());
        assert!(unbounded.status.is_some_and(|s| s.success()), "{why}");
        assert_eq!(unbounded.get("result"), Some("Err(OutOfMemory)"), "{why}");
    }
}

/// What each closing flag costs a function, end to end: the median of each
/// [`BENCH`] workload with the flag against the production flags. The
/// baseline already carries `--no-turboshaft-loop-unrolling`; its own cost was
/// measured against stock V8 before it shipped (~0%).
#[test]
#[ignore = "a measurement, not a contract: run with --run-ignored only, and read its output"]
fn closing_flags_cost() {
    let base = run_child("bench", "128", None);
    for flags in CLOSING_FLAGS {
        let with = run_child("bench", "128", Some(flags));
        for (name, _) in BENCH {
            let key = format!("bench_{name}_ms");
            let ms = |c: &Child| -> f64 {
                c.get(&key)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| panic!("{key} missing: {}", c.describe()))
            };
            let (a, b) = (ms(&base), ms(&with));
            println!(
                "{flags} {name}: production {a:.1} ms, flag {b:.1} ms ({:+.0}%)",
                (b / a - 1.0) * 100.0
            );
        }
    }
}

/// The child half. A no-op unless [`CHILD_ENV`] is set, which only
/// [`run_child`] does.
#[test]
#[ignore = "the child process of the terminate-gap tests; they run it themselves"]
fn terminate_gap_child() {
    let Ok(case) = std::env::var(CHILD_ENV) else {
        return;
    };
    if let Ok(flags) = std::env::var(FLAGS_ENV) {
        let mut args = vec!["oxy".to_string()];
        args.extend(flags.split_whitespace().map(String::from));
        let unknown = deno_core::v8_set_flags(args);
        assert_eq!(unknown.len(), 1, "V8 refused flags: {unknown:?}");
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    if case == "bench" {
        rt.block_on(bench_body());
        std::process::exit(0);
    }
    let artifact = match case.as_str() {
        "bounded" => BOUNDED,
        "unbounded" => UNBOUNDED,
        other => panic!("unknown case {other}"),
    };
    rt.block_on(child_body(artifact));
    // The abandoned isolate thread would keep the process alive forever.
    std::process::exit(0);
}

async fn child_body(artifact: &str) {
    let pool = WarmPool::new(1);
    let org = uuid::Uuid::new_v4();
    // Warm the pool so the stuck call takes a pool slot, as in production.
    let _ = invoke(&pool, org, TRIVIAL, TIMEOUT).await;
    println!("{REPORT} started=1");
    let started = Instant::now();
    let result = invoke(&pool, org, artifact, TIMEOUT).await;
    println!("{REPORT} result={}", show(&result));
    println!("{REPORT} elapsed_ms={}", started.elapsed().as_millis());
    tokio::time::sleep(Duration::from_secs(1)).await;
    let sources = &oxy_telemetry::metrics::sources::ISOLATES_LIVE;
    println!("{REPORT} abandoned={}", abandoned_isolates());
    println!(
        "{REPORT} live_after_grace={}",
        sources.load(Ordering::Relaxed)
    );
    let followup = invoke(&pool, org, TRIVIAL, Duration::from_secs(10)).await;
    println!("{REPORT} followup={}", show(&followup));
    // Keep the process (and the abandoned isolate) alive a while, so a parent
    // can watch what the stuck isolate does next.
    let linger: u64 = std::env::var(LINGER_ENV)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    for s in 0..linger {
        tokio::time::sleep(Duration::from_secs(1)).await;
        println!("{REPORT} alive_s={}", s + 1);
    }
}

/// Median wall time of each [`BENCH`] workload, end to end through `run_in`.
async fn bench_body() {
    let pool = WarmPool::new(1);
    for (name, artifact) in BENCH {
        let mut ms = Vec::with_capacity(BENCH_RUNS);
        for _ in 0..BENCH_RUNS {
            let started = Instant::now();
            let result = invoke(
                &pool,
                uuid::Uuid::new_v4(),
                artifact,
                Duration::from_secs(60),
            )
            .await;
            assert!(result.is_ok(), "{name}: {}", show(&result));
            ms.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        ms.sort_by(f64::total_cmp);
        println!("{REPORT} bench_{name}_ms={:.2}", ms[BENCH_RUNS / 2]);
    }
}

fn show(result: &Result<FnResponse, RuntimeError>) -> String {
    match result {
        Ok(r) => format!("Ok({})", r.status),
        Err(e) => format!("Err({e:?})"),
    }
}

async fn invoke(
    pool: &Arc<WarmPool>,
    org: uuid::Uuid,
    artifact: &str,
    timeout: Duration,
) -> Result<FnResponse, RuntimeError> {
    let (_cancel_tx, cancel_rx) = oneshot::channel::<()>();
    run_in(
        pool,
        artifact.to_string(),
        org,
        test_ctx(),
        FnRequest::from_body(b"{}".to_vec()),
        Arc::new(MockHost::default()),
        cancel_rx,
        timeout,
        Arc::new(std::sync::Mutex::new(Vec::new())),
        InvocationMeters::start(),
        tracing::Span::none(),
    )
    .await
}
