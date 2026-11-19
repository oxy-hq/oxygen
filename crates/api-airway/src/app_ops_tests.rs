//! Which pipeline a name picks, apart from the queries. The queries and the
//! upload itself run in `oxy-api-custom-apps`' integration tests.

use std::collections::BTreeMap;

use oxy_app_core::app_platform_seam::PlatformActionError;
use serde_json::json;
use uuid::Uuid;

use super::pick;

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

#[test]
fn one_promoted_pipeline_of_the_name_is_the_one() {
    let promoted = BTreeMap::from([(id(1), id(10)), (id(2), id(20))]);
    let got = pick(
        "ubereats_transactions",
        &promoted,
        vec![(
            id(2),
            "pipelines/ubereats.airway.yml".into(),
            json!({ "name": "ubereats_transactions" }),
        )],
    )
    .unwrap();
    assert_eq!(got.workspace_id, id(20));
    assert_eq!(got.file_path, "pipelines/ubereats.airway.yml");
}

#[test]
fn none_is_not_found_and_two_are_refused() {
    let promoted = BTreeMap::from([(id(1), id(10)), (id(2), id(20))]);
    assert!(matches!(
        pick("x", &promoted, vec![]),
        Err(PlatformActionError::NotFound(m)) if m.contains("`x`")
    ));
    // A row in a revision that is not promoted (an older one) does not count.
    assert!(matches!(
        pick(
            "x",
            &promoted,
            vec![(id(3), "p.airway.yml".into(), json!({}))]
        ),
        Err(PlatformActionError::NotFound(_))
    ));
    let both = vec![
        (id(1), "a.airway.yml".into(), json!({})),
        (id(2), "b.airway.yml".into(), json!({})),
    ];
    assert!(matches!(
        pick("x", &promoted, both),
        Err(PlatformActionError::Invalid(m)) if m.contains("more than one") && !m.contains("workspace")
    ));
}
