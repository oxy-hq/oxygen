use oxy_authz::Action;
use serde_json::json;

use std::collections::BTreeSet;

use super::{declared, describe, function_declares, function_specs, valid_pipeline_name};

fn names(list: &[&str]) -> BTreeSet<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn each_block_declares_its_action() {
    assert_eq!(
        function_declares(&json!({ "oxygen": { "frontline": { "devices": "manage" } } })),
        vec![Action::FrontlineDeviceManage]
    );
    assert_eq!(
        function_declares(
            &json!({ "oxygen": { "sourceUploads": { "pipelines": ["ubereats_transactions"] } } })
        ),
        vec![Action::SourceUploadReport]
    );
    assert_eq!(
        function_declares(&json!({
            "route": true,
            "oxygen": {
                "frontline": { "devices": "manage" },
                "sourceUploads": { "pipelines": ["a"] }
            }
        })),
        vec![Action::FrontlineDeviceManage, Action::SourceUploadReport]
    );
}

#[test]
fn anything_else_declares_nothing() {
    for entry in [
        json!({}),
        json!({ "oxygen": {} }),
        json!({ "oxygen": { "frontline": {} } }),
        json!({ "oxygen": { "frontline": { "devices": "read" } } }),
        json!({ "oxygen": { "frontline": { "devices": true } } }),
        json!({ "oxygen": { "frontline": "manage" } }),
        // Uploads name pipelines; the earlier `reports` shape asks for nothing.
        json!({ "oxygen": { "sourceUploads": { "reports": "write" } } }),
        json!({ "oxygen": { "sourceUploads": { "pipelines": [] } } }),
        json!({ "oxygen": { "sourceUploads": { "pipelines": "ubereats_transactions" } } }),
        json!({ "oxygen": { "sourceUploads": { "pipelines": ["", "a b", "../x"] } } }),
        json!({ "oxygen": { "source_uploads": { "pipelines": ["a"] } } }),
        // Outside the `oxygen` block a block asks for nothing.
        json!({ "frontline": { "devices": "manage" } }),
        json!({ "sourceUploads": { "pipelines": ["a"] } }),
        json!(null),
    ] {
        assert!(
            function_declares(&entry).is_empty(),
            "{entry} declared something"
        );
    }
}

#[test]
fn a_manifest_lists_each_action_with_its_functions() {
    let manifest = json!({
        "functions": {
            "tablets": { "route": true, "oxygen": { "frontline": { "devices": "manage" } } },
            "reissue": { "oxygen": { "frontline": { "devices": "manage" } } },
            "upload": { "oxygen": { "sourceUploads": { "pipelines": ["ubereats_transactions", "toast_pos"] } } },
            "upload-again": { "oxygen": { "sourceUploads": { "pipelines": ["toast_pos", "bad name"] } } },
            "plain": { "route": true }
        }
    });
    let got = declared(Some(&manifest));
    let kiosks = &got[&Action::FrontlineDeviceManage];
    assert_eq!(kiosks.functions, ["reissue", "tablets"]);
    assert!(kiosks.targets.is_empty(), "kiosks act on the whole org");
    let uploads = &got[&Action::SourceUploadReport];
    assert_eq!(uploads.functions, ["upload", "upload-again"]);
    // Every function's pipelines, once each; a name that cannot be one is dropped.
    assert_eq!(
        uploads.targets,
        names(&["toast_pos", "ubereats_transactions"])
    );
    assert_eq!(got.len(), 2);
    assert!(declared(None).is_empty());
    assert!(declared(Some(&json!({ "functions": [] }))).is_empty());
}

#[test]
fn every_app_action_has_words() {
    for action in Action::APP_ACTIONS {
        assert!(
            !describe(action).is_empty(),
            "{action:?} has no description"
        );
    }
}

#[test]
fn an_upload_names_its_pipelines_and_nothing_else() {
    let specs = function_specs(&json!({
        "oxygen": { "sourceUploads": { "pipelines": ["b", "a", "b", "a b", ""] } }
    }));
    assert_eq!(specs.pipelines(), names(&["a", "b"]));
    assert_eq!(
        specs.targets(Action::SourceUploadReport),
        names(&["a", "b"])
    );
    assert!(specs.targets(Action::FrontlineDeviceManage).is_empty());

    for good in [
        "ubereats_transactions",
        "toast-pos",
        "v2.orders",
        &"x".repeat(128),
    ] {
        assert!(valid_pipeline_name(good), "{good}");
    }
    for bad in ["", "a b", "../x", "a/b", &"x".repeat(129)] {
        assert!(!valid_pipeline_name(bad), "{bad}");
    }
}
