//! The publish check for an app's `widgets` block.
//!
//! Strict where the runtime read (`parse`) is lenient: the first problem
//! refuses the bundle, named, before anything is stored. Unknown keys are
//! refused, as in the notification catalogue: a misspelt `function` would
//! otherwise leave a button that runs nothing. `oxyc publish`, `oxyc validate`
//! and the vite plugin apply the same rules (`sdk/.../widgets.ts`).

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use super::model::{MAX_ACTIONS, MAX_TITLE_CHARS, MAX_WIDGETS, Size};
use crate::notifications::catalog::model::valid_segment;
use crate::notifications::catalog::parse::function_names;
pub use crate::notifications::catalog::validate::Problem;

const ID_RULE: &str = "lowercase letters, digits, `-` and `_`, starting with a letter or digit";

fn problem(check: &'static str, message: String, remediation: String) -> Problem {
    Problem {
        check,
        message,
        remediation,
    }
}

fn shape(message: String, remediation: String) -> Problem {
    problem("widget_invalid", message, remediation)
}

/// Check `manifest`'s `widgets` block.
pub fn check(manifest: Option<&Value>) -> Result<(), Problem> {
    let Some(block) = manifest.and_then(|m| m.get("widgets")) else {
        return Ok(());
    };
    let widgets = block.as_object().ok_or_else(|| {
        shape(
            "`widgets` must be an object keyed by widget id.".into(),
            "Write it as \"widgets\": { \"<id>\": { \"title\": …, \"function\": … } }.".into(),
        )
    })?;
    if widgets.len() > MAX_WIDGETS {
        return Err(problem(
            "widget_too_many",
            format!(
                "The app declares {} widgets; the home shows at most {MAX_WIDGETS} per app.",
                widgets.len()
            ),
            format!("Keep the {MAX_WIDGETS} that matter most."),
        ));
    }
    let functions = function_names(manifest);
    for (id, def) in widgets {
        check_widget(id, def, &functions)?;
    }
    Ok(())
}

fn check_widget(id: &str, def: &Value, functions: &BTreeSet<String>) -> Result<(), Problem> {
    let at = format!("widgets.{id}");
    if !valid_segment(id) {
        return Err(shape(
            format!("`{id}` is not a widget id: use {ID_RULE}."),
            format!("Rename `{at}`."),
        ));
    }
    let obj = object(def, &at)?;
    only_keys(obj, &at, &["title", "function", "size", "actions"])?;
    bounded_text(obj.get("title"), &format!("{at}.title"), MAX_TITLE_CHARS)?;
    let function = required_text(obj.get("function"), &format!("{at}.function"))?;
    if !functions.contains(function) {
        return Err(problem(
            "widget_function_undeclared",
            format!("`{at}.function` names `{function}`, which the manifest does not declare."),
            format!("Declare `{function}` under `functions`, or name a declared function."),
        ));
    }
    if let Some(size) = obj.get("size")
        && size.as_str().and_then(Size::parse).is_none()
    {
        return Err(shape(
            format!("`{at}.size` must be \"regular\" or \"wide\"."),
            format!("Fix `{at}.size`, or leave it out for \"regular\"."),
        ));
    }
    match obj.get("actions") {
        None => Ok(()),
        Some(actions) => check_actions(actions, &at, functions),
    }
}

fn check_actions(actions: &Value, at: &str, functions: &BTreeSet<String>) -> Result<(), Problem> {
    let at = format!("{at}.actions");
    let actions = object(actions, &at)?;
    if actions.len() > MAX_ACTIONS {
        return Err(problem(
            "widget_actions_too_many",
            format!(
                "`{at}` declares {} actions; a widget takes at most {MAX_ACTIONS}.",
                actions.len()
            ),
            format!("Keep at most {MAX_ACTIONS}; the rest belong in the app."),
        ));
    }
    for (id, def) in actions {
        let at = format!("{at}.{id}");
        let invalid = |message: String| {
            problem(
                "widget_action_invalid",
                message,
                format!(
                    "Fix `{at}`: {{ \"function\": …, \"confirm\"?: bool, \"destructive\"?: bool }}."
                ),
            )
        };
        if !valid_segment(id) {
            return Err(invalid(format!(
                "`{id}` is not an action id: use {ID_RULE}."
            )));
        }
        let obj = def
            .as_object()
            .ok_or_else(|| invalid(format!("`{at}` must be an object.")))?;
        if let Some(key) = obj
            .keys()
            .find(|k| !["function", "confirm", "destructive"].contains(&k.as_str()))
        {
            return Err(invalid(format!("`{at}.{key}` is not a field.")));
        }
        for flag in ["confirm", "destructive"] {
            if obj.get(flag).is_some_and(|v| !v.is_boolean()) {
                return Err(invalid(format!("`{at}.{flag}` must be true or false.")));
            }
        }
        let function = obj
            .get("function")
            .and_then(Value::as_str)
            .filter(|f| !f.trim().is_empty())
            .ok_or_else(|| invalid(format!("`{at}.function` must name a function.")))?;
        if !functions.contains(function) {
            return Err(problem(
                "widget_action_function_undeclared",
                format!("`{at}.function` names `{function}`, which the manifest does not declare."),
                format!("Declare `{function}` under `functions`, or name a declared function."),
            ));
        }
    }
    Ok(())
}

fn object<'a>(v: &'a Value, at: &str) -> Result<&'a Map<String, Value>, Problem> {
    v.as_object()
        .ok_or_else(|| shape(format!("`{at}` must be an object."), format!("Fix `{at}`.")))
}

fn only_keys(obj: &Map<String, Value>, at: &str, allowed: &[&str]) -> Result<(), Problem> {
    match obj.keys().find(|k| !allowed.contains(&k.as_str())) {
        None => Ok(()),
        Some(key) => Err(shape(
            format!(
                "`{at}.{key}` is not a field; a widget takes {}.",
                allowed.join(", ")
            ),
            format!("Remove or rename `{at}.{key}`."),
        )),
    }
}

fn required_text<'a>(v: Option<&'a Value>, at: &str) -> Result<&'a str, Problem> {
    v.and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            shape(
                format!("`{at}` must be a non-empty string."),
                format!("Fix `{at}`."),
            )
        })
}

fn bounded_text(v: Option<&Value>, at: &str, max: usize) -> Result<(), Problem> {
    let text = required_text(v, at)?;
    match text.chars().count() <= max {
        true => Ok(()),
        false => Err(shape(
            format!("`{at}` must be at most {max} characters."),
            format!("Shorten `{at}`."),
        )),
    }
}
