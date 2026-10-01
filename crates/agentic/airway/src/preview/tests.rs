//! I4 (no production QuickBooks token is read or rotated) at the spec, and the
//! submit rules.

use chrono::{Duration, TimeZone, Utc};
use serde_json::json;

use super::*;
use crate::AirwayAdmission;
use crate::config::AirwayPipelineSpec;
use airway::connector::Environment;

const KEY: &str = "feat_qb_v2_92a1b7";

fn quickbooks_yaml(extra: &str) -> String {
    format!(
        "name: quickbooks_financials_eastbay
source:
  kind: quickbooks
  config:
    client_id: PROD_CLIENT_ID
    client_secret_var: QB_CLIENT_SECRET
    refresh_token_var: QB_REFRESH_TOKEN__EASTBAY
    realm_id: 9341456860808037
{extra}destination:
  database: airhouse
  dataset_name: quickbooks_eastbay
  schema_separator: ___
allow_concurrent_runs: true
"
    )
}

fn spec(yaml: &str) -> AirwayPipelineSpec {
    AirwayPipelineSpec::from_yaml_str(yaml).expect("the fixture parses")
}

fn sandbox() -> SandboxSource {
    SandboxSource {
        realm_id: "4620816365000000".into(),
        refresh_token_var: Some("QB_SANDBOX_REFRESH_TOKEN__EASTBAY".into()),
        access_token_var: None,
        client_id: None,
        client_id_var: Some("QB_SANDBOX_CLIENT_ID".into()),
        client_secret_var: Some("QB_SANDBOX_CLIENT_SECRET".into()),
    }
}

/// A windowed (QuickBooks) sample: a week.
fn sample(sandbox: Option<SandboxSource>) -> PreviewSample {
    let to = Utc.with_ymd_and_hms(2026, 9, 27, 0, 0, 0).unwrap();
    PreviewSample {
        preview_key: KEY.into(),
        window: Some(SampleWindow {
            from: to - Duration::days(7),
            to,
        }),
        resources: vec![],
        sandbox,
    }
}

#[test]
fn quickbooks_without_sandbox_source_is_refused() {
    let mut s = spec(&quickbooks_yaml(""));
    let before = serde_json::to_value(&s).unwrap();
    let mut admission = AirwayAdmission::default();
    let err = sample(None).apply(&mut s, &mut admission).unwrap_err();
    assert_eq!(err.code(), "sandbox_required", "{err}");
    assert_eq!(
        serde_json::to_value(&s).unwrap(),
        before,
        "a refusal leaves the spec untouched"
    );
    assert_eq!(admission, AirwayAdmission::default());
}

#[test]
fn quickbooks_swaps_realm_and_vars() {
    let mut s = spec(&quickbooks_yaml(""));
    let mut admission = AirwayAdmission::default();
    sample(Some(sandbox()))
        .apply(&mut s, &mut admission)
        .unwrap();
    let config = &s.source.config;
    assert_eq!(config["realm_id"], "4620816365000000");
    assert_eq!(
        config["refresh_token_var"],
        "QB_SANDBOX_REFRESH_TOKEN__EASTBAY"
    );
    assert_eq!(config["client_secret_var"], "QB_SANDBOX_CLIENT_SECRET");
    assert_eq!(config["client_id_var"], "QB_SANDBOX_CLIENT_ID");
    assert!(
        config.get("client_id").is_none(),
        "the production client id is replaced"
    );
    let text = config.to_string();
    for production in [
        "QB_CLIENT_SECRET\"",
        "QB_REFRESH_TOKEN__EASTBAY",
        "9341456860808037",
        "PROD_CLIENT_ID",
    ] {
        assert!(!text.contains(production), "{production} survived: {text}");
    }

    // Without a client id override the branch's own (an identifier) stays,
    // and a read-only sandbox names no refresh token at all.
    let mut s = spec(&quickbooks_yaml(""));
    let read_only = SandboxSource {
        refresh_token_var: None,
        access_token_var: Some("QB_SANDBOX_ACCESS_TOKEN".into()),
        client_id_var: None,
        client_secret_var: None,
        ..sandbox()
    };
    sample(Some(read_only))
        .apply(&mut s, &mut AirwayAdmission::default())
        .unwrap();
    let config = &s.source.config;
    assert_eq!(config["client_id"], "PROD_CLIENT_ID");
    assert_eq!(config["access_token_var"], "QB_SANDBOX_ACCESS_TOKEN");
    for gone in ["refresh_token_var", "client_secret_var"] {
        assert!(config.get(gone).is_none(), "{gone}: {config}");
    }
}

