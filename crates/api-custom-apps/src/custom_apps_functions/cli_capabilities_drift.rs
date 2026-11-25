//! Drift guard between the fail-closed gates in `host.rs` and `oxyc`'s copy
//! of them, `sdk/cli/src/publish/capabilities.ts` — the map `oxyc validate`
//! and `oxyc publish` lint an app's function sources against before a bundle
//! is uploaded.
//!
//! THE HOST IS THE TRUTH; THE CLI IS A COPY. A copy that names a gate the
//! host dropped tells an author to declare a capability nothing reads, and a
//! copy missing a gate the host added lets the exact bug the lint exists for
//! ship again: works locally, `<Area>CapabilityMissing` in prod. Neither side
//! can `use` the other — one is Rust, one is TypeScript — so this holds them
//! together the way `role_manifest_tests.rs` holds the router to its manifest:
//! `include_str!` on both files, comments stripped, two text scans, and a set
//! comparison whose failure message names the field.
//!
//! Text scans, so it needs neither the `custom-app-functions` feature nor a
//! database: it runs in `cargo nextest run -p oxy-api-custom-apps --lib` in every
//! configuration, and `include_str!` makes a rename of either file a compile
//! error rather than a test that quietly scans nothing.

use std::collections::BTreeSet;

const CLI_CAPABILITIES_TS: &str = include_str!("../../../../sdk/cli/src/publish/capabilities.ts");
const HOST_RS: &str = include_str!("host.rs");

/// Fields of `FunctionCapabilities` that are NOT gates: a retention policy, a
/// byte ceiling, the writer an event publish is written as, the build's
/// declared events (an app-level block, not a function's capability), the
/// pipelines an upload may name (its gate is `source_uploads`) and whether the
/// build declares tables under `oltpChanges` (a production `ctx.oltp`
/// transaction is then signed) shape a call that is allowed rather than gate a
/// capability. The TypeScript file carries the same names in its own doc
/// comment, and another non-gate field has to be added here on purpose — the
/// test below fails until it is, which is the point.
pub(super) const NOT_GATES: &[&str] = &[
    "storage_retention",
    "fetch_max_bytes",
    "events_writer",
    "event_catalog",
    "source_upload_pipelines",
    "oltp_changes",
];

/// Strip full-line `//` comments and `/* … */` blocks, trimming what is left.
/// The same conservative, line-oriented shape as `custom_apps_client.rs`: a
/// trailing comment on a line of code is left alone, because deciding where it
/// starts needs a tokenizer, and nothing scanned here puts a field behind one.
pub(super) fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut in_block = false;
    for line in src.lines() {
        let trimmed = line.trim();
        if in_block {
            if let Some((_, rest)) = trimmed.split_once("*/") {
                in_block = false;
                if !rest.trim().is_empty() {
                    out.push_str(rest.trim());
                    out.push('\n');
                }
            }
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        if trimmed.starts_with("/*") {
            if !trimmed.contains("*/") {
                in_block = true;
            }
            continue;
        }
        out.push_str(trimmed);
        out.push('\n');
    }
    out
}

/// Every `"quoted"` string on a line, in order.
pub(super) fn quoted(line: &str) -> Vec<String> {
    line.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

/// The `pub <name>:` fields of `pub struct FunctionCapabilities`, in order.
pub(super) fn host_capability_fields(host: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut in_struct = false;
    for line in host.lines() {
        if line.starts_with("pub struct FunctionCapabilities {") {
            in_struct = true;
            continue;
        }
        if !in_struct {
            continue;
        }
        if line == "}" {
            break;
        }
        if let Some(rest) = line.strip_prefix("pub ") {
            if let Some((name, _)) = rest.split_once(':') {
                fields.push(name.trim().to_string());
            }
        }
    }
    fields
}

/// The ops the host refuses while the `FunctionCapabilities` field `field`
/// is not granted — read off the host-op registry, which is what the broker
/// enforces (`env_policy::HostOp::gated_by`).
pub(super) fn host_ops_gated_by(field: &str) -> BTreeSet<String> {
    super::env_policy::HostOp::gated_by(field)
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// One entry of `GATED_CAPABILITIES` as the text scan sees it.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct CliEntry {
    pub(super) host_field: String,
    pub(super) ops: Vec<String>,
}

/// Every `hostField: "…"` in the TypeScript, with the `ops: […]` that follows
/// it — across lines if the formatter wrapped the array.
pub(super) fn cli_entries(ts: &str) -> Vec<CliEntry> {
    let mut entries: Vec<CliEntry> = Vec::new();
    let mut lines = ts.lines().peekable();
    while let Some(line) = lines.next() {
        if let Some(rest) = line.strip_prefix("hostField:") {
            entries.push(CliEntry {
                host_field: quoted(rest).into_iter().next().unwrap_or_default(),
                ops: Vec::new(),
            });
        } else if let Some(rest) = line.strip_prefix("ops:") {
            let mut array = rest.to_string();
            while !array.contains(']') {
                let Some(more) = lines.next() else { break };
                array.push_str(more);
            }
            if let Some(entry) = entries.last_mut() {
                entry.ops = quoted(&array);
            }
        }
    }
    entries
}

#[test]
fn the_cli_map_names_exactly_the_gates_host_rs_has() {
    let host = strip_comments(HOST_RS);
    let ts = strip_comments(CLI_CAPABILITIES_TS);

    let struct_fields = host_capability_fields(&host);
    assert!(
        struct_fields.len() >= 8,
        "parsed only {} fields from `FunctionCapabilities` — the text scan likely broke; fix it \
         rather than weakening the guard: {struct_fields:?}",
        struct_fields.len()
    );
    for excluded in NOT_GATES {
        assert!(
            struct_fields.iter().any(|f| f == excluded),
            "`{excluded}` is listed as a non-gate here but is no longer a field of \
             `FunctionCapabilities` — drop it from NOT_GATES"
        );
    }
    let gates: BTreeSet<&str> = struct_fields
        .iter()
        .map(String::as_str)
        .filter(|f| !NOT_GATES.contains(f))
        .collect();

    let entries = cli_entries(&ts);
    let cli: BTreeSet<&str> = entries.iter().map(|e| e.host_field.as_str()).collect();
    assert_eq!(
        cli.len(),
        entries.len(),
        "sdk/cli/src/publish/capabilities.ts names a hostField twice: {entries:?}"
    );

    let missing: Vec<_> = gates.difference(&cli).collect();
    let extra: Vec<_> = cli.difference(&gates).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "sdk/cli/src/publish/capabilities.ts has drifted from `FunctionCapabilities` in host.rs.\n\
         gates in host.rs with no CLI entry (add one, so `oxyc` refuses the call the host \
         refuses): {missing:?}\n\
         CLI entries naming no host gate (drop them, or add the gate to the host): {extra:?}\n\
         If a new field shapes an allowed call rather than refusing one, list it in NOT_GATES \
         beside `storage_retention` and `fetch_max_bytes`."
    );
    for entry in &entries {
        assert!(
            !entry.ops.is_empty(),
            "CLI entry `{}` gates no `ctx` member — an entry with no ops lints nothing",
            entry.host_field
        );
    }
}

