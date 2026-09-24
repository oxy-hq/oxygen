//! Drift guard between the host and `@oxy-hq/sdk/testing`'s copy of it,
//! `sdk/typescript/src/testing/host-contract.ts` — the words, gates, op names
//! and rules `createTestContext` enforces on an app's unit tests.
//!
//! THE HOST IS THE TRUTH; THE SDK IS A COPY. The test context exists so an
//! app's tests meet the host's real gates, and it can only do that while its
//! copy says what the host says: a refusal the host reworded is a test that
//! passes on a sentence production no longer utters, and a gate the host
//! added is the permissive-fake bug the context exists to end. Neither side
//! can `use` the other — one is Rust, one is TypeScript — so this holds them
//! together the way `cli_capabilities_drift.rs` holds `oxyc`'s capability map:
//! `include_str!` on every file, comments stripped, text scans, and a set
//! comparison whose failure names what drifted.
//!
//! ONE DIRECTION. The contract is checked against the host, never the host
//! against the contract (`internal-docs/sdk-testing-context.md` §7, answer 9):
//! `HOST_OPS` stays the source of truth in `host_call_attrs.rs`, and
//! `tests/custom_apps/canary_coverage.rs` keeps scanning the canary, not the
//! SDK. What this file does NOT do, on purpose (§7, answer 8): it carries no
//! paging classification, because `classify_host_error`'s heuristics stay in
//! one place and the context records what the host's own error carries.
//!
//! Text scans, not rendered gates (§7, answer 4): it needs neither the
//! `custom-app-functions` feature nor a database, runs in
//! `cargo nextest run -p oxy-app --lib` in every configuration, and a rename
//! of any scanned file is a compile error rather than a scan of nothing.

use std::collections::BTreeSet;

use super::cli_capabilities_drift::{
    NOT_GATES, cli_entries, host_capability_fields, host_storage_arms, quoted, strip_comments,
};

const CONTRACT_TS: &str =
    include_str!("../../../../../../sdk/typescript/src/testing/host-contract.ts");
const FUNCTION_LINT_TS: &str =
    include_str!("../../../../../../sdk/cli/src/publish/function-lint.ts");
const GATES_TS: &str = include_str!("../../../../../../sdk/typescript/src/testing/gates.ts");

/// Every file a `source:` in the contract may name, by that name.
const SOURCES: &[(&str, &str)] = &[
    ("host.rs", include_str!("host.rs")),
    ("host/destinations.rs", include_str!("host/destinations.rs")),
    ("host/airhouse_ops.rs", include_str!("host/airhouse_ops.rs")),
    ("host_call_attrs.rs", include_str!("host_call_attrs.rs")),
    ("upsert_support.rs", include_str!("upsert_support.rs")),
    ("tx.rs", include_str!("tx.rs")),
    ("runtime.rs", include_str!("runtime.rs")),
    (
        "transaction.rs",
        include_str!("../../../../../agentic/connector/src/transaction.rs"),
    ),
    (
        "postgres_tx/convert.rs",
        include_str!("../../../../../agentic/connector/src/postgres_tx/convert.rs"),
    ),
    (
        "connector.rs",
        include_str!("../../../../../agentic/connector/src/connector.rs"),
    ),
    (
        "schema.rs",
        include_str!("../../../../../oltp/src/schema.rs"),
    ),
];

/// The marker every capability-gate refusal in the host carries. Scanned so a
/// gate the host ADDS shows up here as a literal no template matches — the one
/// bounded "added" check this file makes (the rest is contract → host).
const GATE_MARKER: &str = "has not declared the";

fn source(name: &str) -> &'static str {
    SOURCES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, s)| *s)
        .unwrap_or_else(|| {
            panic!("host-contract.ts names a source this test does not include: `{name}`")
        })
}

/// A Rust or JS source as one comparable text: comments stripped, Rust `\`
/// line continuations joined, `\"` unescaped, `{{`/`}}` unescaped, and a JS
/// `${x}` spelled `{x}` so one placeholder syntax covers both.
fn flatten(src: &str) -> String {
    strip_comments(src)
        .replace("\\\n", "")
        .replace("\\\"", "\"")
        .replace("{{", "{")
        .replace("}}", "}")
        .replace("${", "{")
}

