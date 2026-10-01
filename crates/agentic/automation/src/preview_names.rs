//! Preview-scoped names: `preview:<run_id>:<name>`.
//!
//! Inside a workspace-preview run, every name that selects a side effect — the
//! root automation ref, each `execute_sql` `database`, each sub-automation
//! `src`, each agent ref, each airway `pipeline` — is re-emitted in this form
//! by the preview platform. Only a pod that knows previews, driving *that*
//! run, strips the prefix back off ([`unscope`]). A pod without that code
//! resolves none of them: the automation is `Missing`, the database is "not
//! found", and the run fails instead of running as production. That is what
//! makes a rolling deploy, or a rollback while preview runs are queued, safe.
//!
//! The prefix is therefore reserved everywhere a public caller can name one of
//! these things; see `agentic_airway::config::RESERVED_NAME_PREFIX` for the
//! pipeline-name half.

/// The reserved prefix.
pub const PREVIEW_PREFIX: &str = "preview:";

/// `preview:<run_id>:<name>`.
///
/// `run_id` must not contain `:` — run ids are UUIDs (children `uuid.N`), so
/// the first `:` after the prefix always ends it and [`parse_scoped`] is the
/// exact inverse.
pub fn scoped(run_id: &str, name: &str) -> String {
    debug_assert!(!run_id.contains(':'), "run id {run_id:?} contains ':'");
    format!("{PREVIEW_PREFIX}{run_id}:{name}")
}

/// Whether `value` carries the reserved prefix at all.
pub fn is_scoped(value: &str) -> bool {
    value.starts_with(PREVIEW_PREFIX)
}

/// `(run_id, name)` for a well-formed scoped name; `None` otherwise,
/// including for an empty run id or name.
pub fn parse_scoped(value: &str) -> Option<(&str, &str)> {
    let rest = value.strip_prefix(PREVIEW_PREFIX)?;
    let (run_id, name) = rest.split_once(':')?;
    (!run_id.is_empty() && !name.is_empty()).then_some((run_id, name))
}

/// `(run_id, verb)` for a scoped `http_request` method as the step executor
/// hands it on: it upper-cases the method first, so `preview:<run_id>:POST`
/// arrives as `PREVIEW:<RUN_ID>:POST`. The prefix is matched without regard
/// to case, and the run id comes back as it arrived (upper-cased) — compare
/// it with `eq_ignore_ascii_case`.
///
/// A scoped method is not an HTTP token (`:` is not a `tchar`), so a pod that
/// does not know previews fails to parse it and sends nothing.
pub fn parse_scoped_method(method: &str) -> Option<(&str, &str)> {
    let prefix = method.get(..PREVIEW_PREFIX.len())?;
    if !prefix.eq_ignore_ascii_case(PREVIEW_PREFIX) {
        return None;
    }
    let (run_id, verb) = method[PREVIEW_PREFIX.len()..].split_once(':')?;
    (!run_id.is_empty() && !verb.is_empty()).then_some((run_id, verb))
}

/// The unscoped name, only when `value` is scoped to exactly `run_id` — the
/// run this pod is driving. A name scoped to any other run is `None`, never
/// the bare name: one preview run cannot reach another's side effects, and a
/// forged prefix reaches nothing.
pub fn unscope<'a>(value: &'a str, run_id: &str) -> Option<&'a str> {
    parse_scoped(value).and_then(|(owner, name)| (owner == run_id).then_some(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_names_round_trip() {
        let run = "3f0b6c1e-8d2a-4c61-9d7e-0a1b2c3d4e5f";
        let name = "workflows/compute_toast_journal_entry_airhouse.procedure.yml";
        let s = scoped(run, name);
        assert_eq!(s, format!("preview:{run}:{name}"));
        assert!(is_scoped(&s));
        assert_eq!(parse_scoped(&s), Some((run, name)));
        assert_eq!(unscope(&s, run), Some(name));
        // A child run id keeps working; a name may itself contain ':'.
        assert_eq!(
            parse_scoped(&scoped("abc.1", "db:weird")),
            Some(("abc.1", "db:weird"))
        );
    }

    #[test]
    fn only_the_owning_run_can_unscope() {
        let s = scoped("run-a", "clickhouse");
        assert_eq!(unscope(&s, "run-b"), None);
        assert_eq!(
            unscope("clickhouse", "run-a"),
            None,
            "an unscoped name is not a scoped one"
        );
    }

    #[test]
    fn a_scoped_method_survives_upper_casing() {
        let run = "3f0b6c1e-8d2a-4c61-9d7e-0a1b2c3d4e5f";
        let upper = scoped(run, "POST").to_ascii_uppercase();
        let (owner, verb) = parse_scoped_method(&upper).expect("parses");
        assert!(owner.eq_ignore_ascii_case(run));
        assert_eq!(verb, "POST");
        assert_eq!(parse_scoped_method("POST"), None);
        assert_eq!(parse_scoped_method("PREVIEW:"), None);
        assert!(
            reqwest::Method::from_bytes(upper.as_bytes()).is_err(),
            "a scoped method must not be sendable"
        );
    }

    #[test]
    fn malformed_names_do_not_parse() {
        for bad in [
            "preview:",
            "preview::x",
            "preview:run:",
            "preview:run",
            "Preview:run:x",
            "x",
        ] {
            assert_eq!(parse_scoped(bad), None, "{bad}");
        }
    }
}
