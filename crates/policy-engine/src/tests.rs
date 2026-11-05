//! Phase 1's acceptance tests: the script receives the mock policy, routes as
//! an independent implementation predicts, writes through the audit op, is
//! refused at the edge when it writes junk, and serves many decisions from one
//! isolate without the heap growing.

use std::path::{Path, PathBuf};

use crate::{EngineError, PolicyDecision, PolicyEngine, PolicySource};

const POLICY_JS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/js/policy.js");
const SDK_JS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/js/sdk.js");

/// The mock policy's variants as `(name, weight, multiplier)`, restated here
/// rather than read from the host so the test checks the script against an
/// independent source.
const VARIANTS: [(&str, u32, f64); 3] = [
    ("control", 50, 1.0),
    ("discount_10", 30, 0.9),
    ("premium_5", 20, 1.05),
];

/// FNV-1a over UTF-16 code units — what `OxyMath.hashVariant` claims to be.
fn fnv1a_utf16(s: &str) -> u32 {
    s.encode_utf16().fold(0x811c_9dc5, |hash, unit| {
        (hash ^ u32::from(unit)).wrapping_mul(0x0100_0193)
    })
}

fn expected(sku: &str) -> PolicyDecision {
    let total: u32 = VARIANTS.iter().map(|v| v.1).sum();
    let mut bucket = i64::from(fnv1a_utf16(sku) % total);
    let (name, _, multiplier) = VARIANTS
        .iter()
        .find(|v| {
            bucket -= i64::from(v.1);
            bucket < 0
        })
        .expect("weights cover every bucket");
    PolicyDecision {
        policy_id: "mock".into(),
        sku: sku.into(),
        variant: (*name).into(),
        price_multiplier: *multiplier,
    }
}

/// A one-off policy script under the system temp dir, importing the compiled SDK.
fn script(name: &str, body: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("oxy-policy-{}-{name}.js", std::process::id()));
    let sdk = deno_core::ModuleSpecifier::from_file_path(Path::new(SDK_JS)).expect("absolute path");
    std::fs::write(
        &path,
        format!("import {{ OxyAudit, OxyContext }} from \"{sdk}\";\n{body}"),
    )
    .expect("temp dir is writable");
    path
}

#[tokio::test]
async fn the_script_routes_each_sku_as_an_independent_implementation_predicts() {
    let mut engine = PolicyEngine::boot(Path::new(POLICY_JS), PolicySource::Mock)
        .await
        .expect("boots");
    let skus = [
        "SKU-1001",
        "SKU-1002",
        "SKU-2048",
        "SKU-9999",
        "ünïcödé-SKU",
        "",
    ];
    let mut returned = Vec::new();
    for sku in skus {
        returned.push(engine.decide("mock", sku).await.expect("decides"));
    }
    let predicted: Vec<_> = skus.iter().map(|s| expected(s)).collect();
    assert_eq!(
        returned, predicted,
        "TS routing disagrees with the Rust restatement"
    );
    assert_eq!(
        engine.audit_log(),
        returned,
        "every decision was written through the op"
    );
}

#[tokio::test]
async fn routing_spreads_skus_across_every_variant() {
    let mut engine = PolicyEngine::boot(Path::new(POLICY_JS), PolicySource::Mock)
        .await
        .expect("boots");
    let mut seen = std::collections::BTreeSet::new();
    for i in 0..200 {
        seen.insert(
            engine
                .decide("mock", &format!("SKU-{i}"))
                .await
                .expect("decides")
                .variant,
        );
    }
    let all: std::collections::BTreeSet<_> = VARIANTS.iter().map(|v| v.0.to_string()).collect();
    assert_eq!(seen, all);
}

#[tokio::test]
async fn a_malformed_decision_is_refused_by_the_write_op() {
    let path = script(
        "malformed",
        r#"export function decide(sku) {
            const d = { policyId: "mock", sku, variant: "control", priceMultiplier: 1, extra: true };
            OxyAudit.record(d);
            return d;
        }"#,
    );
    let mut engine = PolicyEngine::boot(&path, PolicySource::Mock)
        .await
        .expect("boots");
    let err = engine
        .decide("mock", "SKU-1")
        .await
        .expect_err("the write op refuses it");
    assert!(
        matches!(&err, EngineError::Script(m) if m.contains("invalid policy decision") && m.contains("extra")),
        "{err}"
    );
    assert!(
        engine.audit_log().is_empty(),
        "a refused write leaves no row"
    );
}

#[tokio::test]
async fn an_unknown_policy_throws_in_the_script() {
    let path = script(
        "unknown",
        r#"export function decide(sku) { return OxyContext.getPolicy("nope"); }"#,
    );
    let mut engine = PolicyEngine::boot(&path, PolicySource::Mock)
        .await
        .expect("boots");
    let err = engine
        .decide("mock", "SKU-1")
        .await
        .expect_err("no such policy");
    assert!(
        matches!(&err, EngineError::Script(m) if m.contains("unknown policy: nope")),
        "{err}"
    );
}

#[tokio::test]
async fn a_script_without_a_decide_export_fails_at_boot() {
    let path = script("no-export", "export const notDecide = 1;");
    let err = PolicyEngine::boot(&path, PolicySource::Mock)
        .await
        .err()
        .expect("refused at boot");
    assert!(
        matches!(&err, EngineError::Boot(m) if m.contains("no `decide`")),
        "{err}"
    );
}

/// Phase 1's "no memory leaks" claim, measured: after a warm-up, 20,000 more
/// decisions in the same isolate must not grow the heap V8 retains after a full
/// collection. The audit log grows in Rust memory, outside this measure, by
/// design; what is checked is that nothing crossing the boundary is retained
/// on the JavaScript side.
#[tokio::test]
async fn the_isolate_heap_does_not_grow_with_decisions() {
    let mut engine = PolicyEngine::boot(Path::new(POLICY_JS), PolicySource::Mock)
        .await
        .expect("boots");
    for i in 0..2_000 {
        engine
            .decide("mock", &format!("WARM-{i}"))
            .await
            .expect("decides");
    }
    let before = engine.retained_heap_bytes();
    for i in 0..20_000 {
        engine
            .decide("mock", &format!("LOAD-{i}"))
            .await
            .expect("decides");
    }
    let after = engine.retained_heap_bytes();
    let growth = after.saturating_sub(before);
    assert!(
        growth < 256 * 1024,
        "retained heap grew {growth} bytes over 20,000 decisions ({before} → {after})"
    );
}

#[tokio::test]
async fn a_typescript_script_is_refused_with_the_build_step_named() {
    let path = std::env::temp_dir().join(format!("oxy-policy-{}-raw.ts", std::process::id()));
    std::fs::write(&path, "export function decide(sku: string) { return sku; }")
        .expect("temp dir is writable");
    let err = PolicyEngine::boot(&path, PolicySource::Mock)
        .await
        .err()
        .expect("refused at boot");
    assert!(
        matches!(&err, EngineError::Boot(m) if m.contains("JavaScript only") && m.contains("build-js.sh")),
        "{err}"
    );
}