/// The literal segments of a template, split on `{placeholder}`s. A leading
/// placeholder leaves an empty first segment.
fn segments(template: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else { break };
        let name = &after[..close];
        let is_placeholder =
            !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
        if is_placeholder {
            out.last_mut().unwrap().push_str(&rest[..open]);
            out.push(String::new());
            rest = &after[close + 1..];
        } else {
            out.last_mut().unwrap().push_str(&rest[..open + 1]);
            rest = after;
        }
    }
    out.last_mut().unwrap().push_str(rest);
    out
}

/// Whether `hay` contains `template`, with each `{placeholder}` standing for
/// one or more characters on one line.
fn contains_template(hay: &str, template: &str) -> bool {
    let segs = segments(template);
    let (first, rest) = segs.split_first().unwrap();
    let starts: Vec<usize> = if first.is_empty() {
        vec![0]
    } else {
        hay.match_indices(first.as_str()).map(|(i, _)| i).collect()
    };
    'start: for start in starts {
        let mut pos = start + first.len();
        let mut after_gap = first.is_empty();
        for seg in rest {
            if seg.is_empty() {
                // A trailing placeholder stands for whatever follows.
                continue;
            }
            let Some(found) = hay[pos..].find(seg.as_str()) else {
                continue 'start;
            };
            let gap = &hay[pos..pos + found];
            // A placeholder stands for at least one character and never a line
            // break; the leading placeholder of a template is unconstrained.
            if !after_gap || !first.is_empty() {
                if gap.is_empty() || gap.contains('\n') {
                    continue 'start;
                }
            }
            after_gap = false;
            pos += found + seg.len();
        }
        return true;
    }
    false
}

/// The JS string literal that starts at the first quote in `s`, unescaped.
/// `None` when `s` holds no quote.
fn js_string(s: &str) -> Option<String> {
    let open = s.find(['"', '\''])?;
    let delim = s[open..].chars().next()?;
    let mut out = String::new();
    let mut chars = s[open + 1..].chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                other => out.push(other),
            },
            c if c == delim => return Some(out),
            c => out.push(c),
        }
    }
    None
}

/// `key: "…"` on a line, as the JS string after `key:`.
fn field(line: &str, key: &str) -> Option<String> {
    let at = line.find(&format!("{key}:"))?;
    js_string(&line[at + key.len() + 1..])
}

/// The lines of `src` from the one starting with `start` through the first
/// later line whose trimmed form ends with `end`.
///
/// Both ends are asserted. Asserting only the start let a terminator that no
/// longer matches return the rest of the file instead: `GATES` ends
/// `] as const satisfies readonly Gate[];`, so asking for `] as const;` ran on
/// through `ABSENT_GLOBALS` to *its* closer and the scan read whatever followed
/// as though it belonged to the block.
fn block(src: &str, start: &str, end: &str) -> String {
    let mut lines = src.lines().skip_while(|l| !l.starts_with(start)).peekable();
    assert!(lines.peek().is_some(), "no line starts with `{start}`");
    let mut out = Vec::new();
    let mut terminated = false;
    for line in lines {
        out.push(line);
        if line.trim_end().ends_with(end) {
            terminated = true;
            break;
        }
    }
    assert!(
        terminated,
        "the block opened by `{start}` has no later line ending `{end}` — the scan \
         would have read on to the end of the file"
    );
    out.join("\n")
}

/// One `refusal` entry of the contract: its key, the file it cites, its template.
#[derive(Debug, PartialEq, Eq)]
struct Refusal {
    key: String,
    source: String,
    template: String,
}

/// Every `{ source: "…", refusal: "…" }` object in the contract, keyed by the
/// object key on the line that opened it.
fn refusals(ts: &str) -> Vec<Refusal> {
    let mut out = Vec::new();
    let mut key = String::new();
    let mut source = String::new();
    let mut lines = ts.lines().peekable();
    while let Some(line) = lines.next() {
        if let Some(k) = line.strip_suffix(": {") {
            key = k.trim().to_string();
        } else if let Some(s) = field(line, "source") {
            source = s;
        } else if let Some(rest) = line.strip_prefix("refusal:") {
            let mut text = rest.to_string();
            let mut template = js_string(&text);
            while template.is_none() {
                let Some(more) = lines.next() else { break };
                text.push_str(more);
                template = js_string(&text);
            }
            let template = template.unwrap_or_else(|| panic!("refusal `{key}` has no string"));
            out.push(Refusal {
                key: key.clone(),
                source: source.clone(),
                template,
            });
        }
    }
    out
}

