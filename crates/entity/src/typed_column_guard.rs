//! Enforces the rule stated at the crate root: a struct deriving
//! `DeriveEntityModel` also carries `#[sea_orm::model]`.
//!
//! The crate-root docs describe the failure mode — the attribute is what emits
//! `COLUMN`, so omitting it loses the typed form *while still compiling*, and
//! the mistake surfaces later at an unrelated call site that cannot resolve
//! `COLUMN`. A convention whose failure mode is "still compiles" cannot be left
//! to review attention; this is the mechanical guard for it.
//!
//! The scan is workspace-wide rather than crate-local on purpose. Sea-ORM
//! entities live in seven other crates, and a `crates/entity`-only check would
//! report all-clear while someone in `crates/cameras` hits an unresolvable
//! `COLUMN` with no way to tell whether that is deliberate.
//!
//! The scan reaches past this crate's own dependencies, so the dependency graph
//! cannot tell CI's test selection when it is affected. `entity` is therefore
//! an `always-test` package in `.config/test-selection.toml`, and is not
//! excluded by the `default-filter` in `.config/nextest.toml`. While it was
//! excluded there, this guard ran nowhere, and 21 entities landed without the
//! attribute before anyone noticed.
//!
//! `PENDING` is a backlog, not an exemption list. Those entities predate the
//! attribute and are listed so the guard can fail on *new* omissions today
//! rather than waiting for the backlog to clear. Removing an entry — by adding
//! the attribute — is always the right direction; adding one needs a reason.

use std::path::{Path, PathBuf};

/// Entities that derive `DeriveEntityModel` without `#[sea_orm::model]`.
/// Shrink this list; do not grow it.
const PENDING: &[&str] = &[
    "crates/agentic/airway/src/extension/load_audit.rs",
    "crates/agentic/airway/src/extension/pipeline_lease.rs",
    "crates/agentic/airway/src/extension/pipeline_state.rs",
    "crates/agentic/airway/src/extension/run_extension.rs",
    "crates/agentic/analytics/src/extension/entity.rs",
    "crates/agentic/automation/src/extension/entity.rs",
    "crates/agentic/runtime/src/lifecycle/entity/run.rs",
    "crates/agentic/runtime/src/lifecycle/entity/run_event.rs",
    "crates/agentic/runtime/src/lifecycle/entity/run_suspension.rs",
    "crates/agentic/runtime/src/lifecycle/entity/schedule.rs",
    "crates/agentic/runtime/src/orchestrator/entity/task_outcome.rs",
    "crates/agentic/runtime/src/orchestrator/entity/task_queue.rs",
    "crates/airhouse/src/entity/tenants.rs",
    "crates/cameras/src/entities/audit_events.rs",
    "crates/cameras/src/entities/cameras.rs",
    "crates/cameras/src/entities/compliance_arbitrations.rs",
    "crates/cameras/src/entities/device_claims.rs",
    "crates/cameras/src/entities/device_registry.rs",
    "crates/cameras/src/entities/domain_packs.rs",
    "crates/cameras/src/entities/edge_boxes.rs",
    "crates/cameras/src/entities/rollout_plans.rs",
    "crates/cameras/src/entities/sites.rs",
    "crates/cameras/src/entities/unifi_credentials.rs",
];

/// Either macro emits `COLUMN`: `#[sea_orm::model]` is the 2.0 dense format and
/// `#[sea_orm::compact_model]` the transitional one that back-fits `COLUMN` onto
/// the 1.x layout. Both satisfy this rule, so the backlog above can be worked
/// off with whichever fits each crate.
fn is_typed_column_attr(line: &str) -> bool {
    let l = line.trim();
    l.starts_with("#[") && (l.contains("sea_orm::model") || l.contains("sea_orm::compact_model"))
}

/// `crates/entity` -> workspace root.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/entity is two levels below the workspace root")
        .to_path_buf()
}

