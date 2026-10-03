//! Shared Jinja rendering + workspace path containment helpers.
//!
//! Used by [`step_executor`](crate::step_executor) (for `sql_file` paths
//! and inline SQL) and [`export`](crate::export) (for `export.path`).
//! Lifted into one module so a future divergence — e.g. one renderer
//! sandboxing templates differently — doesn't sneak in unnoticed.
//!
//! ## Path containment
//!
//! Automation authors are trusted, but the data flowing through Jinja
//! isn't. An automation that does `sql_file: "data/{{ loop.value }}.sql"`
//! over a list whose values come from a SQL query will substitute
//! whatever the query returned — and if any row contains `../etc/passwd`
//! we'd happily read it (or, in the export case, write to it). The
//! same path-traversal concern that closed
//! [`OxyProjectContext::resolve_automation_yaml`][rwy] applies here.
//!
//! Containment is enforced syntactically (no `canonicalize`): we
//! reject empty paths, absolute paths, and any path containing a `..`
//! component. The export writer also targets paths that don't exist
//! yet, so the canonicalisation-based check `validate_path_within_project`
//! uses for read-only paths isn't applicable here.
//!
//! [rwy]: agentic_wiring::OxyProjectContext::resolve_automation_yaml

use std::path::{Path, PathBuf};

use agentic_connector::StringLiteral;
use serde_json::Value;

/// Build a chainable-undefined minijinja [`Environment`] preloaded with
/// the automation filter set, for text that is not sent to a database.
///
/// Used for step bodies, formatters, and any user-authored template
/// where a typo'd path should expand to empty rather than fail — this
/// matches the legacy `oxy_core::exec_runtime::renderer::setup_jinja_environment`
/// behaviour so templates ported from the previous automation engine
/// keep rendering. The filters registered are the same as
/// [`automation_env_strict`]:
///
/// - `now(utc?, fmt?)` — current datetime, RFC3339 by default.
/// - `tojson` — serialize to a compact JSON string.
/// - `sqlquote` — write as a SQL string literal (`Paris` → `'Paris'`).
///   Templates should NOT add their own surrounding quotes when using
///   this filter. No engine reads what this environment renders, so a
///   value holding a quote or a backslash is refused — see
///   [`automation_env_for`], which is the environment SQL is rendered in.
pub(crate) fn automation_env() -> minijinja::Environment<'static> {
    automation_env_for(None)
}

/// [`automation_env`] for SQL sent to an engine that reads a literal by
/// `literal`: `sqlquote` escapes a value the way that engine reads it.
///
/// One quote-doubling filter for every engine is what this replaces. On an
/// engine that reads a backslash as an escape, a value ending in `\`
/// swallowed the closing quote and a `\'` ended the literal early, so a
/// control value or an automation variable could run as SQL.
///
/// `None` is an engine nobody could name. A value with no quote and no
/// backslash is written — it is the same bytes under every rule — and any
/// other is refused: no spelling of it is read as that value everywhere
/// (`oxy_shared::sql_literal::quote_engine_unknown`).
pub(crate) fn automation_env_for(
    literal: Option<StringLiteral>,
) -> minijinja::Environment<'static> {
    let mut env = minijinja::Environment::new();
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Chainable);
    add_global_helpers(&mut env, literal);
    env
}

/// Build a strict-undefined minijinja [`Environment`] with the same
/// filter set as [`automation_env`].
///
/// Used for runtime expressions where silent typos are unacceptable —
/// e.g. `loop_sequential.values: "{{ intervals.list }}"`, where an
/// empty resolution silently turns the loop into a no-op. Errors on
/// undefined access; callers translate the error into a clear "did
/// not resolve" / "available keys" message.
pub(crate) fn automation_env_strict() -> minijinja::Environment<'static> {
    let mut env = minijinja::Environment::new();
    add_global_helpers(&mut env, None);
    env
}

/// `value` as a `'…'` literal under `literal`, or by the fail-closed rule
/// when the engine is not known.
fn sqlquote(literal: Option<StringLiteral>, value: &str) -> Result<String, minijinja::Error> {
    match literal {
        Some(rule) => Ok(rule.quote(value)),
        // The refusal travels as the error's source, so a caller that treats
        // a failed render as "false" can tell this failure from the others.
        None => oxy_shared::sql_literal::quote_engine_unknown(value).map_err(|refused| {
            minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, refused.to_string())
                .with_source(refused)
        }),
    }
}