/// `name` or `name.member` for each entry of an `ABSENT_GLOBALS` block.
fn absent_globals(src: &str, end: &str) -> BTreeSet<String> {
    let body = block(src, "export const ABSENT_GLOBALS", end);
    // `name:` opens an entry; a `member:` on the same or a later line narrows it.
    let mut entries: Vec<(String, String)> = Vec::new();
    for line in body.lines() {
        if let Some(name) = field(line, "name") {
            entries.push((name, String::new()));
        }
        if let (Some(member), Some(last)) = (field(line, "member"), entries.last_mut()) {
            last.1 = member;
        }
    }
    entries
        .into_iter()
        .map(|(name, member)| {
            if member.is_empty() {
                name
            } else {
                format!("{name}.{member}")
            }
        })
        .collect()
}

/// The body of the Rust `fn` whose signature line starts with `start`: from
/// that line to the first later line that is exactly `}`, comments stripped.
/// Read from the unstripped source, because the closing brace is only
/// recognisable by its column.
fn fn_body(raw: &str, start: &str) -> String {
    let mut lines = raw.lines().skip_while(|l| !l.starts_with(start)).peekable();
    assert!(lines.peek().is_some(), "no line starts with `{start}`");
    let mut out = Vec::new();
    for line in lines {
        out.push(line);
        if line == "}" {
            break;
        }
    }
    strip_comments(&out.join("\n"))
}

#[test]
fn the_sdk_host_op_union_is_the_hosts_closed_list() {
    let ts = strip_comments(CONTRACT_TS);
    let rs = strip_comments(source("host_call_attrs.rs"));
    let sdk: BTreeSet<String> = quoted(&block(&ts, "export const HOST_OPS", "] as const;"))
        .into_iter()
        .collect();
    let host: BTreeSet<String> = quoted(&block(&rs, "pub const HOST_OPS", "];"))
        .into_iter()
        .collect();
    assert!(
        host.len() >= 30,
        "parsed only {} names from HOST_OPS — the scan broke",
        host.len()
    );
    let missing: Vec<_> = host.difference(&sdk).collect();
    let extra: Vec<_> = sdk.difference(&host).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "`HOST_OPS` in sdk/typescript/src/testing/host-contract.ts drifted from HOST_OPS in \
         host_call_attrs.rs.\nhost ops the SDK lacks (add the name, and a member on the \
         context that records under it): {missing:?}\nSDK names the host lacks (drop them): \
         {extra:?}"
    );
}

#[test]
fn every_refusal_the_sdk_context_throws_is_in_the_hosts_source() {
    let ts = strip_comments(CONTRACT_TS);
    let entries = refusals(&ts);
    assert!(
        entries.len() >= 15,
        "parsed only {} refusal templates from host-contract.ts — the text scan likely \
         broke; fix it rather than weakening the guard: {entries:?}",
        entries.len()
    );
    let keys: BTreeSet<&str> = entries.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(
        keys.len(),
        entries.len(),
        "host-contract.ts names a refusal twice"
    );

    let mut reworded = Vec::new();
    for entry in &entries {
        let hay = flatten(source(&entry.source));
        if !contains_template(&hay, &entry.template) {
            reworded.push(format!(
                "  `{}` (cites {}): {:?}",
                entry.key, entry.source, entry.template
            ));
        }
    }
    assert!(
        reworded.is_empty(),
        "host-contract.ts carries {} refusal(s) the host no longer says, word for word. Update \
         the template to the host's current text — the context must throw what production \
         throws, never a paraphrase:\n{}",
        reworded.len(),
        reworded.join("\n")
    );
}

