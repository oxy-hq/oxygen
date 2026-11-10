//! `oxy.anomaly` as the inbox reads it: the words and the link are built from
//! the event's worst bucket when the inbox is read.
//!
//! The half of `oxy-app`'s `anomaly_notify::people` test that reads the inbox
//! (`inbox_view`), which is this surface's. Who the anomaly delivery tells, and
//! that it tells them once, stays there with the delivery.

use chrono::{Duration, Utc};
use entity::metric_anomalies;
use entity::workspaces::WorkspaceStatus;
use oxy_api_notifications::inbox_view;
use oxy_app_core::notifications::platform::kinds::ANOMALY;
use oxy_app_core::notifications::platform::{PlatformNotice, notify};
use sea_orm::ActiveValue::Set;
use sea_orm::EntityTrait;
use uuid::Uuid;

use super::{seed_org, seed_workspace, told};
use crate::common::{Schema, fresh_db};

#[tokio::test]
async fn platform_notices_anomaly_reads_as_the_measure_and_opens_the_anomalies_view() {
    let (db, _url) = fresh_db(Schema::All).await;
    let org = seed_org(&db).await;
    let workspace = seed_workspace(&db, org.id, "Sales", WorkspaceStatus::Ready, None).await;
    let event = Uuid::new_v4();
    let now = Utc::now();
    let period_end = now - Duration::days(1);
    metric_anomalies::Entity::insert(metric_anomalies::ActiveModel {
        id: Set(Uuid::new_v4()),
        workspace_id: Set(workspace),
        measure: Set("sales.net".into()),
        time_dimension: Set("sales.day".into()),
        granularity: Set("day".into()),
        period_start: Set((period_end - Duration::days(1)).into()),
        period_end: Set(period_end.into()),
        observed: Set(1204.0),
        expected: Set(1571.0),
        lower_bound: Set(1400.0),
        upper_bound: Set(1700.0),
        z_score: Set(-4.0),
        severity: Set("high".into()),
        status: Set("new".into()),
        label: Set(Some("Net sales".into())),
        dimension_key: Set("sales.store=1".into()),
        event_id: Set(Some(event)),
        detected_at: Set((now - Duration::hours(1)).into()),
        updated_at: Set(now.into()),
        ..Default::default()
    })
    .exec(&db)
    .await
    .expect("seed anomaly");

    let notice = PlatformNotice::Anomaly {
        org_id: org.id,
        workspace_id: workspace,
        event_id: event,
    };
    notify(&db, &notice, &[org.admin]).await.expect("notify");

    let rows = told(&db, ANOMALY, &event.to_string()).await;
    assert_eq!(rows.len(), 1);
    let items = inbox_view::items(&db, rows).await.expect("inbox");
    assert_eq!(items[0].title.as_deref(), Some("Net sales looks unusual"));
    assert!(
        items[0].href.as_deref().unwrap().ends_with(&format!(
            "/workspaces/{workspace}/ide/semantic?view=anomalies"
        )),
        "{:?}",
        items[0].href
    );
}
