//! The approval body's rules and the page's statuses, without a database.

use super::*;

fn kiosks() -> BTreeMap<Action, Asked> {
    BTreeMap::from([(
        Action::FrontlineDeviceManage,
        Asked {
            functions: vec!["tablets".to_string()],
            targets: BTreeSet::new(),
        },
    )])
}

fn on_org(action: Action) -> ApprovedAction {
    ApprovedAction {
        action,
        target: None,
    }
}

#[test]
fn no_body_approves_everything_asked_for() {
    assert_eq!(
        requested(&ApproveBody::default(), &kiosks()).unwrap(),
        vec![on_org(Action::FrontlineDeviceManage)]
    );
    let empty = ApproveBody {
        actions: Some(Vec::new()),
    };
    assert_eq!(
        requested(&empty, &kiosks()).unwrap(),
        vec![on_org(Action::FrontlineDeviceManage)]
    );
}

#[test]
fn an_upload_is_approved_on_each_pipeline_the_build_lists() {
    let mut asked = kiosks();
    asked.insert(
        Action::SourceUploadReport,
        Asked {
            functions: vec!["webhook".to_string()],
            targets: BTreeSet::from(["toast_pos".to_string(), "ubereats_transactions".to_string()]),
        },
    );
    let body = ApproveBody {
        actions: Some(vec!["source_upload_report".to_string()]),
    };
    let on = |name: &str| ApprovedAction {
        action: Action::SourceUploadReport,
        target: Some(name.to_string()),
    };
    assert_eq!(
        requested(&body, &asked).unwrap(),
        vec![on("toast_pos"), on("ubereats_transactions")],
        "every listed pipeline, and nothing on the whole org"
    );
}

#[test]
fn an_action_not_asked_for_is_refused() {
    for id in ["source_upload_report", "member_set_role", "nonsense"] {
        let body = ApproveBody {
            actions: Some(vec![id.to_string()]),
        };
        assert!(
            matches!(requested(&body, &kiosks()), Err(TokenError::NotDeclared(got)) if got == id),
            "{id} should be refused"
        );
    }
}

fn grant(token: Uuid, action: &str, target: Option<&str>) -> entity::app_platform_grants::Model {
    entity::app_platform_grants::Model {
        id: Uuid::new_v4(),
        token_id: token,
        app_id: Uuid::nil(),
        org_id: Uuid::nil(),
        action: action.into(),
        target: target.map(str::to_string),
        created_at: chrono::Utc::now().fixed_offset(),
        created_by: None,
        revoked_at: None,
        revoked_by: None,
    }
}

#[test]
fn each_pipeline_has_its_own_status_and_only_the_live_row_counts() {
    use std::collections::HashMap;

    let live = Uuid::new_v4();
    let manifest = json!({ "functions": {
        "webhook": { "oxygen": { "sourceUploads": { "pipelines": ["a", "b"] } } }
    }});
    let grants = [
        grant(live, "source_upload_report", Some("a")),
        // Under a revoked row: approves nothing now.
        grant(Uuid::new_v4(), "source_upload_report", Some("b")),
        // Approved, and no longer asked for.
        grant(live, "frontline_device_manage", None),
    ];
    let actions = dto::actions_of(Some(&manifest), &grants, Some(live), &HashMap::new());

    let kiosks = actions
        .iter()
        .find(|a| a.action == "frontline_device_manage")
        .unwrap();
    assert_eq!(kiosks.status, "approved_unused");
    assert!(kiosks.targets.is_empty());

    let uploads = actions
        .iter()
        .find(|a| a.action == "source_upload_report")
        .unwrap();
    assert_eq!(uploads.status, "pending", "`b` still waits");
    assert_eq!(uploads.functions, ["webhook"]);
    let statuses: Vec<_> = uploads
        .targets
        .iter()
        .map(|t| (t.name.as_str(), t.status))
        .collect();
    assert_eq!(statuses, [("a", "approved"), ("b", "pending")]);

    // With no live row, nothing is approved.
    let none = dto::actions_of(Some(&manifest), &grants, None, &HashMap::new());
    assert_eq!(none.len(), 1);
    assert_eq!(none[0].status, "pending");
}
