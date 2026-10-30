//! The runtime read of a live build's `widgets`.
//!
//! Lenient: publish already refused a malformed block (`validate`), so a
//! widget that still does not parse (a build from before the check) is left
//! out rather than failing the whole home.

use serde_json::Value;

use super::model::{ActionDef, MAX_ACTIONS, MAX_WIDGETS, Size, WidgetDef};
use crate::notifications::catalog::model::valid_segment;
use crate::notifications::catalog::parse::function_names;

/// The widgets `manifest` declares, in manifest order.
pub fn widgets(manifest: Option<&Value>) -> Vec<WidgetDef> {
    let Some(block) = manifest
        .and_then(|m| m.get("widgets"))
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };
    let functions = function_names(manifest);
    block
        .iter()
        .filter_map(|(id, def)| widget(id, def, &functions))
        .take(MAX_WIDGETS)
        .collect()
}

fn widget(
    id: &str,
    def: &Value,
    functions: &std::collections::BTreeSet<String>,
) -> Option<WidgetDef> {
    if !valid_segment(id) {
        return None;
    }
    let title = text(def.get("title"))?;
    let function = text(def.get("function")).filter(|f| functions.contains(f))?;
    let size = match def.get("size").and_then(Value::as_str) {
        None => Size::Regular,
        Some(s) => Size::parse(s)?,
    };
    let mut sizes: Vec<Size> = match def.get("sizes").and_then(Value::as_array) {
        None => vec![size],
        Some(list) => list
            .iter()
            .filter_map(|v| v.as_str().and_then(Size::parse))
            .collect(),
    };
    if !sizes.contains(&size) {
        sizes.push(size);
    }
    sizes.sort();
    sizes.dedup();
    let actions = def
        .get("actions")
        .and_then(Value::as_object)
        .map(|actions| {
            actions
                .iter()
                .filter_map(|(id, def)| action(id, def, functions))
                .take(MAX_ACTIONS)
                .collect()
        })
        .unwrap_or_default();
    Some(WidgetDef {
        id: id.to_string(),
        title,
        function,
        size,
        sizes,
        params: super::params::parse(def.get("params")),
        actions,
    })
}

fn action(
    id: &str,
    def: &Value,
    functions: &std::collections::BTreeSet<String>,
) -> Option<ActionDef> {
    if !valid_segment(id) {
        return None;
    }
    let function = text(def.get("function")).filter(|f| functions.contains(f))?;
    let flag = |k: &str| def.get(k).and_then(Value::as_bool).unwrap_or(false);
    Some(ActionDef {
        id: id.to_string(),
        function,
        confirm: flag("confirm"),
        destructive: flag("destructive"),
    })
}

fn text(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_widget_naming_an_undeclared_function_is_left_out() {
        let manifest = json!({
            "functions": { "close-widget": {}, "post": {} },
            "widgets": {
                "close": {
                    "title": "Close", "function": "close-widget", "size": "wide",
                    "actions": {
                        "post": { "function": "post", "confirm": true },
                        "gone": { "function": "missing" }
                    }
                },
                "ghost": { "title": "Ghost", "function": "missing" }
            }
        });
        let got = widgets(Some(&manifest));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].size, Size::Wide);
        assert_eq!(got[0].sizes, [Size::Wide], "no `sizes`: only its own size");
        assert_eq!(
            got[0].actions,
            [ActionDef {
                id: "post".into(),
                function: "post".into(),
                confirm: true,
                destructive: false
            }]
        );
    }

    #[test]
    fn no_block_is_no_widgets() {
        assert!(widgets(Some(&json!({ "functions": {} }))).is_empty());
        assert!(widgets(None).is_empty());
    }
}
