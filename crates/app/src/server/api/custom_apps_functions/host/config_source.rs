//! Whether this node can say what the workspace's `config.yml` holds.
//!
//! The project context is built boundary-first: with a promoted revision the
//! config is the compiled one, on any pod. With none, the builder falls back
//! to `config.yml` in the working copy — and on a pod that holds no working
//! copy that file is not there, so the fallback hands back an EMPTY config.
//!
//! An empty config and an unknown one answer the same questions the same way:
//! no default database, no database by any name. Reporting the second as the
//! first is "absent reported as empty" — it told an app author to add a
//! database to a `config.yml` that already declares three.
//!
//! A route call is kept out of that state (`invocation_placement` sends it to the
//! Factory first), short of a database blip between that check and the build.
//! A scheduled, manual or webhook run on a diskless worker has no such gate,
//! so the host says what it knows.

use super::*;

impl ProjectFunctionHost {
    /// `Some(why)` when the workspace's databases are unknown on this node
    /// rather than absent from its config.
    ///
    /// Asks the filesystem, not the role flag: a process that declares no
    /// working copy but sits beside one (a standalone `oxy worker` on a dev
    /// box) did read the real `config.yml`, and its databases are known.
    pub(super) fn databases_unknown_here(&self) -> Option<String> {
        let cm = &self.proj_ctx.workspace_manager().config_manager;
        let compiled = cm.revision_id().is_some();
        let on_disk = !compiled && cm.working_copy().is_some_and(|w| w.root().is_dir());
        unknown_here(compiled, on_disk)
    }
}

fn unknown_here(compiled: bool, working_copy_on_disk: bool) -> Option<String> {
    if compiled || working_copy_on_disk {
        return None;
    }
    Some(
        "this workspace has no compiled revision, and this node holds no working copy to \
         read its config.yml from, so its databases are not known here; retry once the \
         workspace has compiled"
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::unknown_here;

    #[test]
    fn a_compiled_config_is_known_on_any_node() {
        assert_eq!(unknown_here(true, false), None);
        assert_eq!(unknown_here(true, true), None);
    }

    #[test]
    fn a_working_copy_answers_for_an_uncompiled_workspace() {
        assert_eq!(unknown_here(false, true), None);
    }

    #[test]
    fn neither_source_is_unknown_not_empty() {
        let why = unknown_here(false, false).expect("unknown");
        assert!(why.contains("retry"), "it must read as retryable: {why}");
        assert!(
            !why.contains("no databases configured"),
            "never the claim that the customer configured nothing: {why}"
        );
    }
}
