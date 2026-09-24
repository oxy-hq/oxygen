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
//! database: it runs in `cargo nextest run -p oxy-app --lib` in every
//! configuration, and `include_str!` makes a rename of either file a compile
//! error rather than a test that quietly scans nothing.

use std::collections::BTreeSet;

const CLI_CAPABILITIES_TS: &str =
    include_str!("../../../../../../sdk/cli/src/publish/capabilities.ts");
const HOST_RS: &str = include_str!("host.rs");

/// Fields of `FunctionCapabilities` that are NOT gates: a retention policy and
/// a byte ceiling shape a call that is allowed, they never refuse one. The
/// TypeScript file carries the same two names in its own doc comment, and a
/// third non-gate field has to be added here on purpose — the test below fails
/// until it is, which is the point.
pub(super) const NOT_GATES: &[&str] = &["storage_retention", "fetch_max_bytes"];

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

/// The `check_storage_capability` arms: which `ctx.storage` op needs
/// `storage.read`, which `storage.write`. Parsed from lines shaped
/// `"a" | "b" => (needs_read, needs_write),`.
pub(super) fn host_storage_arms(host: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut read = BTreeSet::new();
    let mut write = BTreeSet::new();
    let mut in_fn = false;
    for line in host.lines() {
        if line.starts_with("fn check_storage_capability(") {
            in_fn = true;
            continue;
        }
        if !in_fn {
            continue;
        }
        if line.starts_with("other =>") {
            break;
        }
        let Some((ops, needs)) = line.split_once("=> (") else {
            continue;
        };
        let needs: Vec<&str> = needs
            .trim_end_matches([')', ','])
            .split(',')
            .map(str::trim)
            .collect();
        let needs_read = needs.first() == Some(&"true");
        let needs_write = needs.get(1) == Some(&"true");
        for op in quoted(ops) {
            if needs_read {
                read.insert(format!("storage.{op}"));
            }
            if needs_write {
                write.insert(format!("storage.{op}"));
            }
        }
    }
    (read, write)
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
fn the_cli_storage_ops_match_check_storage_capability() {
    let host = strip_comments(HOST_RS);
    let ts = strip_comments(CLI_CAPABILITIES_TS);
    let (read, write) = host_storage_arms(&host);
    assert!(
        read.len() >= 4 && write.len() >= 3,
        "parsed too few arms from `check_storage_capability` (read {read:?}, write {write:?}) — \
         the text scan likely broke; fix it rather than weakening the guard"
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
        "the `ctx.storage` ops `oxyc` gates on `storage.read` differ from \
         `check_storage_capability`'s read arms"
    );
    assert_eq!(
        ops_of("storage_write"),
        write,
        "the `ctx.storage` ops `oxyc` gates on `storage.write` differ from \
         `check_storage_capability`'s write arms"
    );
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