#[test]
fn the_cli_storage_ops_match_the_registry() {
    let ts = strip_comments(CLI_CAPABILITIES_TS);
    let (read, write) = (
        host_ops_gated_by("storage_read"),
        host_ops_gated_by("storage_write"),
    );
    assert!(
        read.len() >= 4 && write.len() >= 3,
        "the registry gates too few storage ops (read {read:?}, write {write:?})"
    );

    let entries = cli_entries(&ts);
    let ops_of = |field: &str| -> BTreeSet<String> {
        entries
            .iter()
            .find(|e| e.host_field == field)
            .map(|e| e.ops.iter().cloned().collect())
            .unwrap_or_default()
    };
    assert_eq!(
        ops_of("storage_read"),
        read,
        "the `ctx.storage` ops `oxyc` gates on `storage.read` differ from the registry's"
    );
    assert_eq!(
        ops_of("storage_write"),
        write,
        "the `ctx.storage` ops `oxyc` gates on `storage.write` differ from the registry's"
    );
}

/// The registry's gate fields are the struct's gate fields: a `Capability`
/// naming a field the struct lacks, or a struct gate no row reads, fails.
#[test]
fn the_registry_gates_are_the_struct_fields() {
    use super::env_policy::{Capability, HostOp};
    let host = strip_comments(HOST_RS);
    let gates: BTreeSet<String> = host_capability_fields(&host)
        .into_iter()
        .filter(|f| !NOT_GATES.contains(&f.as_str()))
        .collect();
    let registry: BTreeSet<String> = HostOp::ALL
        .iter()
        .filter_map(|op| op.spec().capability.host_field())
        .map(str::to_string)
        .collect();
    assert_eq!(
        registry, gates,
        "registry capabilities vs `FunctionCapabilities` gates"
    );
    // `storage.copy` answers to both storage fields and names neither.
    assert_eq!(Capability::StorageReadAndWrite.host_field(), None);
    assert!(HostOp::gated_by("storage_read").contains(&"storage.copy"));
    assert!(HostOp::gated_by("storage_write").contains(&"storage.copy"));
}

#[test]
fn the_scans_ignore_commented_out_entries() {
    let ts = strip_comments(
        r#"
        export const GATED_CAPABILITIES = [
          // { hostField: "ghost_gate", ops: ["ghost.op"] },
          /* hostField: "in_a_block",
             ops: ["block.op"], */
          {
            hostField: "real_gate",
            ops: [
              "real.one",
              "real.two"
            ],
          }
        ];
        "#,
    );
    assert_eq!(
        cli_entries(&ts),
        vec![CliEntry {
            host_field: "real_gate".into(),
            ops: vec!["real.one".into(), "real.two".into()],
        }]
    );

    let host = strip_comments(
        r#"
        /// pub not_a_field: bool,
        pub struct FunctionCapabilities {
            /// A gate.
            pub secrets_write: bool,
            pub oltp: WriterCapability,
        }
        pub struct Other {
            pub unrelated: bool,
        }
        "#,
    );
    assert_eq!(host_capability_fields(&host), vec!["secrets_write", "oltp"]);
}