#[test]
fn every_capability_refusal_the_host_writes_has_a_template() {
    let ts = strip_comments(CONTRACT_TS);
    let templates: Vec<String> = refusals(&ts).into_iter().map(|e| e.template).collect();
    let mut uncarried = Vec::new();
    for name in ["host.rs", "host/airhouse_ops.rs"] {
        let hay = flatten(source(name));
        // Each gate literal is one `"…"` on the flattened text; find every one
        // that carries the marker and check some template covers it.
        for (at, _) in hay.match_indices(GATE_MARKER) {
            let start = hay[..at].rfind('"').map(|i| i + 1).unwrap_or(0);
            let end = hay[at..].find('"').map(|i| at + i).unwrap_or(hay.len());
            let literal = &hay[start..end];
            if !templates
                .iter()
                .any(|t| contains_template(literal, t) || contains_template(&flatten(t), literal))
            {
                uncarried.push(format!("  {name}: {literal:?}"));
            }
        }
    }
    assert!(
        uncarried.is_empty(),
        "the host refuses {} capability gate(s) that host-contract.ts carries no template \
         for — add each as a `refusal:` entry, so the test context refuses it too:\n{}",
        uncarried.len(),
        uncarried.join("\n")
    );
}

#[test]
fn the_sdk_gates_name_exactly_the_gates_host_rs_has() {
    let host = strip_comments(source("host.rs"));
    let ts = strip_comments(CONTRACT_TS);
    let struct_fields = host_capability_fields(&host);
    let gates: BTreeSet<&str> = struct_fields
        .iter()
        .map(String::as_str)
        .filter(|f| !NOT_GATES.contains(f))
        .collect();
    // GATES's own closer. Asking for `] as const;` found ABSENT_GLOBALS's
    // instead, so the block ran on and the scan read that array too.
    let entries = cli_entries(&block(
        &ts,
        "export const GATES",
        "] as const satisfies readonly Gate[];",
    ));
    let sdk: BTreeSet<&str> = entries.iter().map(|e| e.host_field.as_str()).collect();
    assert_eq!(
        sdk.len(),
        entries.len(),
        "host-contract.ts names a hostField twice: {entries:?}"
    );
    let missing: Vec<_> = gates.difference(&sdk).collect();
    let extra: Vec<_> = sdk.difference(&gates).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "GATES in host-contract.ts has drifted from `FunctionCapabilities` in host.rs.\n\
         gates in host.rs with no SDK entry (add one, so the context refuses the call the \
         host refuses): {missing:?}\nSDK entries naming no host gate (drop them, or add the \
         gate to the host): {extra:?}\nA field that shapes an allowed call rather than \
         refusing one belongs in NOT_GATES in cli_capabilities_drift.rs."
    );
    for entry in &entries {
        assert!(
            !entry.ops.is_empty(),
            "SDK gate `{}` gates no host op — an entry with no ops refuses nothing",
            entry.host_field
        );
    }

    let (read, write) = host_storage_arms(&host);
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
        "the `storage.*` ops the SDK gates on `storage.read` differ from \
         `check_storage_capability`'s read arms"
    );
    assert_eq!(
        ops_of("storage_write"),
        write,
        "the `storage.*` ops the SDK gates on `storage.write` differ from \
         `check_storage_capability`'s write arms"
    );
}

#[test]
fn the_sdk_absent_globals_are_the_clis() {
    let sdk = absent_globals(&strip_comments(CONTRACT_TS), "] as const;");
    let cli = absent_globals(&strip_comments(FUNCTION_LINT_TS), "];");
    assert!(
        cli.len() >= 8,
        "parsed only {} entries from function-lint.ts — the scan broke",
        cli.len()
    );
    assert_eq!(
        sdk, cli,
        "ABSENT_GLOBALS in host-contract.ts differs from function-lint.ts's: the CLI and the \
         test context would disagree about what the isolate lacks. Change both in one PR."
    );
}

#[test]
fn the_sdk_fetch_rules_are_is_safe_outbounds() {
    let ts = strip_comments(CONTRACT_TS);
    let body = fn_body(source("host.rs"), "fn is_safe_outbound(");
    let host_rules: BTreeSet<String> = body.lines().flat_map(quoted).collect();
    let sdk_rules: BTreeSet<String> = block(&ts, "export const FETCH_RULES", "} as const;")
        .lines()
        .flat_map(quoted)
        .collect();
    assert!(
        host_rules.len() >= 8,
        "parsed only {host_rules:?} from `is_safe_outbound` — the scan broke"
    );
    assert_eq!(
        sdk_rules, host_rules,
        "FETCH_RULES in host-contract.ts differs from `is_safe_outbound` in host.rs (the \
         scheme, the literal hosts and INTERNAL_SUFFIXES). Change both in one PR."
    );
}

