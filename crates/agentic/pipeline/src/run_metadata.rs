//! Host-supplied keys on a new run's `agentic_runs.metadata`.
//!
//! The pipeline writes its own keys there (`agent_id`, `thinking_mode`, the
//! preview stamp, the builder's onboarding fields) and reads them back on
//! resume and recovery. A host sometimes needs one more that the pipeline has
//! no opinion about — who asked, for instance, so a recovery driver can run
//! the root as that caller instead of as itself. It is merged **at the
//! insert**, not written afterwards: a run that exists without it is a run
//! recovery would drive without it.

use serde_json::{Map, Value};

/// Add `extra` to a new run's `metadata` — never over a key already there.
///
/// The pipeline's own keys win, whatever the host passed: a host key named
/// `agent_id` would otherwise decide which agent a cold resume rebuilds, and
/// one named after the preview stamp would decide whether recovery drives the
/// run at all. A no-op on metadata that is not an object.
pub(crate) fn merge(metadata: &mut Value, extra: &Map<String, Value>) {
    let Some(map) = metadata.as_object_mut() else {
        return;
    };
    for (key, value) in extra {
        map.entry(key.clone()).or_insert_with(|| value.clone());
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::merge;

    fn extra(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        value.as_object().expect("an object").clone()
    }

    #[test]
    fn a_host_key_is_added_beside_the_pipelines_own() {
        let mut metadata = json!({ "agent_id": "sales", "thinking_mode": null });
        merge(
            &mut metadata,
            &extra(json!({ "caller": { "user_id": "u-1" } })),
        );
        assert_eq!(
            metadata,
            json!({
                "agent_id": "sales",
                "thinking_mode": null,
                "caller": { "user_id": "u-1" },
            })
        );
    }

    /// A host key can never replace one the pipeline wrote — including one the
    /// pipeline wrote as `null`, which is still its key.
    #[test]
    fn a_host_key_never_replaces_a_pipeline_key() {
        let mut metadata = json!({
            "agent_id": "sales",
            "thinking_mode": null,
            "workspace_preview": { "revision_id": "r-1" },
        });
        let before = metadata.clone();
        merge(
            &mut metadata,
            &extra(json!({
                "agent_id": "other",
                "thinking_mode": "extended",
                "workspace_preview": null,
            })),
        );
        assert_eq!(metadata, before);
    }

    #[test]
    fn nothing_to_add_and_non_object_metadata_are_left_alone() {
        let mut metadata = json!({ "agent_id": "sales" });
        merge(&mut metadata, &serde_json::Map::new());
        assert_eq!(metadata, json!({ "agent_id": "sales" }));

        let mut not_an_object = json!("text");
        merge(&mut not_an_object, &extra(json!({ "caller": 1 })));
        assert_eq!(not_an_object, json!("text"));
    }
}