fn add_global_helpers(env: &mut minijinja::Environment<'static>, literal: Option<StringLiteral>) {
    use chrono::{DateTime, Local, Utc};

    env.add_function(
        "now",
        |kwargs: minijinja::value::Kwargs| -> Result<String, minijinja::Error> {
            let utc = kwargs.get::<Option<bool>>("utc")?.unwrap_or(false);
            let fmt = kwargs.get::<Option<String>>("fmt")?;
            let out = if utc {
                let now: DateTime<Utc> = Utc::now();
                match fmt {
                    Some(f) => now.format(&f).to_string(),
                    None => now.to_rfc3339(),
                }
            } else {
                let now: DateTime<Local> = Local::now();
                match fmt {
                    Some(f) => now.format(&f).to_string(),
                    None => now.to_rfc3339(),
                }
            };
            Ok(out)
        },
    );

    env.add_filter(
        "tojson",
        |value: minijinja::Value| -> Result<String, minijinja::Error> {
            serde_json::to_string(&value).map_err(|e| {
                minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    format!("Failed to convert to JSON: {e}"),
                )
            })
        },
    );

    // SQL string literal, escaped as the engine the SQL is sent to reads
    // one (see `automation_env_for`).
    //   {{ controls.store | sqlquote }}  →  'O''Brien'  (DuckDB, Postgres)
    //                                    →  'O\'Brien'  (BigQuery)
    // Templates must NOT add surrounding quotes themselves.
    env.add_filter(
        "sqlquote",
        move |value: minijinja::Value| -> Result<String, minijinja::Error> {
            sqlquote(literal, &value.to_string())
        },
    );

    // Base64 (standard) encode — primarily for HTTP Basic auth headers in
    // `http_request` tasks, e.g.
    //   Authorization: "Basic {{ (secrets.ID ~ ':' ~ secrets.SECRET) | b64encode }}"
    env.add_filter(
        "b64encode",
        |value: minijinja::Value| -> Result<String, minijinja::Error> {
            use base64::Engine;
            Ok(base64::engine::general_purpose::STANDARD.encode(value.to_string().as_bytes()))
        },
    );
}

/// Render a Jinja template string against the given context.
///
/// For text that is not sent to a database — paths, prompts, HTTP parts,
/// values handed to another automation. See [`automation_env`] for the
/// filters / functions available to templates. Returns `Err` with a parse
/// or render error on failure; missing keys are forgiven (chainable
/// undefined).
pub(crate) fn render_jinja_string(template: &str, context: &Value) -> Result<String, String> {
    render_sql_string(template, context, None)
}

/// [`render_jinja_string`] for SQL — or a value on its way into SQL — sent
/// to an engine that reads a literal by `literal`
/// ([`WorkspaceContext::string_literal`] for the task's `database`).
///
/// [`WorkspaceContext::string_literal`]: crate::workspace::WorkspaceContext::string_literal
pub(crate) fn render_sql_string(
    template: &str,
    context: &Value,
    literal: Option<StringLiteral>,
) -> Result<String, String> {
    render_sql_checked(template, context, literal).map_err(|e| e.message)
}

/// A render failure, plus whether it was specifically `sqlquote` refusing a
/// value because the engine is unnamed — as opposed to any other render
/// failure (a bad template, an unknown filter) that happens to occur while
/// the engine is *also* unnamed. Only the former is about the engine; a
/// caller that blames the database for every failure on an unnamed engine
/// would mislabel the latter.
pub(crate) struct SqlRenderError {
    message: String,
    pub(crate) is_quote_refusal: bool,
}

impl std::fmt::Display for SqlRenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// [`render_sql_string`], reporting whether the failure was a `sqlquote`
/// refusal rather than stringifying that distinction away.
pub(crate) fn render_sql_checked(
    template: &str,
    context: &Value,
    literal: Option<StringLiteral>,
) -> Result<String, SqlRenderError> {
    let env = automation_env_for(literal);
    let tmpl = env
        .template_from_str(template)
        .map_err(|e| SqlRenderError {
            message: format!("template parse error: {e}"),
            is_quote_refusal: false,
        })?;
    let ctx = crate::step_orchestrator::build_minijinja_context(context);
    tmpl.render(&ctx).map_err(|e| SqlRenderError {
        is_quote_refusal: sqlquote_refused(&e),
        message: format!("render error: {e}"),
    })
}