#[test]
fn the_template_matcher_reads_placeholders_as_wildcards() {
    let hay = flatten(
        r#"
        // return Err(format!("ghost '{db}' refusal"));
        return Err(format!(
            "database '{database}' is not in this function's \
             `destinations` allowlist (declare it in oxy-app.json to permit writes)"
        ));
        let m = "add \"org\": {{ \"read\": true }} to its entry";
        const s = `${label}: this transaction is already finished`;
        "#,
    );
    assert!(contains_template(
        &hay,
        "database '{database}' is not in this function's `destinations` allowlist (declare it in oxy-app.json to permit writes)"
    ));
    assert!(contains_template(
        &hay,
        "add \"org\": { \"read\": true } to its entry"
    ));
    assert!(contains_template(
        &hay,
        "{label}: this transaction is already finished"
    ));
    assert!(!contains_template(&hay, "ghost '{db}' refusal"));
    assert!(!contains_template(
        &hay,
        "database '{database}' is not in this function's allowlist"
    ));

    let ts = strip_comments(
        r#"
        export const REFUSALS = {
          // ghost: { source: "host.rs", refusal: "not here" },
          real: {
            source: "host.rs",
            refusal:
              'say "why" on the function'
          }
        } as const;
        "#,
    );
    assert_eq!(
        refusals(&ts),
        vec![Refusal {
            key: "real".into(),
            source: "host.rs".into(),
            template: "say \"why\" on the function".into(),
        }]
    );
}

/// `DIALECTS` names a deliberate subset of [`SqlDialect`], and says which.
///
/// The contract pins three engines a `TestDatabase` can be. `SqlDialect` has
/// six, so this cannot be an equality check — but it must not be nothing
/// either: without it a new engine, or a change to which engines parse
/// `ON CONFLICT`, drifts silently and `warehouse.upsert`'s refusal names the
/// wrong one. So: every dialect the contract names must exist in the host, its
/// `parsesOnConflict` must match `parses_on_conflict`, and every host variant
/// the contract omits must be listed here with the reason it is out of scope.
#[test]
fn the_sdk_dialects_are_a_named_subset_of_the_hosts() {
    /// Host variants a `TestDatabase` deliberately cannot be, and why.
    const OMITTED: &[(&str, &str)] = &[
        (
            "Sqlite",
            "no Oxy connector reports it; it exists for SQL-shape questions",
        ),
        (
            "BigQuery",
            "no test fixture renders its types; add it with a zoo engine",
        ),
        (
            "Snowflake",
            "no test fixture renders its types; add it with a zoo engine",
        ),
    ];

    let connector = strip_comments(source("connector.rs"));
    let ts = strip_comments(CONTRACT_TS);

    // The variants of the enum, from its declaration.
    let decl = block(&connector, "pub enum SqlDialect", "}");
    let variants: BTreeSet<String> = decl
        .lines()
        .skip(1)
        .filter_map(|l| {
            let t = l.trim().trim_end_matches(',');
            let name = t.split('(').next().unwrap_or_default().trim();
            (!name.is_empty() && name != "}" && name.chars().next().is_some_and(char::is_uppercase))
                .then(|| name.to_string())
        })
        .collect();
    assert!(
        variants.contains("Postgres") && variants.contains("DuckDb"),
        "did not parse SqlDialect's variants: {variants:?}"
    );

    // `Other` is the open arm; ClickHouse rides it through `SqlDialect::CLICKHOUSE`.
    let named: BTreeSet<&str> = variants
        .iter()
        .map(String::as_str)
        .filter(|v| *v != "Other")
        .collect();
    let omitted: BTreeSet<&str> = OMITTED.iter().map(|(v, _)| *v).collect();
    let unexplained: Vec<_> = named
        .iter()
        .filter(|v| !omitted.contains(*v) && !matches!(**v, "Postgres" | "DuckDb"))
        .collect();
    assert!(
        unexplained.is_empty(),
        "SqlDialect gained {unexplained:?}: either give it a DIALECTS entry (and a zoo \
         engine to render its types) or add it to OMITTED with the reason"
    );

    // What the contract names, and what the host says each one does.
    let dialects = block(&ts, "export const DIALECTS", "} as const;");
    let on_conflict = fn_body(&connector, "pub fn parses_on_conflict");
    for (key, host_variant) in [
        ("clickhouse", "Other"),
        ("postgres", "Postgres"),
        ("duckdb", "DuckDb"),
    ] {
        let line = dialects
            .lines()
            .find(|l| l.trim_start().starts_with(&format!("{key}:")))
            .unwrap_or_else(|| panic!("DIALECTS lost its `{key}` entry"));
        let sdk_parses = line.contains("parsesOnConflict: true");
        let host_parses = host_variant != "Other" && on_conflict.contains(host_variant);
        assert_eq!(
            sdk_parses, host_parses,
            "DIALECTS.{key}.parsesOnConflict disagrees with parses_on_conflict \
             (host: {host_parses}); warehouse.upsert's refusal would name the wrong engine"
        );
    }
}

