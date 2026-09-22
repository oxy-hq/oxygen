//! Coverage: every host op an Oxy Function can make has a platform-canary step, or a
//! reasoned exemption.
//!
//! `HOST_OPS` (`custom_apps_functions::host_call_attrs`) is the closed list of names a host
//! op pages under. The platform canary (`customer-apps/examples/platform-canary`) makes the
//! host calls live apps make, every five minutes on staging and prod, and its
//! `functions/steps.ts` declares per step which ops it makes (`STEP_OPS`); the canary's
//! vitest suite checks each declaration against the calls the step really makes on a fake
//! host. This test closes the loop from the platform's side: a name in `HOST_OPS` that no
//! step declares and no `EXEMPT` entry explains fails here, as does a declared op the host
//! does not have, and a `HostOp` type in `steps.ts` that drifts from `HOST_OPS`.
//!
//! Source scans only, no database, in the style of `shape_zoo_coverage.rs`: each block is
//! read from `steps.ts` with comments removed, so a commented-out op does not count.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use oxy_app::server::api::custom_apps_functions::HOST_OPS;

const STEPS_TS: &str = "customer-apps/examples/platform-canary/functions/steps.ts";

const AIRHOUSE: &str = "`ctx.airhouse` writes the app's own schema in the workspace's Airhouse, whose tables \
     come only from `airhouseMigrations` applied at promote (`exec` refuses DDL). The canary \
     manifest declares neither the `airhouse` capability nor a migrations dir, and neither \
     staging nor prod is known to provision Airhouse for the `oxy-canary` org. Unblocked by: \
     provisioning it there, adding `\"airhouse\": { \"enabled\": true }` and a one-table \
     migration to oxy-app.json, then a step that appends one run-tagged row and reads it back.";

/// Host ops the canary does not exercise: each with why, and what would unblock it.
const EXEMPT: &[(&str, &str)] = &[
    (
        "email.send",
        "a real SES send every five minutes is a side effect outside the platform, to a \
         mailbox someone has to own (open decision D3 in \
         internal-docs/2026-09-14-custom-app-verification-design.md). Unblocked by that \
         decision and a sink address.",
    ),
    (
        "semantic.query",
        "needs a semantic model in the canary workspace — a topic with a measure — and the \
         workspace has none, so a call would fail on the missing topic rather than prove the \
         host path. Unblocked by committing a minimal view and topic to the canary workspace \
         and querying one measure.",
    ),
    (
        "airway.run",
        "runs an ELT pipeline: a side effect (a run, its lease, landed tables) that needs an \
         `.airway.yml` and source credentials the canary org has none of. Out of scope for \
         the canary by design.",
    ),
    ("airhouse.query", AIRHOUSE),
    ("airhouse.exec", AIRHOUSE),
    ("airhouse.append", AIRHOUSE),
];

#[test]
fn every_host_op_has_a_canary_step_or_a_reasoned_exemption() {
    let steps = step_ops(&steps_ts());
    let declared: BTreeSet<&str> = steps.values().flatten().map(String::as_str).collect();
    let missing: Vec<&str> = HOST_OPS
        .iter()
        .copied()
        .filter(|op| !declared.contains(op) && !EXEMPT.iter().any(|(e, _)| e == op))
        .collect();
    assert!(
        missing.is_empty(),
        "no platform-canary step declares {} host op(s): {}\nAdd a step to {STEPS_TS} that \
         makes the call and list the op in STEP_OPS, or, when the canary cannot make it on \
         staging and prod without a third party or a side effect outside the platform, an \
         EXEMPT entry in canary_coverage.rs saying why and what would unblock it.",
        missing.len(),
        missing.join(", ")
    );
}

#[test]
fn every_op_a_canary_step_declares_is_a_host_op() {
    let steps = step_ops(&steps_ts());
    let bogus: Vec<String> = steps
        .iter()
        .flat_map(|(step, ops)| ops.iter().map(move |op| (step, op)))
        .filter(|(_, op)| !HOST_OPS.contains(&op.as_str()))
        .map(|(step, op)| format!("  `{op}` (declared by {step})"))
        .collect();
    assert!(
        bogus.is_empty(),
        "STEP_OPS in {STEPS_TS} declares {} op(s) the host does not have:\n{}\nThe names \
         are HOST_OPS in custom_apps_functions/host_call_attrs.rs.",
        bogus.len(),
        bogus.join("\n")
    );
}

#[test]
fn the_canary_host_op_type_is_the_hosts_closed_list() {
    let union = host_op_union(&steps_ts());
    let host: BTreeSet<&str> = HOST_OPS.iter().copied().collect();
    let extra: Vec<&str> = union
        .iter()
        .map(String::as_str)
        .filter(|op| !host.contains(op))
        .collect();
    let lacking: Vec<&str> = host
        .iter()
        .copied()
        .filter(|op| !union.contains(*op))
        .collect();
    assert!(
        extra.is_empty() && lacking.is_empty(),
        "`type HostOp` in {STEPS_TS} drifted from HOST_OPS: names the host lacks: [{}]; host \
         ops the type lacks: [{}]",
        extra.join(", "),
        lacking.join(", ")
    );
}

#[test]
fn canary_exemptions_are_still_host_ops_and_still_undeclared() {
    let steps = step_ops(&steps_ts());
    let declared: BTreeSet<&str> = steps.values().flatten().map(String::as_str).collect();
    for (op, why) in EXEMPT {
        assert!(
            HOST_OPS.contains(op),
            "EXEMPT names `{op}`, which is not a host op any more; delete the entry"
        );
        assert!(
            !declared.contains(op),
            "EXEMPT names `{op}`, which a canary step now declares; delete the entry"
        );
        assert!(
            why.contains("Unblocked by") || why.contains("by design"),
            "the exemption for `{op}` says neither what would unblock it nor that it is by design"
        );
    }
}

