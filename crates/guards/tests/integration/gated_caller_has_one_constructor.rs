//! The gate's admitted caller (`oxy_app_core::gate_response::GatedCaller`) is
//! built in exactly one place: `oxy_custom_apps::gate::CustomAppContext::gated`.
//!
//! Its `staging_pin` picks the custom-app result-cache partition
//! (`staging_pin_scope::scope_for`): `""` on live, `staging-pin:<uuid>` on a
//! pinned staging request. A second constructor that took the pin from
//! anywhere but the gate's own context would serve a branch's answer to live
//! users, and nothing would fail to compile. So the type's fields are private,
//! its one constructor is `GatedCaller::from_gate`, and this guard holds the
//! rest:
//!
//! - `from_gate` is called only from `crates/custom-apps/src/gate.rs`;
//! - both surfaces that open the semantic boundary (`oxy-api-custom-apps`'
//!   `custom_apps_gates`, `oxy-api-world-model`'s `customer_app`) pass the
//!   context's `gated()`, so each gets its `staging_pin` from that constructor.
//!
//! And the boundary opens only after a gate: `open_semantic_boundary` runs no
//! gate itself, so its production callers are exactly those two surface
//! wrappers, each of which runs `check_custom_app_gates` first
//! ([`the_semantic_boundary_opens_only_behind_a_gate`]). A hand-built
//! `CustomAppContext` can't stand in for a gate that ran: its private
//! `_gate_ran` field means only the gate module builds one.
//!
//! Depth stage C (A2, 2026-11-27) moved the gate up into the surfaces; this is
//! what keeps its answer from being rebuilt by hand in each of them, and the
//! boundary from being opened without it.

use std::path::{Path, PathBuf};

const CONSTRUCTOR_CALL: &str = "GatedCaller::from_gate(";
/// The one file allowed to call [`CONSTRUCTOR_CALL`].
const THE_ONE_CALLER: &str = "../custom-apps/src/gate.rs";
/// The surfaces that run the gate and then open the semantic boundary, each
/// with the call that hands it the gate's answer.
const SURFACES: &[&str] = &[
    "../api-custom-apps/src/custom_apps_gates.rs",
    "../api-world-model/src/customer_app/mod.rs",
];
const SURFACE_CALL: &str = "open_semantic_boundary(app.gated())";
const BOUNDARY_CALL: &str = "open_semantic_boundary(";
/// What each surface wrapper runs before [`SURFACE_CALL`].
const GATE_CALL: &str = "check_custom_app_gates(";

/// Whether `path` is test code: an integration-test directory or a
/// `*_tests.rs` file a module pulls in with `#[path]`.
fn is_test_file(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == "tests")
        || path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with("_tests.rs"))
}

/// The calls of `needle` (ending in `(`) in `text`'s non-comment lines,
/// leaving out its definition (`fn name(`) and inline `#[cfg(test)] mod`
/// blocks.
fn production_calls(text: &str, needle: &str) -> usize {
    let mut count = 0;
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if line == "#[cfg(test)]"
            && lines
                .peek()
                .is_some_and(|next| next.starts_with("mod ") && next.ends_with('{'))
        {
            for inner in lines.by_ref() {
                if inner == "}" {
                    break;
                }
            }
            continue;
        }
        if line.trim_start().starts_with("//") {
            continue;
        }
        count += line
            .match_indices(needle)
            .filter(|(at, _)| !line[..*at].ends_with("fn "))
            .count();
    }
    count
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if name == "target" || name.starts_with('.') {
                continue;
            }
            rust_files(&path, out);
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
}

fn relative(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root.join("..")).unwrap_or(path);
    format!("../{}", rel.display())
}