/// `writerSchema`'s length rule is a hand-copy of `MAX_NAME_LEN`, and it feeds
/// both the gate (is this slug's schema derivable?) and the `{max}` the
/// refusal renders — where the host renders the constant itself. A change to
/// `MAX_IDENT_LEN` or to either affix moves the host's answer and leaves the
/// SDK's where it was, so a test written against the context would keep
/// passing on a slug production has started refusing. Recompute it here
/// rather than restate it: the arithmetic is the contract.
#[test]
fn the_sdk_schema_name_cap_is_the_hosts_arithmetic() {
    let schema = strip_comments(source("schema.rs"));

    // `const NAME: usize = 63;` / `const NAME: &str = "app_";`
    let usize_const = |name: &str| -> usize {
        let needle = format!("const {name}: usize = ");
        let rest = schema.split(&needle).nth(1).unwrap_or_else(|| {
            panic!("crates/oltp/src/schema.rs no longer declares `{name}: usize`")
        });
        rest.split(';')
            .next()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or_else(|| panic!("`{name}` in schema.rs is no longer a literal usize"))
    };
    let str_const_len = |name: &str| -> usize {
        let needle = format!("const {name}: &str = ");
        let rest = schema
            .split(&needle)
            .nth(1)
            .unwrap_or_else(|| panic!("crates/oltp/src/schema.rs no longer declares `{name}`"));
        quoted(rest.split(';').next().unwrap_or_default())
            .first()
            .unwrap_or_else(|| panic!("`{name}` in schema.rs is no longer a string literal"))
            .len()
    };

    let host_max =
        usize_const("MAX_IDENT_LEN") - str_const_len("APP_PREFIX") - str_const_len("RW_SUFFIX");

    // The one place the arithmetic is asserted to be the one `MAX_NAME_LEN` does.
    assert!(
        strip_comments(source("schema.rs"))
            .contains("MAX_NAME_LEN: usize = MAX_IDENT_LEN - APP_PREFIX.len() - RW_SUFFIX.len()"),
        "MAX_NAME_LEN is no longer `MAX_IDENT_LEN - APP_PREFIX - RW_SUFFIX`; \
         this test recomputes that expression and must be updated with it"
    );

    let sdk_line = strip_comments(GATES_TS)
        .lines()
        .find(|l| l.contains("MAX_SCHEMA_NAME_LEN"))
        .map(str::to_owned)
        .expect("gates.ts no longer declares MAX_SCHEMA_NAME_LEN");
    let sdk_max: usize = sdk_line
        .split('=')
        .nth(1)
        .and_then(|v| v.trim().trim_end_matches(';').parse().ok())
        .unwrap_or_else(|| panic!("MAX_SCHEMA_NAME_LEN is no longer a literal: `{sdk_line}`"));

    assert_eq!(
        sdk_max, host_max,
        "MAX_SCHEMA_NAME_LEN in sdk/typescript/src/testing/gates.ts is {sdk_max}, but \
         crates/oltp/src/schema.rs computes {host_max}; writerSchema would accept a slug \
         the host refuses (or refuse one it accepts), and the refusal would render the \
         wrong {{max}}"
    );
}
