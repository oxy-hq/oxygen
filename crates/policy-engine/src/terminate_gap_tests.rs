//! The V8 terminate gap, closed: a `decide` stuck in an optimised allocating
//! loop stops when another thread terminates the isolate.
//!
//! On V8 15.0 (deno_core 0.412/0.413) and 15.2, when Turboshaft unrolls
//! `for (;;) keep.push(new Array(64).fill(i))` the optimised code carries no
//! interrupt check, so `terminate_execution` never lands and the isolate's
//! thread spins forever. `--no-turboshaft-loop-unrolling`, set process-wide by
//! `v8_platform::init` before the first isolate, closes it. The same gap and
//! fix in the custom-app function runtime:
//! `api-custom-apps/src/custom_apps_functions/runtime/terminate_gap_tests.rs`.
//!
//! The case runs in a **child process** — this test binary re-invoked on
//! [`terminate_gap_child`] — because a regression leaves a thread spinning
//! that nothing in-process can stop, and V8 flags are process-wide.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::{PolicyEngine, PolicySource};

/// The loop, with memory bounded so the heap never nears its ceiling: the only
/// way out is a terminate that lands.
const BOUNDED: &str = r#"export async function decide(sku, policyId) {
    const keep = [];
    for (let i = 0;; i++) {
        keep.push(new Array(64).fill(i));
        if (keep.length > 20000) keep.length = 0;
    }
}
"#;

/// Selects the child half; unset in a normal run.
const CHILD_ENV: &str = "OXY_POLICY_TERMINATE_GAP_CHILD";
/// Prefix of every line the child reports on stdout.
const REPORT: &str = "TERMINATE_GAP";

/// How long the loop runs before it is terminated — long enough for TurboFan
/// to optimise it, which is when the gap opens.
const TERMINATE_AFTER: Duration = Duration::from_secs(1);
/// How long a terminate may take to land once sent.
const LANDING: Duration = Duration::from_secs(5);
/// Past `TERMINATE_AFTER + LANDING` with room for the child's own startup.
const CHILD_DEADLINE: Duration = Duration::from_secs(40);

/// What the child process did.
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

fn run_child() -> Child {
    let mut child = Command::new(std::env::current_exe().expect("test binary path"))
        .args([&child_test_name(), "--exact", "--ignored", "--nocapture"])
        .env(CHILD_ENV, "bounded")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the child test process");
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

/// A `decide` spinning in an allocating loop stops when the isolate is
/// terminated from another thread: the decision fails as a script error
/// shortly after the terminate, rather than the thread spinning until the
/// process is killed.
#[test]
fn allocating_loop_in_decide_stops_when_terminated() {
    let child = run_child();
    let why = child.describe();
    assert!(
        child.status.is_some_and(|s| s.success()),
        "child did not finish — the terminate never landed and the isolate is \
         still spinning: {why}"
    );
    assert_eq!(child.get("booted"), Some("1"), "{why}");
    assert!(
        child
            .get("result")
            .is_some_and(|r| r.starts_with("Err(Script")),
        "decide should fail as a terminated script: {why}"
    );
    let elapsed: u128 = child
        .get("elapsed_ms")
        .and_then(|v| v.parse().ok())
        .expect(&why);
    assert!(
        elapsed < (TERMINATE_AFTER + LANDING).as_millis(),
        "the terminate took {elapsed} ms to land: {why}"
    );
}

/// The child half. A no-op unless [`CHILD_ENV`] is set, which only
/// [`run_child`] does.
#[test]
#[ignore = "the child process of the terminate-gap test; it runs it itself"]
fn terminate_gap_child() {
    if std::env::var(CHILD_ENV).is_err() {
        return;
    }
    let script = write_script();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(child_body(&script));
    let _ = std::fs::remove_file(&script);
    // A spinning isolate would never get here; exit without unwinding it.
    std::process::exit(0);
}

async fn child_body(script: &std::path::Path) {
    let mut engine = PolicyEngine::boot(script, PolicySource::Mock)
        .await
        .expect("boots");
    println!("{REPORT} booted=1");
    let handle = engine.isolate_handle();
    let started = Instant::now();
    std::thread::spawn(move || {
        std::thread::sleep(TERMINATE_AFTER);
        handle.terminate_execution();
    });
    let result = engine.decide("mock", "SKU-1").await;
    println!("{REPORT} elapsed_ms={}", started.elapsed().as_millis());
    let shown = match &result {
        Ok(d) => format!("Ok({})", d.variant),
        Err(e) => format!("Err({e:?})"),
    };
    println!("{REPORT} result={shown}");
}

fn write_script() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "oxy-policy-{}-terminate-gap.js",
        std::process::id()
    ));
    std::fs::write(&path, BOUNDED).expect("temp dir is writable");
    path
}
