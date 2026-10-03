//! A Data App's control values: what its author declared, and what a viewer
//! chose.
//!
//! Two people write the text that ends up in `controls.*`, and only one of
//! them writes templates. The app's **author** writes the `.app.yml`, so a
//! control's `default:` (and each of its `options:`) may be a Jinja
//! expression — `default: "{{ now(fmt='%Y-%m-%d') }}"`. A **viewer** sends a
//! value in the run request, and that value is data: whatever characters it
//! holds, `{{` included, are the value.

use std::collections::HashMap;

use oxy::config::model::{AppConfig, ControlConfig, Display};
use oxy::exec_runtime::renderer::Renderer;
use serde_json::Value as JsonValue;

/// Render the Jinja in a control field its **author** wrote (`default`, an
/// entry of `options`).
///
/// Supports global functions such as `now()`:
///
/// ```yaml
/// - type: control
///   name: start_date
///   control_type: date
///   default: "{{ now(fmt='%Y-%m-%d') }}"
/// ```
///
/// Non-string values and strings without Jinja tokens are returned unchanged.
/// Rendering errors are logged as warnings and the original value is returned.
///
/// Never hand this a value from a request. It evaluates its argument: a
/// viewer who can reach it runs a template on the server.
pub fn render_control_default(val: JsonValue) -> JsonValue {
    let JsonValue::String(ref s) = val else {
        return val;
    };
    if !s.contains("{{") && !s.contains("{%") {
        return val;
    }
    let renderer = Renderer::new(minijinja::Value::UNDEFINED);
    match renderer.render_str(s) {
        Ok(rendered) => JsonValue::String(rendered),
        Err(e) => {
            tracing::warn!("Failed to render Jinja in control default '{s}': {e}");
            val
        }
    }
}

/// Every control an app declares: the top-level `controls:` plus any inline
/// `- type: control` / `- type: controls` items in the `display:` list.
pub(super) fn declared_controls(config: &AppConfig) -> Vec<ControlConfig> {
    let mut all = config.controls.clone();
    for display in &config.display {
        match display {
            Display::Control(c) => all.push(ControlConfig::from(c.clone())),
            Display::Controls(cs) => all.extend(cs.items.iter().cloned()),
            _ => {}
        }
    }
    all
}

/// The value of each declared control for one run: the viewer's, or the
/// author's default where the viewer sent none.
///
/// Only the default is rendered. The viewer's value used to go through
/// [`render_control_default`] too, so a param of `{{ 7 * 7 }}` became `49`:
/// any workspace member could run a template on the server — its clock, a
/// loop with no bound on the request's thread, a `now(fmt=…)` that panics.
///
/// An empty-string param is treated as absent, so the configured default is
/// used — it avoids injecting `''` into a typed SQL column. A param naming no
/// declared control is dropped.
pub(super) fn control_values(
    controls: &[ControlConfig],
    params: &HashMap<String, JsonValue>,
) -> HashMap<String, JsonValue> {
    controls
        .iter()
        .map(|c| {
            let supplied = params.get(&c.name).filter(|v| v.as_str() != Some(""));
            let value = match supplied {
                // The viewer's: data, taken as sent.
                Some(sent) => sent.clone(),
                // The author's: a template, rendered once.
                None => c
                    .default
                    .clone()
                    .map(render_control_default)
                    .unwrap_or(JsonValue::Null),
            };
            (c.name.clone(), value)
        })
        .collect()
}
