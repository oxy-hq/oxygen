//! The compiled arm and the working-copy arm must derive the same name for the
//! same file, or a workspace shows different app names depending on whether it
//! has been compiled.
//!
//! `oxy_compile::compile::derive_name_from_path` is the rule the compile worker
//! writes into `*_definitions.name`. `oxy::config::artifact_name` is the copy the
//! filesystem arm uses, because `crates/core` cannot depend on `oxy-compile`
//! without a new cross-layer edge. Two copies drift; this is what stops them.
//!
//! `oxy-app` is the only crate that depends on both.

#[test]
fn the_two_name_derivations_agree() {
    for path in [
        "agents/foo.agentic.yml",
        "views/v.view.yml",
        "topics/t.topic.yml",
        "p/x.procedure.yml",
        "p/z.automation.yml",
        "pipe/a.airway.yml",
        "worlds/flat.simulation.yml",
        "plain.yml",
        "noext",
        "",
    ] {
        assert_eq!(
            oxy::config::artifact_name(path),
            oxy_compile::compile::derive_name_from_path(path),
            "the two name derivations disagree on `{path}`. `*_definitions.name` \
             is written by the compile worker and read back by the filesystem \
             arm's fallback — a mismatch means the same app is listed under two \
             different names depending on whether the workspace is compiled."
        );
    }
}
