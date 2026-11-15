//! How a sample reads in `GET /previews/runs/{run_id}`: what was asked (the
//! run's options) and, once it ran, what its outcome recorded.

use serde_json::{Value, json};

/// The run detail's `sample`: `pipeline`, `dataset`, `window`, `resources`,
/// `wall_clock_capped`, and after the run `preview_pipeline`, `tables`,
/// `compared_with_live`, `verdict`, `findings`, `partial`, `partial_reason`.
pub fn view(options: &Value, metadata: Option<&Value>) -> Value {
    let mut view = json!({
        "pipeline": options.get("pipeline_name"),
        "dataset": options.get("dataset_name"),
        "window": options.get("window"),
        "resources": options.get("resources").cloned().unwrap_or_else(|| json!([])),
        "wall_clock_capped": options.get("wall_clock_capped").cloned().unwrap_or(json!(false)),
    });
    let recorded = metadata
        .and_then(|m| m.get("sample"))
        .and_then(Value::as_object);
    if let (Some(view), Some(recorded)) = (view.as_object_mut(), recorded) {
        for (key, value) in recorded {
            if key != "pipeline" {
                view.insert(key.clone(), value.clone());
            }
        }
    }
    view
}