/// Resolve a workspace-relative path, rejecting traversal attempts.
///
/// Returns `workspace.join(relative)` when:
/// - `relative` is non-empty,
/// - `relative` is not absolute,
/// - `relative` contains no `..` components.
///
/// Otherwise returns `Err` with a user-readable message.
///
/// Containment is purely syntactic — see the module docs for the
/// rationale and threat model.
pub(crate) fn validate_workspace_relative_path(
    workspace: &Path,
    relative: &str,
) -> Result<PathBuf, String> {
    validate_workspace_relative_ref(relative)?;
    Ok(workspace.join(Path::new(relative)))
}

/// The containment rule above, without a root to join to.
///
/// Split out because the same ref is now used two ways: as a **lookup key**
/// into the compile boundary (`WorkspaceContext::resolve_sql_file`) and, only
/// on a miss, as a filesystem path. A `../` ref has to be rejected before
/// either, so the check cannot live inside the join — and it must stay ONE
/// definition, because a boundary lookup that accepted what the filesystem
/// path rejects would be a containment hole reachable without touching disk.
///
/// Purely syntactic: empty, absolute, and any `..` component. Symlink
/// containment is the caller's job and still needs the canonicalising check on
/// the filesystem side.
pub(crate) fn validate_workspace_relative_ref(relative: &str) -> Result<(), String> {
    normalize_workspace_relative_ref(relative).map(|_| ())
}

/// Validate as above, and return the ref in the **compile boundary's** spelling:
/// forward slashes, no `./` prefix, no redundant `.` components.
///
/// The normalisation is not cosmetic. `verified_queries.file_path` holds the
/// walker's rel-path (`crates/oxy-compile/src/walker.rs` — "workspace-relative
/// path (forward slashes)"), which is never `./`-prefixed. A ref spelled
/// `./sql/rollup.sql` is perfectly legal input — `sql_file` has always accepted
/// it, and a Jinja-rendered path can easily produce it — but as a lookup key it
/// MISSES a row that exists. The caller then falls through to the filesystem
/// and fails on a node without one, which is precisely the instance affinity
/// the boundary removes, reachable by a spelling.
///
/// So the same string must be normalised once and used for BOTH the row lookup
/// and the filesystem join. Returning it from the validator is what keeps those
/// two uses from drifting apart.
///
/// `Component::CurDir` is dropped rather than rejected: `./x.sql` and `x.sql`
/// name the same file, and refusing one of them would be a behaviour break for
/// automations that already use it (pinned by
/// `render_validates_dot_slash_paths`).
pub(crate) fn normalize_workspace_relative_ref(relative: &str) -> Result<String, String> {
    if relative.is_empty() {
        return Err("path is empty".into());
    }
    let candidate = Path::new(relative);
    if candidate.is_absolute() {
        return Err(format!(
            "path {relative:?} must be relative to the workspace"
        ));
    }

    let mut parts: Vec<&str> = Vec::new();
    for component in candidate.components() {
        match component {
            std::path::Component::ParentDir => {
                return Err(format!("path {relative:?} must not contain `..` segments"));
            }
            // `./a/./b` and `a/b` address the same file; the boundary key is
            // the latter.
            std::path::Component::CurDir => {}
            std::path::Component::Normal(part) => {
                parts.push(
                    part.to_str()
                        .ok_or_else(|| format!("path {relative:?} is not valid UTF-8"))?,
                );
            }
            // Unreachable for a relative path that is not absolute, but a
            // silent `_ => {}` here would drop a prefix/root component and
            // quietly rewrite the path.
            other => {
                return Err(format!(
                    "path {relative:?} contains an unsupported component {other:?}"
                ));
            }
        }
    }
    if parts.is_empty() {
        // e.g. "." — syntactically fine, addresses no file.
        return Err(format!("path {relative:?} names no file"));
    }
    Ok(parts.join("/"))
}

/// Decide whether a rendered condition expression counts as true.
///
/// Automation conditions are evaluated by rendering `{{ <condition> }}`
/// and inspecting the resulting text, so the falsy set has to be spelled
/// out. Falsy is: empty, `false`, `0`, and `none`.
///
/// **The comparisons are deliberately case-insensitive.** minijinja
/// renders scalars Python-style — 2.23 emits `False` / `None` where 2.20
/// emitted `false` / `none` — and a case-sensitive check silently made
/// *every* condition truthy across that upgrade, so `conditional` steps
/// always took their first branch. Matching both spellings keeps this
/// independent of which side of that change the pinned minijinja is on.
pub(crate) fn condition_is_truthy(rendered: &str) -> bool {
    let trimmed = rendered.trim();
    !trimmed.is_empty()
        && !trimmed.eq_ignore_ascii_case("false")
        && trimmed != "0"
        && !trimmed.eq_ignore_ascii_case("none")
}

