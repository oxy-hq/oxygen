//! The world-model module's source rule, asserted where it is declared.

/// `src` with `//` comments removed. The rule below is about code, and this
/// module's own docs name the very calls it forbids.
fn code(src: &str) -> String {
    src.lines()
        .map(|line| line.split("//").next().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every handler-side file of the module: comments stripped, test modules cut.
fn module_code() -> Vec<(&'static str, String)> {
    [
        ("handlers.rs", include_str!("handlers.rs")),
        ("query.rs", include_str!("query.rs")),
        ("source.rs", include_str!("source.rs")),
    ]
    .into_iter()
    .map(|(name, src)| {
        let src = src.split("\n#[cfg(test)]").next().unwrap_or_default();
        (name, code(src))
    })
    .collect()
}

/// These routes are `FleetOk`. A handler that scans the working copy compiles
/// fine and fails only on a replica — and with an empty model rather than an
/// error — so the shape is guarded here, as `metric_tree` guards its own
/// (`handlers_never_scan_the_working_copy`).
#[test]
fn nothing_here_reads_the_working_copy_directly() {
    for (file, code) in module_code() {
        assert!(!code.is_empty(), "{file} read back empty");
        for forbidden in [
            "semantics_scan_path",
            "working_copy_key",
            "WorkspaceManagerWorkingCopy",
        ] {
            assert!(
                !code.contains(forbidden),
                "world_model_graph/{file} uses `{forbidden}`: resolve the scan through \
                 `source::ModelSource` (compile boundary first) and key caches by what it read"
            );
        }
    }
}

/// The stripper: a forbidden call must be seen, a comment naming it must not.
#[test]
fn the_scan_sees_code_and_not_comments() {
    assert!(code("let p = cm.semantics_scan_path();").contains("semantics_scan_path"));
    assert!(!code("// never semantics_scan_path() here").contains("semantics_scan_path"));
    assert!(!code("let x = 1; // semantics_scan_path").contains("semantics_scan_path"));
}