#[test]
fn explicit_base_url_is_refused_in_a_sample() {
    let yaml = quickbooks_yaml("    base_url: https://quickbooks.api.intuit.com\n");
    let mut s = spec(&yaml);
    let err = sample(Some(sandbox()))
        .apply(&mut s, &mut AirwayAdmission::default())
        .unwrap_err();
    assert_eq!(err, SampleRefusal::ExplicitBaseUrl);
    assert_eq!(err.code(), "sample_refused");
    assert_eq!(s.name, "quickbooks_financials_eastbay", "untouched");
}

#[test]
fn environment_is_sandbox_for_rotate_on_use() {
    let mut admission = AirwayAdmission::default();
    sample(Some(sandbox()))
        .apply(&mut spec(&quickbooks_yaml("")), &mut admission)
        .unwrap();
    assert_eq!(admission.environment, Environment::Sandbox);

    // The control: a source that does not rotate keeps its admission.
    let mut admission = AirwayAdmission::default();
    fs_sample(Some(sandbox()))
        .apply(&mut spec(&filesystem_yaml()), &mut admission)
        .unwrap();
    assert_eq!(admission.environment, Environment::Production);
}

/// A capped (filesystem) sample of its one resource.
fn fs_sample(sandbox: Option<SandboxSource>) -> PreviewSample {
    PreviewSample {
        window: None,
        resources: vec!["users".into()],
        ..sample(sandbox)
    }
}

fn filesystem_yaml() -> String {
    "name: users
source:
  kind: filesystem
  config:
    base_path: /tmp/x
    pattern: \"*.jsonl\"
    format: jsonl
    table_name: users
destination:
  kind: memory
  config:
    dataset_name: scratch
"
    .into()
}

#[test]
fn the_sample_runs_under_its_own_single_flight_name() {
    let mut s = spec(&quickbooks_yaml(""));
    let applied = sample(Some(sandbox()))
        .apply(&mut s, &mut AirwayAdmission::default())
        .unwrap();
    assert_eq!(applied.live_name, "quickbooks_financials_eastbay");
    assert_eq!(
        applied.name,
        format!("preview:{KEY}:quickbooks_financials_eastbay")
    );
    assert_eq!(s.name, applied.name);
    assert!(
        !s.allow_concurrent_runs,
        "a sample is single-flight whatever the YAML says"
    );
    let crate::DestinationSpec::Reference(reference) = &s.destination else {
        panic!("still a reference");
    };
    assert_eq!(reference.schema_separator, None);
    assert_eq!(
        reference.dataset_name, "quickbooks_eastbay",
        "the host maps it"
    );
    assert!(
        s.validate().is_err(),
        "the scoped name is one no authored YAML may carry"
    );
}

#[test]
fn refused_kinds_and_inline_destinations_are_refused_at_claim_too() {
    for kind in SAMPLE_REFUSED_KINDS {
        let mut s = spec(&filesystem_yaml());
        s.source.kind = kind.into();
        let err = fs_sample(None)
            .apply(&mut s, &mut AirwayAdmission::default())
            .unwrap_err();
        assert_eq!(err.code(), "sample_refused", "{kind}");
    }
    let yaml = filesystem_yaml().replace(
        "  kind: memory\n  config:\n    dataset_name: scratch",
        "  kind: postgres\n  config:\n    connection_string: postgres://prod/db\n    dataset_name: raw",
    );
    let err = fs_sample(None)
        .apply(&mut spec(&yaml), &mut AirwayAdmission::default())
        .unwrap_err();
    assert!(
        matches!(err, SampleRefusal::InlineDestination { .. }),
        "{err}"
    );
}