/// Whether `condition` holds in `ctx`.
///
/// A condition that fails to render is false, as it always was, and is now
/// logged. One failure is not: `sqlquote` refusing a value it cannot write
/// with no engine named. That is the step's error — falling through to the
/// next branch would run the wrong tasks for a reason nobody could see.
pub(crate) fn condition_holds(
    env: &minijinja::Environment<'_>,
    condition: &str,
    ctx: &minijinja::Value,
) -> Result<bool, String> {
    let source = format!("{{{{{condition}}}}}");
    let tmpl = env
        .template_from_str(&source)
        .map_err(|e| format!("condition parse error: {e}"))?;
    let rendered = match tmpl.render(ctx) {
        Ok(text) => text,
        Err(e) if sqlquote_refused(&e) => return Err(format!("condition `{condition}`: {e}")),
        Err(e) => {
            tracing::warn!(%condition, error = %e, "condition failed to render; treated as false");
            String::new()
        }
    };
    Ok(condition_is_truthy(&rendered))
}

/// Whether a render failed because `sqlquote` refused its value.
fn sqlquote_refused(error: &minijinja::Error) -> bool {
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        if cause.is::<oxy_shared::sql_literal::EngineUnknown>() {
            return true;
        }
        source = cause.source();
    }
    false
}

#[cfg(test)]
#[path = "render_sqlquote_tests.rs"]
mod sqlquote_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn condition_truthiness_covers_both_minijinja_spellings() {
        // Falsy — lowercase (minijinja <= 2.22) and capitalized (>= 2.23).
        for falsy in [
            "", "  ", "false", "False", "FALSE", "0", "none", "None", "NONE",
        ] {
            assert!(
                !condition_is_truthy(falsy),
                "expected {falsy:?} to be falsy"
            );
        }