#[test]
fn gated_caller_has_one_constructor() {
    let root = crate::guards_root();
    let mut files = Vec::new();
    rust_files(&root.join(".."), &mut files);
    assert!(
        files.len() > 100,
        "walked only {} files under {}: the scan would pass by finding nothing",
        files.len(),
        root.join("..").display()
    );
    let guards = root.canonicalize().expect("crates/guards");
    let mut callers: Vec<String> = files
        .iter()
        .filter(|path| {
            !path
                .canonicalize()
                .map(|p| p.starts_with(&guards))
                .unwrap_or(false)
        })
        .filter(|path| {
            std::fs::read_to_string(path)
                .map(|text| text.contains(CONSTRUCTOR_CALL))
                .unwrap_or(false)
        })
        .map(|path| relative(&root, path))
        .collect();
    callers.sort();
    assert_eq!(
        callers,
        vec![THE_ONE_CALLER.to_string()],
        "`{CONSTRUCTOR_CALL}` must have one caller, `CustomAppContext::gated`: \
         its `staging_pin` picks the live/staging result-cache partition, so \
         a second constructor can serve a branch's answer to live users. Call \
         `ctx.gated()` instead."
    );
}

#[test]
fn both_surfaces_take_the_gates_answer_from_gated() {
    let root = crate::guards_root();
    for surface in SURFACES {
        let path = root.join(surface);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        assert!(
            text.contains(SURFACE_CALL),
            "{surface} should open the semantic boundary with `{SURFACE_CALL}`, \
             so its caller and staging pin come from the one constructor"
        );
        assert!(
            !text.contains("GatedCaller {"),
            "{surface} builds a `GatedCaller` by hand; take `app.gated()`"
        );
    }
}

/// `open_semantic_boundary` runs no gate: the surface must have run it. So
/// its production callers are exactly the two surface wrappers in
/// [`SURFACES`], and in each the gate call comes before the boundary call. A
/// new caller anywhere else could open the boundary for a caller and a pin
/// no gate admitted.
#[test]
fn the_semantic_boundary_opens_only_behind_a_gate() {
    let root = crate::guards_root();
    let mut files = Vec::new();
    rust_files(&root.join(".."), &mut files);
    let guards = root.canonicalize().expect("crates/guards");
    let mut callers: Vec<String> = files
        .iter()
        .filter(|path| !is_test_file(path))
        .filter(|path| {
            !path
                .canonicalize()
                .map(|p| p.starts_with(&guards))
                .unwrap_or(false)
        })
        .filter(|path| {
            std::fs::read_to_string(path)
                .map(|text| production_calls(&text, BOUNDARY_CALL) > 0)
                .unwrap_or(false)
        })
        .map(|path| relative(&root, path))
        .collect();
    callers.sort();
    let mut expected: Vec<String> = SURFACES.iter().map(|s| s.to_string()).collect();
    expected.sort();
    assert_eq!(
        callers, expected,
        "`open_semantic_boundary` runs no gate, so only the surface wrappers that \
         run `check_custom_app_gates` first may call it. Enter through the \
         surface's `enter_semantic_boundary` instead."
    );
    for surface in SURFACES {
        let text = std::fs::read_to_string(root.join(surface)).expect("surface source");
        let wrapper = text
            .find("fn enter_semantic_boundary(")
            .unwrap_or_else(|| panic!("{surface} should define `enter_semantic_boundary`"));
        let boundary = wrapper
            + text[wrapper..]
                .find(SURFACE_CALL)
                .unwrap_or_else(|| panic!("{surface}'s wrapper should call `{SURFACE_CALL}`"));
        assert!(
            text[wrapper..boundary].contains(GATE_CALL),
            "{surface}: `enter_semantic_boundary` must run `{GATE_CALL}` before \
             `open_semantic_boundary`"
        );
    }
}

/// The derivation above, on fixtures: a call counts, its definition, a
/// comment and an inline test module do not.
#[test]
fn a_boundary_call_is_a_call_not_a_mention() {
    let source = [
        "pub async fn open_semantic_boundary(caller: GatedCaller<'_>) {}",
        "// open_semantic_boundary(app.gated()) in a comment",
        "fn enter() { open_semantic_boundary(app.gated()).await; }",
        "#[cfg(test)]",
        "mod tests {",
        "    fn t() { open_semantic_boundary(x); }",
        "}",
    ]
    .join("\n");
    assert_eq!(production_calls(&source, BOUNDARY_CALL), 1);
}