#[test]
fn every_canary_step_declares_its_ops_and_every_declaration_is_a_step() {
    let src = steps_ts();
    let steps = step_ops(&src);
    let all: BTreeSet<String> = string_literals(&block(&src, "export const ALL_STEPS", "];"))
        .into_iter()
        .collect();
    assert!(all.len() >= 10, "ALL_STEPS scan found only {all:?}");
    let keys: BTreeSet<String> = steps.keys().cloned().collect();
    assert_eq!(keys, all, "STEP_OPS keys and ALL_STEPS differ");
    for (step, ops) in &steps {
        assert!(!ops.is_empty(), "step {step} declares no host op");
        let unique: BTreeSet<&String> = ops.iter().collect();
        assert_eq!(unique.len(), ops.len(), "step {step} declares an op twice");
    }
}

#[test]
fn canary_scanner_reads_code_not_comments() {
    let fixture = r#"
// export const STEP_OPS: Record<StepName, readonly HostOp[]> = { ghost: ["email.send"] };
export type HostOp =
  | "query" // | "airway.run"
  | "fetch";

export const ALL_STEPS: StepName[] = ["a", "b" /* , "c" */];

export const STEP_OPS: Record<StepName, readonly HostOp[]> = {
  // a: ["ghost.op"],
  a: ["query" /* , "email.send" */],
  b: [
    "fetch", // "org.people"
    "storage.put"
  ]
};
"#;
    let steps = step_ops(fixture);
    let expect = |names: &[&str]| names.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        steps,
        BTreeMap::from([
            ("a".to_string(), expect(&["query"])),
            ("b".to_string(), expect(&["fetch", "storage.put"])),
        ])
    );
    assert_eq!(
        host_op_union(fixture),
        BTreeSet::from(["query".to_string(), "fetch".to_string()])
    );
    assert_eq!(
        string_literals(&block(fixture, "export const ALL_STEPS", "];")),
        expect(&["a", "b"])
    );
}

fn steps_ts() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(STEPS_TS);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {STEPS_TS}: {e}"))
}

/// `STEP_OPS` from `src`: each step's declared ops, in declaration order.
fn step_ops(src: &str) -> BTreeMap<String, Vec<String>> {
    let body = block(src, "export const STEP_OPS", "};");
    let entries = &body[body.find('{').expect("STEP_OPS opens with `{`")..];
    let mut out = BTreeMap::new();
    for chunk in entries.split(']') {
        let Some((before, list)) = chunk.split_once('[') else {
            continue;
        };
        let key = before
            .trim()
            .trim_end_matches(':')
            .rsplit(|c: char| c.is_whitespace() || c == ',' || c == '{')
            .next()
            .unwrap_or("")
            .to_string();
        assert!(
            !key.is_empty() && key.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
            "STEP_OPS entry key {key:?} is not a step name"
        );
        assert!(
            out.insert(key.clone(), string_literals(list)).is_none(),
            "STEP_OPS lists {key} twice"
        );
    }
    assert!(!out.is_empty(), "STEP_OPS has no entries");
    out
}

/// The `type HostOp = | "…" | "…";` union from `src`.
fn host_op_union(src: &str) -> BTreeSet<String> {
    string_literals(&block(src, "export type HostOp =", ";"))
        .into_iter()
        .collect()
}

/// The lines of `src` from the one starting with `start` through the first later line
/// ending with `end`, comments removed. Anchored at a line start so a mention of `start`
/// in a comment or a string does not count.
fn block(src: &str, start: &str, end: &str) -> String {
    let mut lines = src.lines();
    let first = lines
        .by_ref()
        .find(|line| line.starts_with(start))
        .unwrap_or_else(|| panic!("no line starts with `{start}`"));
    let mut raw = vec![first];
    if !first.trim_end().ends_with(end) {
        for line in lines {
            raw.push(line);
            if line.trim_end().ends_with(end) {
                break;
            }
        }
    }
    let body = strip_comments(&raw.join("\n"));
    assert!(
        body.trim_end().ends_with(end),
        "the `{start}` block does not end with `{end}`"
    );
    body
}

/// `src` without `//` and `/* */` comments; `"`, `'` and `` ` `` literals copied whole.
fn strip_comments(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < chars.len() {
        match (chars[i], chars.get(i + 1)) {
            (q @ ('"' | '\'' | '`'), _) => {
                let end = literal_end(&chars, i, q);
                out.extend(&chars[i..end]);
                i = end;
            }
            ('/', Some('/')) => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            ('/', Some('*')) => {
                let mut j = i + 2;
                while j + 1 < chars.len() && !(chars[j] == '*' && chars[j + 1] == '/') {
                    j += 1;
                }
                i = j + 2;
            }
            _ => {
                out.push(chars[i]);
                i += 1;
            }
        }
    }
    out
}

/// Index just past the literal quoted with `quote` that opens at `start`.
fn literal_end(chars: &[char], start: usize, quote: char) -> usize {
    let mut i = start + 1;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 2,
            c if c == quote => return i + 1,
            _ => i += 1,
        }
    }
    chars.len()
}

/// Every `"…"` literal in comment-free `s`, in order.
fn string_literals(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let (mut out, mut i) = (Vec::new(), 0);
    while i < chars.len() {
        if chars[i] == '"' {
            let end = literal_end(&chars, i, '"');
            out.push(chars[i + 1..end - 1].iter().collect());
            i = end;
        } else {
            i += 1;
        }
    }
    out
}