#[test]
fn a_sandbox_must_be_well_formed() {
    let both = SandboxSource {
        access_token_var: Some("A".into()),
        ..sandbox()
    };
    assert!(both.validate().is_err());
    let no_secret = SandboxSource {
        client_secret_var: None,
        ..sandbox()
    };
    assert!(
        no_secret.validate().is_err(),
        "a rotating grant needs its secret"
    );
    let odd = SandboxSource {
        refresh_token_var: Some("QB REFRESH; DROP".into()),
        ..sandbox()
    };
    assert!(odd.validate().is_err());
    // The production rotator's own vars are app-scoped (`apps/<app_id>/…`,
    // Pokehouse's `refresh-qb-token` Function): never a sandbox's.
    for app_scoped in [
        "apps/5b7e0c55-1f2a-4f7e-9d1c-7a0b1c2d3e4f/QB_REFRESH_TOKEN_EASTBAY",
        "team/QB_REFRESH",
    ] {
        let rotator = SandboxSource {
            refresh_token_var: Some(app_scoped.into()),
            ..sandbox()
        };
        let err = rotator.validate().unwrap_err();
        assert!(err.contains("app-scoped"), "{app_scoped}: {err}");
    }
    assert!(sandbox().validate().is_ok());
    assert_eq!(
        sandbox().rotating_var(),
        Some("QB_SANDBOX_REFRESH_TOKEN__EASTBAY")
    );
    let parsed: Result<SandboxSource, _> =
        serde_json::from_value(json!({ "realm_id": "1", "refresh_token": "a-literal-token" }));
    assert!(parsed.is_err(), "a secret value is never a field");
}

// ── SamplePolicy ────────────────────────────────────────────────────────────

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 27, 12, 0, 0).unwrap()
}

fn ask<'a>(
    kind: &'a str,
    resources: &'a [String],
    advertised: Option<&'a [String]>,
) -> SampleAsk<'a> {
    SampleAsk {
        pipeline: "p",
        kind,
        window: None,
        resources,
        advertised,
        has_sandbox: true,
        now: now(),
    }
}

#[test]
fn refused_kinds_answer_before_anything_else() {
    for kind in SAMPLE_REFUSED_KINDS {
        let err = SamplePolicy::check(&ask(kind, &[], None)).unwrap_err();
        assert_eq!(err.code(), "sample_refused", "{kind}");
    }
    let err = SamplePolicy::check(&SampleAsk {
        has_sandbox: false,
        ..ask("quickbooks", &[], None)
    })
    .unwrap_err();
    assert_eq!(err.code(), "sandbox_required");
}

#[test]
fn a_windowed_sample_defaults_to_a_week_and_caps_at_31_days() {
    let plan = SamplePolicy::check(&ask("toast", &[], None)).unwrap();
    let window = plan.window.expect("a window");
    assert_eq!(window.to - window.from, Duration::days(DEFAULT_WINDOW_DAYS));
    assert_eq!(window.to, now());
    assert!(!plan.wall_clock_capped, "the window bounds it");

    let asked = |from: i64, to: i64| SampleAsk {
        window: Some(RequestedWindow {
            from: Some(now() - Duration::days(from)),
            to: Some(now() - Duration::days(to)),
        }),
        ..ask("quickbooks", &[], None)
    };
    assert!(SamplePolicy::check(&asked(31, 0)).is_ok());
    assert_eq!(
        SamplePolicy::check(&asked(32, 0)).unwrap_err(),
        SampleRefusal::WindowTooLong { days: 32 }
    );
    assert_eq!(
        SamplePolicy::check(&asked(1, 2)).unwrap_err().code(),
        "window_required",
        "inverted"
    );
    let half = SampleAsk {
        window: Some(RequestedWindow {
            from: Some(now()),
            to: None,
        }),
        ..ask("toast", &[], None)
    };
    assert_eq!(
        SamplePolicy::check(&half).unwrap_err().code(),
        "window_required"
    );
}

#[test]
fn a_source_without_a_window_names_its_resources_and_is_capped() {
    let two = ["a".to_string(), "b".to_string()];
    let err = SamplePolicy::check(&ask("rest_api", &[], Some(&two))).unwrap_err();
    assert_eq!(err.code(), "resources_required");
    let err = SamplePolicy::check(&ask("rest_api", &[], None)).unwrap_err();
    assert_eq!(err.code(), "resources_required", "unknown count: name them");

    let named = ["b".to_string()];
    let plan = SamplePolicy::check(&ask("rest_api", &named, Some(&two))).unwrap();
    assert_eq!(plan.resources, named);
    assert!(plan.wall_clock_capped);
    assert_eq!(plan.window, None);

    let one = ["only".to_string()];
    let single = SamplePolicy::check(&ask("rest_api", &[], Some(&one))).unwrap();
    assert_eq!(
        single.resources, one,
        "a single resource is named for the caller, so the claim never meets an empty list"
    );

    let stray = ["c".to_string()];
    let err = SamplePolicy::check(&ask("rest_api", &stray, Some(&two))).unwrap_err();
    assert_eq!(err.code(), "unknown_resource");

    let windowed = SampleAsk {
        window: Some(RequestedWindow::default()),
        ..ask("rest_api", &named, Some(&two))
    };
    assert_eq!(
        SamplePolicy::check(&windowed).unwrap_err().code(),
        "window_not_supported"
    );
}