/// Every `.rs` file under `crates/`, skipping build output.
fn entity_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if name != "target" && name != "node_modules" {
                entity_sources(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Repo-relative, forward-slashed, so the output matches `PENDING` verbatim.
fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Line numbers of real `#[derive(..DeriveEntityModel..)]` attributes.
///
/// Must be the attribute itself, not any line naming the macro: this crate's
/// root docs and `feature_flag`'s test docs both discuss `DeriveEntityModel` in
/// prose, and counting those reports two files that contain no entity at all.
///
/// A `#[derive(` whose parenthesis does not close on its own line is joined
/// with the lines that follow before testing. rustfmt wraps a derive list once
/// it exceeds `max_width`, and a wrapped list would otherwise match nothing —
/// the entity is skipped and the file passes silently, which is the failure
/// direction a guard exists to prevent. No entity is wrapped today; it is one
/// added trait away.
///
/// Returns the index of the line the derive *starts* on, which is what
/// `derive_is_annotated` walks back from.
fn derive_lines(lines: &[&str]) -> Vec<usize> {
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if !t.starts_with("#[derive(") {
            continue;
        }
        let mut joined = t.to_string();
        let mut j = i;
        // Balance the parens; `max_width` wrapping puts the traits below.
        while joined.matches('(').count() > joined.matches(')').count() {
            j += 1;
            match lines.get(j) {
                Some(next) => {
                    joined.push(' ');
                    joined.push_str(next.trim());
                }
                None => break,
            }
        }
        if joined.contains("DeriveEntityModel") {
            out.push(i);
        }
    }
    out
}

/// True when the `DeriveEntityModel` on `line_idx` has the attribute above it.
///
/// Per *struct*, not per file: walking back over the contiguous attribute and
/// doc-comment lines directly above the derive means a file holding two
/// entities, only one annotated, is still caught. A file-wide substring search
/// would see the one hit and pass the whole file.
fn derive_is_annotated(lines: &[&str], line_idx: usize) -> bool {
    // Stop at the first line that is unambiguously *code*, and treat everything
    // between it and the derive as the attribute block.
    //
    // Matching continuation lines by shape does not work: a multi-line
    // attribute's inner lines look like anything at all (`table_name = "x",`,
    // `schema_name = "public"`, a bare `)]`), so any pattern list is a guess
    // that reports a false violation the first time an entity is formatted
    // differently. Recognising the small, closed set of things that *end* an
    // attribute block is the reliable direction.
    const CODE_STARTS: &[&str] = &[
        "pub", "fn ", "struct ", "enum ", "impl ", "use ", "mod ", "type ", "const ", "static ",
        "let ", "}",
    ];

    for line in lines[..line_idx].iter().rev() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("//") {
            continue;
        }
        if CODE_STARTS.iter().any(|k| t.starts_with(k)) {
            break;
        }
        if is_typed_column_attr(line) {
            return true;
        }
    }
    false
}

/// How many unannotated entity structs the `PENDING` files hold between them.
///
/// `PENDING` is a list of *paths*, so it alone cannot tell a known-bad file
/// from one that has since grown a second unannotated entity: the file stays
/// in `missing`, still matches its entry, and the tests stay green. That is the
/// "a new omission slips in unnoticed" case this guard exists to prevent, just
/// inside the backlog rather than outside it. Pinning the struct count closes
/// it — every listed file holds exactly one today, so this equals `PENDING`'s
/// length, and adding an entity to any of them fails.
const PENDING_STRUCT_COUNT: usize = 23;

/// Splits every file with a `DeriveEntityModel` struct into (annotated,
/// missing), where each `missing` entry carries its count of unannotated
/// structs.
///
/// A file lands in `missing` if *any* of its entity structs lacks the attribute.
fn partition_entities(root: &Path) -> (Vec<String>, Vec<(String, usize)>) {
    let mut files = Vec::new();
    entity_sources(&root.join("crates"), &mut files);

    let (mut annotated, mut missing) = (Vec::new(), Vec::new());
    for path in files {
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        let rel = relative(root, &path);
        // This file names the markers in prose, so it would match itself.
        if rel.ends_with("typed_column_guard.rs") {
            continue;
        }

        let lines: Vec<&str> = src.lines().collect();
        let derives = derive_lines(&lines);
        if derives.is_empty() {
            continue;
        }

        let unannotated = derives
            .iter()
            .filter(|&&i| !derive_is_annotated(&lines, i))
            .count();
        if unannotated == 0 {
            annotated.push(rel);
        } else {
            missing.push((rel, unannotated));
        }
    }
    (annotated, missing)
}

