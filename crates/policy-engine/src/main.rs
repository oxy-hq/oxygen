//! `policy-engine` — run the Phase 1 loop end to end.
//!
//! ```text
//! policy-engine [--script <path.js>] [--bench <n>] [SKU ...]
//! ```
//!
//! Boots the policy script (default: `js/policy.js` in this crate, compiled from
//! `ts/policy.ts` by `build-js.sh`), asks it to decide each SKU, and prints every decision next to what the audit op
//! recorded. `--bench <n>` then runs `n` more decisions in the same isolate and
//! reports the time per decision and the retained heap before and after.

use std::path::PathBuf;
use std::time::Instant;

use oxy_policy_engine::PolicyEngine;

const DEFAULT_SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/js/policy.js");
const DEFAULT_SKUS: [&str; 4] = ["SKU-1001", "SKU-1002", "SKU-2048", "SKU-9999"];

struct Args {
    script: PathBuf,
    bench: Option<usize>,
    skus: Vec<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        script: PathBuf::from(DEFAULT_SCRIPT),
        bench: None,
        skus: Vec::new(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--script" => args.script = it.next().ok_or("--script needs a path")?.into(),
            "--bench" => {
                let n = it.next().ok_or("--bench needs a count")?;
                args.bench = Some(n.parse().map_err(|_| format!("not a count: {n}"))?);
            }
            sku => args.skus.push(sku.to_string()),
        }
    }
    if args.skus.is_empty() {
        args.skus = DEFAULT_SKUS.iter().map(|s| s.to_string()).collect();
    }
    Ok(args)
}

async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let mut engine = PolicyEngine::boot(&args.script).await?;
    println!("booted {}", args.script.display());
    for sku in &args.skus {
        let d = engine.decide(sku).await?;
        println!("{sku:<10} → {:<12} ×{:.2}", d.variant, d.price_multiplier);
    }
    let audit = engine.audit_log();
    println!(
        "audit log: {} decision(s) written through op_oxy_write_decision",
        audit.len()
    );

    if let Some(n) = args.bench {
        let before = engine.retained_heap_bytes();
        let start = Instant::now();
        for i in 0..n {
            engine.decide(&format!("BENCH-{i}")).await?;
        }
        let elapsed = start.elapsed();
        let after = engine.retained_heap_bytes();
        println!(
            "bench: {n} decisions in {:.1} ms ({:.2} µs each); retained V8 heap {} → {} bytes",
            elapsed.as_secs_f64() * 1e3,
            elapsed.as_secs_f64() * 1e6 / n as f64,
            before,
            after,
        );
    }
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
    if let Err(e) = rt.block_on(run(args)) {
        eprintln!("policy-engine: {e}");
        std::process::exit(1);
    }
}