// ── Fix round 1: claim-time scope, and airway's metadata in `main` ─────────

#[test]
fn a_windowed_sample_needs_its_window_at_claim() {
    let windowless = PreviewSample {
        window: None,
        ..sample(Some(sandbox()))
    };
    let mut s = spec(&quickbooks_yaml(""));
    let err = windowless
        .apply(&mut s, &mut AirwayAdmission::default())
        .unwrap_err();
    assert_eq!(err.code(), "window_required", "{err}");
    assert_eq!(s.name, "quickbooks_financials_eastbay", "untouched");
    let wide = PreviewSample {
        window: Some(SampleWindow {
            from: now() - Duration::days(40),
            to: now(),
        }),
        ..sample(Some(sandbox()))
    };
    let err = wide
        .apply(
            &mut spec(&quickbooks_yaml("")),
            &mut AirwayAdmission::default(),
        )
        .unwrap_err();
    assert_eq!(err.code(), "window_too_long", "{err}");
}

#[test]
fn a_capped_sample_needs_named_resources_it_reads_at_claim() {
    let unnamed = PreviewSample {
        resources: vec![],
        ..fs_sample(None)
    };
    let err = unnamed
        .apply(
            &mut spec(&filesystem_yaml()),
            &mut AirwayAdmission::default(),
        )
        .unwrap_err();
    assert_eq!(err.code(), "resources_required", "{err}");

    let listed = format!("{}resources: [users]\n", filesystem_yaml());
    let stray = PreviewSample {
        resources: vec!["orders".into()],
        ..fs_sample(None)
    };
    let err = stray
        .apply(&mut spec(&listed), &mut AirwayAdmission::default())
        .unwrap_err();
    assert_eq!(err.code(), "unknown_resource", "{err}");
    let windowed = PreviewSample {
        window: sample(None).window,
        ..fs_sample(None)
    };
    let err = windowed
        .apply(
            &mut spec(&filesystem_yaml()),
            &mut AirwayAdmission::default(),
        )
        .unwrap_err();
    assert_eq!(err.code(), "window_not_supported", "{err}");
    // The control: the named resource the pipeline lists.
    fs_sample(None)
        .apply(&mut spec(&listed), &mut AirwayAdmission::default())
        .unwrap();
}

#[test]
fn metadata_in_main_names_replacing_and_watermarked_tables() {
    use crate::{ResourceInfo, WriteDisposition};
    let resource = |name: &str, write_disposition| ResourceInfo {
        name: name.into(),
        description: None,
        write_disposition,
        primary_key: None,
        cursor_field: None,
    };
    let resources = [
        resource("orders", WriteDisposition::Replacing),
        resource("restaurants", WriteDisposition::Merge),
    ];
    let stored: crate::schema_compat::Schema = serde_json::from_value(json!({
        "name": "toast", "version": 1, "version_hash": "", "engine_version": 1,
        "tables": {
            "time_entries": { "name": "time_entries", "columns": {},
                              "write_disposition": "merge", "business_column": "business_date" },
            "orders__checks": { "name": "orders__checks", "columns": {},
                                "write_disposition": "replacing", "parent": "orders" },
            "jobs": { "name": "jobs", "columns": {}, "write_disposition": "merge" } }
    }))
    .unwrap();
    assert_eq!(
        metadata_in_main(&resources, Some(&stored), &[]),
        vec!["orders", "orders__checks", "time_entries"]
    );
    assert_eq!(
        metadata_in_main(&resources, Some(&stored), &["orders".into()]),
        vec!["orders", "orders__checks"],
        "only what the sample reads"
    );
    assert!(
        metadata_in_main(
            &resources,
            Some(&stored),
            &["restaurants".into(), "jobs".into()]
        )
        .is_empty(),
        "merge-only resources sample"
    );
    assert_eq!(
        SampleRefusal::Unsupported {
            tables: vec!["orders".into()]
        }
        .code(),
        "sample_unsupported"
    );
}