#[test]
fn every_entity_carries_the_typed_column_macro_or_is_a_known_exception() {
    let root = workspace_root();
    let (annotated, missing) = partition_entities(&root);

    // Guards the scan itself: a walker that silently found nothing would
    // otherwise pass while enforcing nothing.
    assert!(
        annotated.len() > 50,
        "expected the walker to find the annotated entities in crates/entity, saw {}",
        annotated.len()
    );

    let unexpected: Vec<_> = missing
        .iter()
        .filter(|(f, _)| !PENDING.contains(&f.as_str()))
        .collect();

    assert!(
        unexpected.is_empty(),
        "these files have a `DeriveEntityModel` struct without `#[sea_orm::model]`, \
         so it emits no `COLUMN` and the omission compiles cleanly:\n  {}\n\
         Add the attribute above the `#[derive(..)]` line. See the crate-root docs.",
        unexpected
            .iter()
            .map(|(f, _)| f.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );

    // Per-file membership is not enough on its own: a *new* unannotated entity
    // added to one of the listed files keeps that file in `missing`, still
    // matching its entry, with every assertion above still green.
    let total: usize = missing.iter().map(|(_, n)| n).sum();
    assert_eq!(
        total, PENDING_STRUCT_COUNT,
        "the backlog holds {total} unannotated entity structs, expected \
         {PENDING_STRUCT_COUNT}. Up means a new entity was added without \
         `#[sea_orm::model]` to a file already on the list; down means one was \
         fixed — lower the constant, and drop the path from PENDING if that was \
         its last one."
    );
}

#[test]
fn the_pending_backlog_has_no_stale_entries() {
    let root = workspace_root();
    let (_, missing) = partition_entities(&root);

    let fixed: Vec<_> = PENDING
        .iter()
        .filter(|f| !missing.iter().any(|(m, _)| m == *f))
        .collect();

    assert!(
        fixed.is_empty(),
        "these are listed as PENDING but now carry the attribute (or moved). \
         Drop them from the list so it keeps shrinking:\n  {}",
        fixed.iter().map(|f| **f).collect::<Vec<_>>().join("\n  ")
    );
}

#[test]
fn crates_entity_itself_is_fully_annotated() {
    let root = workspace_root();
    let (_, missing) = partition_entities(&root);

    let local: Vec<_> = missing
        .iter()
        .filter(|(f, _)| f.starts_with("crates/entity/"))
        .collect();

    assert!(
        local.is_empty(),
        "the crate-root docs claim every entity here carries the attribute:\n  {}",
        local
            .iter()
            .map(|(f, _)| f.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}

/// The per-struct walk must catch a second, unannotated entity in a file whose
/// first entity is fine — the case a file-wide substring search gets wrong.
#[test]
fn detects_an_unannotated_struct_in_an_otherwise_annotated_file() {
    let src = r#"
#[sea_orm::model]
#[derive(Clone, DeriveEntityModel)]
#[sea_orm(table_name = "good")]
pub struct Model { pub id: i32 }

#[derive(Clone, DeriveEntityModel)]
#[sea_orm(table_name = "bad")]
pub struct Other { pub id: i32 }
"#;
    let lines: Vec<&str> = src.lines().collect();
    let derives = derive_lines(&lines);

    assert_eq!(derives.len(), 2, "fixture should hold two entity structs");
    assert!(derive_is_annotated(&lines, derives[0]));
    assert!(
        !derive_is_annotated(&lines, derives[1]),
        "the second struct has no attribute and must be reported"
    );
}

/// A file that only *discusses* `DeriveEntityModel` in prose holds no entity.
/// This crate's root docs and `feature_flag`'s test docs both do, and counting
/// them reported two entity-free files as violations.
#[test]
fn prose_mentions_are_not_mistaken_for_derives() {
    let src = r#"
//! `DeriveEntityModel` on its own does not generate `COLUMN`.
/// Guards the `#[sea_orm::model]` annotation on every DeriveEntityModel entity.
fn documented() {}
"#;
    let lines: Vec<&str> = src.lines().collect();
    assert!(
        derive_lines(&lines).is_empty(),
        "prose naming the macro must not count as an entity"
    );
}

/// rustfmt wraps a derive list once it exceeds `max_width`. The wrapped form
/// must still be detected — an undetected entity is skipped silently, which is
/// a false pass, the direction that matters.
#[test]
fn a_wrapped_derive_list_is_still_detected() {
    let src = r#"
#[sea_orm::model]
#[derive(
    Clone,
    Debug,
    PartialEq,
    DeriveEntityModel,
    Eq,
    Serialize,
    Deserialize,
)]
#[sea_orm(table_name = "wrapped")]
pub struct Model { pub id: i32 }
"#;
    let lines: Vec<&str> = src.lines().collect();
    let derives = derive_lines(&lines);
    assert_eq!(derives.len(), 1, "wrapped derive list must be found");
    assert!(
        derive_is_annotated(&lines, derives[0]),
        "and its marker attribute must still resolve"
    );
}

/// A multi-line attribute between the marker and the derive must not end the
/// walk back — that would report a false violation on a correct entity.
#[test]
fn a_multiline_attribute_does_not_break_the_walk_back() {
    let src = r#"
#[sea_orm::model]
#[sea_orm(
    table_name = "spread",
    schema_name = "public"
)]
#[derive(Clone, DeriveEntityModel)]
pub struct Model { pub id: i32 }
"#;
    let lines: Vec<&str> = src.lines().collect();
    let derives = derive_lines(&lines);
    assert_eq!(derives.len(), 1);
    assert!(
        derive_is_annotated(&lines, derives[0]),
        "the marker sits above a multi-line attribute and must still be seen"
    );
}