        // Truthy — including the surrounding whitespace a render can leave.
        for truthy in ["true", "True", "1", "-1", "0.0", "text", " True "] {
            assert!(
                condition_is_truthy(truthy),
                "expected {truthy:?} to be truthy"
            );
        }
    }

    #[test]
    fn jinja_renders_simple_substitution() {
        let ctx = json!({"x": 42, "name": "world"});
        assert_eq!(render_jinja_string("v={{ x }}", &ctx).unwrap(), "v=42");
        assert_eq!(
            render_jinja_string("hello {{ name }}", &ctx).unwrap(),
            "hello world"
        );
    }

    /// Regression: agent prompts reference SQL step results as
    /// `{{ execute_step.col[0] }}` / `{{ execute_step }}`. The
    /// decider now Jinja-renders the prompt against the parent
    /// context before dispatching to the agent (without this, the
    /// LLM receives the raw template syntax and complains the data
    /// wasn't included). This test pins the column-table access shape
    /// the demo automations depend on.
    #[test]
    fn jinja_renders_agent_prompt_with_column_table_access() {
        // Mirror what `to_column_oriented` produces for a SQL step.
        let ctx = json!({
            "execute_portfolio_summary": {
                "views": [1561132i64],
                "minutes": [6187523i64],
                "mom_views_perc": [14.3],
                "mom_minutes_perc": [-7.8],
                "__row_count__": 1,
                "__columns__": ["views", "minutes", "mom_views_perc", "mom_minutes_perc"],
            }
        });
        let prompt = "v={{ execute_portfolio_summary.views[0] }} \
                      m={{ execute_portfolio_summary.minutes[0] }} \
                      mv={{ execute_portfolio_summary.mom_views_perc[0] }} \
                      mm={{ execute_portfolio_summary.mom_minutes_perc[0] }}";
        let rendered = render_jinja_string(prompt, &ctx).unwrap();
        assert_eq!(rendered, "v=1561132 m=6187523 mv=14.3 mm=-7.8");
    }

    #[test]
    fn jinja_renders_missing_keys_as_empty() {
        // Chainable: `foo.bar` when `foo` is undefined → empty.
        let ctx = json!({});
        assert_eq!(render_jinja_string("x={{ foo.bar }}", &ctx).unwrap(), "x=");
    }

    #[test]
    fn validate_accepts_simple_relative() {
        let p = validate_workspace_relative_path(Path::new("/ws"), "data/x.sql").unwrap();
        assert_eq!(p, Path::new("/ws/data/x.sql"));
    }

    #[test]
    fn validate_rejects_empty() {
        let err = validate_workspace_relative_path(Path::new("/ws"), "").unwrap_err();
        assert!(err.contains("empty"));
    }

    #[test]
    fn validate_rejects_absolute() {
        for p in ["/etc/passwd", "/tmp/escape.csv"] {
            let err = validate_workspace_relative_path(Path::new("/ws"), p).unwrap_err();
            assert!(err.contains("relative"), "for {p:?}: {err}");
        }
    }

    #[test]
    fn validate_rejects_parent_dir_traversal() {
        for p in ["../etc/passwd", "data/../../etc", "..", "a/../b", "./../x"] {
            let err = validate_workspace_relative_path(Path::new("/ws"), p);
            assert!(err.is_err(), "should reject {p:?}, got {err:?}");
        }
    }

    /// The root-free half must reject exactly what the joining half rejects.
    ///
    /// `sql_file` now uses the ref TWICE — once as a compile-boundary lookup
    /// key, and only on a miss as a filesystem path. If these two ever diverge,
    /// a ref the filesystem path refuses could still be used to address a
    /// compiled row, which is a containment hole reachable without touching
    /// disk. Asserting them against each other is what keeps the split honest;
    /// asserting the new one alone would not.
    #[test]
    fn the_root_free_check_agrees_with_the_joining_one() {
        let cases = [
            "../etc/passwd",
            "data/../../etc",
            "..",
            "a/../b",
            "./../x",
            "/etc/passwd",
            "",
            "data/x.sql",
            "./data/x.sql",
            "queries/sales_daily_rollup.sql",
        ];
        for p in cases {
            let joined = validate_workspace_relative_path(Path::new("/ws"), p);
            let bare = validate_workspace_relative_ref(p);
            assert_eq!(
                joined.is_ok(),
                bare.is_ok(),
                "{p:?}: joining={joined:?} bare={bare:?}"
            );
        }
    }

    /// The normalised form is the compile boundary's key, so it must match the
    /// walker's spelling exactly: forward slashes, no `./`, no bare `.`
    /// components. A ref that normalises wrongly misses a row that exists and
    /// falls back to a filesystem the node may not have.
    #[test]
    fn normalisation_produces_the_walker_spelling() {
        for (input, want) in [
            ("sql/rollup.sql", "sql/rollup.sql"),
            ("./sql/rollup.sql", "sql/rollup.sql"),
            ("./sql/./rollup.sql", "sql/rollup.sql"),
            ("sql/./rollup.sql", "sql/rollup.sql"),
            ("rollup.sql", "rollup.sql"),
        ] {
            assert_eq!(
                normalize_workspace_relative_ref(input).unwrap(),
                want,
                "for {input:?}"
            );
        }
    }

    /// `.` alone is syntactically clean but addresses no file. Left to the
    /// join it would silently become the workspace root.
    #[test]
    fn a_ref_naming_no_file_is_rejected() {
        assert!(normalize_workspace_relative_ref(".").is_err());
        assert!(normalize_workspace_relative_ref("./").is_err());
    }

    /// `tojson` filter round-trips a value to its JSON encoding.
    #[test]
    fn tojson_filter_serializes() {
        let ctx = json!({"x": [1, 2, 3], "y": "abc"});
        assert_eq!(
            render_jinja_string("{{ x | tojson }}", &ctx).unwrap(),
            "[1,2,3]"
        );
        assert_eq!(
            render_jinja_string("{{ y | tojson }}", &ctx).unwrap(),
            "\"abc\""
        );
    }

    /// `b64encode` standard-encodes a string — used to build HTTP Basic auth
    /// headers in `http_request` tasks.
    #[test]
    fn b64encode_filter_encodes() {
        let ctx = json!({"id": "abc", "secret": "s3cr3t"});
        assert_eq!(
            render_jinja_string("{{ (id ~ ':' ~ secret) | b64encode }}", &ctx).unwrap(),
            "YWJjOnMzY3IzdA==",
        );
    }

    /// `now()` is callable. Don't pin a value (it changes every call) —
    /// just check the format-string variant produces the expected width.
    #[test]
    fn now_function_with_format_renders_fixed_width() {
        let ctx = json!({});
        let out = render_jinja_string("{{ now(fmt='%Y-%m-%d') }}", &ctx).unwrap();
        assert_eq!(out.len(), 10, "expected YYYY-MM-DD, got {out:?}");
        assert_eq!(out.chars().filter(|c| *c == '-').count(), 2);
    }

    #[test]
    fn validate_allows_current_dir_prefix() {
        // `./data/x.sql` has only `CurDir` + `Normal` components, no
        // `ParentDir` — allowed.
        let p = validate_workspace_relative_path(Path::new("/ws"), "./data/x.sql").unwrap();
        assert!(p.starts_with("/ws"));
    }
}
