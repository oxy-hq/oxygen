//! `policy-engine` — run the policy loop end to end, or sync the policy store.
//!
//! ```text
//! policy-engine [--oltp-dsn <dsn>] [--policy <id>] [--script <path.js>] [--bench <n>] [SKU ...]
//! policy-engine sync [--oltp-dsn <dsn>] [--out <path.parquet>]
//! ```
//!
//! `--oltp-dsn` (or `POLICY_OLTP_DSN`) is an app writer's DSN for the org's
//! OLTP database, as `oxy oltp dsn --org <org> --role app:<slug>` prints it;
//! `policy_overrides` is read from that app's schema. Without one, policies
//! come from the built-in mock and `--policy` defaults to `mock`.
//!
//! The default form boots the policy script (default: `js/policy.js` in this
//! crate, compiled from `ts/policy.ts` by `build-js.sh`), asks it to decide each
//! SKU under `--policy`, and prints every decision next to what the audit op
//! recorded. `--bench <n>` then runs `n` more decisions in the same isolate and
//! reports the time per decision and the retained heap before and after.
//!
//! `sync` copies `policy_overrides` to a Parquet file (default `local.parquet`)
//! through DuckDB and prints the row count.

use std::path::PathBuf;
use std::time::Instant;

use oxy_policy_engine::{OltpWriter, PolicyEngine, PolicySource, sync};

const DEFAULT_SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/js/policy.js");
const DEFAULT_SKUS: [&str; 4] = ["SKU-1001", "SKU-1002", "SKU-2048", "SKU-9999"];
const OLTP_DSN_ENV: &str = "POLICY_OLTP_DSN";

type BoxError = Box<dyn std::error::Error>;

struct DecideArgs {
    script: PathBuf,
    policy: String,
    bench: Option<usize>,
    skus: Vec<String>,
}

enum Command {
    Decide(DecideArgs),
    Sync { out: PathBuf },
}

struct Args {
    oltp_dsn: Option<String>,
    command: Command,
}

fn parse_args() -> Result<Args, String> {
    let mut it = std::env::args().skip(1).peekable();
    let is_sync = it.peek().map(String::as_str) == Some("sync");
    if is_sync {
        it.next();
    }
    let mut oltp_dsn = None;
    let mut out = None;
    let mut d = DecideArgs {
        script: DEFAULT_SCRIPT.into(),
        policy: "mock".into(),
        bench: None,
        skus: Vec::new(),
    };
    while let Some(arg) = it.next() {
        let mut value = |flag: &str| it.next().ok_or(format!("{flag} needs a value"));
        match arg.as_str() {
            "--oltp-dsn" => oltp_dsn = Some(value("--oltp-dsn")?),
            "--out" if is_sync => out = Some(value("--out")?.into()),
            "--script" if !is_sync => d.script = value("--script")?.into(),
            "--policy" if !is_sync => d.policy = value("--policy")?,
            "--bench" if !is_sync => {
                let n = value("--bench")?;
                d.bench = Some(n.parse().map_err(|_| format!("not a count: {n}"))?);
            }
            flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}")),
            sku if !is_sync => d.skus.push(sku.to_string()),
            other => return Err(format!("sync takes no argument {other}")),
        }
    }
    if d.skus.is_empty() {
        d.skus = DEFAULT_SKUS.iter().map(|s| s.to_string()).collect();
    }
    Ok(Args {
        oltp_dsn: oltp_dsn.or_else(|| std::env::var(OLTP_DSN_ENV).ok()),
        command: if is_sync {
            Command::Sync {
                out: out.unwrap_or_else(|| "local.parquet".into()),
            }
        } else {
            Command::Decide(d)
        },
    })
}

async fn decide(oltp_dsn: Option<String>, d: DecideArgs) -> Result<(), BoxError> {
    let source = match oltp_dsn {
        Some(dsn) => PolicySource::connect(&OltpWriter::from_dsn(&dsn)?).await?,
        None => PolicySource::Mock,
    };
    let mut engine = PolicyEngine::boot(&d.script, source).await?;
    println!("booted {} (policy {})", d.script.display(), d.policy);
    for sku in &d.skus {
        let decision = engine.decide(&d.policy, sku).await?;
        println!(
            "{sku:<10} → {:<12} ×{:.2}",
            decision.variant, decision.price_multiplier
        );
    }
    println!(
        "audit log: {} decision(s) written through op_oxy_write_decision",
        engine.audit_log().len()
    );
    if let Some(n) = d.bench {
        let before = engine.retained_heap_bytes();
        let start = Instant::now();
        for i in 0..n {
            engine.decide(&d.policy, &format!("BENCH-{i}")).await?;
        }
        let elapsed = start.elapsed();
        let after = engine.retained_heap_bytes();
        println!(
            "bench: {n} decisions in {:.1} ms ({:.2} µs each); retained V8 heap {before} → {after} bytes",
            elapsed.as_secs_f64() * 1e3,
            elapsed.as_secs_f64() * 1e6 / n as f64,
        );
    }
    Ok(())
}

async fn sync_command(oltp_dsn: Option<String>, out: PathBuf) -> Result<(), BoxError> {
    let dsn = oltp_dsn.ok_or(format!("sync needs --oltp-dsn or {OLTP_DSN_ENV}"))?;
    let writer = OltpWriter::from_dsn(&dsn)?;
    let started = Instant::now();
    let (w, target) = (writer.clone(), out.clone());
    let rows = tokio::task::spawn_blocking(move || sync::sync_overrides(&w, &target)).await??;
    println!(
        "synced {rows} row(s) of {}.policy_overrides to {} in {:.0} ms",
        writer.schema(),
        out.display(),
        started.elapsed().as_secs_f64() * 1e3
    );
    Ok(())
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("policy-engine: {e}");
            std::process::exit(2);
        }
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread Tokio runtime builds");
    let result = rt.block_on(async {
        match args.command {
            Command::Decide(d) => decide(args.oltp_dsn, d).await,
            Command::Sync { out } => sync_command(args.oltp_dsn, out).await,
        }
    });
    if let Err(e) = result {
        eprintln!("policy-engine: {e}");
        std::process::exit(1);
    }
}
