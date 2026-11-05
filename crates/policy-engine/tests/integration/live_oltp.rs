//! The same read and sync against a real org OLTP database (Neon) — skipped
//! unless `POLICY_OLTP_DSN` is set, so CI and a laptop without credentials
//! pass. The DSN is an app writer's, as `oxy oltp dsn` prints it, and the app's
//! schema must already hold `policy_overrides`. Read-only: it syncs to a temp
//! file and, if `POLICY_OLTP_POLICY` names a row, decides one SKU with it.
//!
//! ```sh
//! POLICY_OLTP_DSN="$(oxy oltp dsn --org <org> --role app:<slug>)" \
//! POLICY_OLTP_POLICY=spring_sale \
//!   cargo nextest run -p oxy-policy-engine --test integration -E 'test(live_oltp)' --no-capture
//! ```

use std::path::Path;

use oxy_policy_engine::{OltpWriter, PolicyEngine, PolicySource, sync};

#[tokio::test]
async fn live_oltp_read_and_sync() {
    let Ok(dsn) = std::env::var("POLICY_OLTP_DSN") else {
        eprintln!("POLICY_OLTP_DSN is not set; skipping the live OLTP check");
        return;
    };
    let writer = OltpWriter::from_dsn(&dsn).expect("an app writer's DSN");
    let out = std::env::temp_dir().join(format!("oxy-policy-oltp-{}.parquet", std::process::id()));
    let (w, o) = (writer.clone(), out.clone());
    let rows = tokio::task::spawn_blocking(move || sync::sync_overrides(&w, &o))
        .await
        .expect("join")
        .expect("syncs from the OLTP database");
    eprintln!(
        "synced {rows} row(s) of {}.policy_overrides to {}",
        writer.schema(),
        out.display()
    );

    if let Ok(policy) = std::env::var("POLICY_OLTP_POLICY") {
        let source = PolicySource::connect(&writer)
            .await
            .expect("connect as the writer");
        let mut engine = PolicyEngine::boot(Path::new(crate::common::POLICY_JS), source)
            .await
            .expect("boots");
        let decision = engine.decide(&policy, "SKU-1001").await.expect("decides");
        eprintln!("{decision:?}");
    }
}
