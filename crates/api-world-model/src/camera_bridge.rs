//! Camera domain events onto the world-model live feed.
//!
//! `oxy-cameras` cannot depend on the feed, so it emits through a process-wide
//! sink (`service::events::set_sink`) and this module is that sink, registered
//! once at boot by [`crate::background`]. The events land as rows like every
//! other publish (`oxy_world_model::live_feed`), so the compliance ingest on a
//! serve replica reaches a viewer on any pod.

use chrono::Utc;
use oxy_world_model::live_feed::{CameraHealthEvent, ComplianceEvent, WorldModelEvent, publish};
use uuid::Uuid;

/// Bridges `oxy_cameras` domain events onto the bus. Registered once at
/// startup ([`crate::background`]) via the cameras crate's `service::events` sink —
/// that crate can't depend on this one.
pub fn publish_camera_domain_event(
    workspace_id: Uuid,
    event: oxy_cameras::service::events::CameraDomainEvent,
) {
    use oxy_cameras::service::events::CameraDomainEvent;
    let event = match event {
        CameraDomainEvent::ComplianceReport {
            camera_id,
            report_id,
            violation,
            missing_items,
            confidence,
            segment_start,
            segment_end,
        } => WorldModelEvent::Compliance(ComplianceEvent {
            kind: if violation {
                "compliance_violation"
            } else {
                "compliance_report"
            },
            camera_id: camera_id.to_string(),
            report_id: report_id.to_string(),
            violation,
            missing_items,
            confidence,
            segment_start,
            segment_end,
            ts: Utc::now(),
        }),
        CameraDomainEvent::HealthTransition {
            camera_id,
            from,
            to,
            reason,
        } => WorldModelEvent::Health(CameraHealthEvent {
            kind: "camera_health",
            camera_id: camera_id.to_string(),
            from,
            to,
            reason,
            ts: Utc::now(),
        }),
    };
    publish(workspace_id, event);
}
