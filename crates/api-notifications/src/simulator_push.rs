//! Debug builds only: deliver an APNs push to an iOS Simulator with
//! `xcrun simctl push`, so push can be tested on a Mac with no Apple Developer
//! account and no APNs key.
//!
//! The Oxygen iOS app, in a Debug build running on a simulator, registers
//! `simulator:<UDID>` as its `apns` token (`mobile/ios/Oxygen/Push/`). The
//! routing transport sends those rows here (`push_routing::route_key`), and this
//! hands the exact payload [`apns::payload`] would send to Apple to `simctl` for
//! that simulator. The phone side — banner, tap, fetching the item by id — runs
//! exactly as it would for a real push.
//!
//! Release builds don't compile this module, never route a simulator token to
//! Apple, and refuse to register one (`handlers::register_device`).

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use entity::device_tokens;
use serde_json::json;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::apns::{self, PLATFORM_APNS};
use super::deliver::{Push, PushReport, PushSignal};
use super::native_push::{self, Outcome};
use super::push_routing::SIMULATOR_TOKEN_PREFIX;

const TRANSPORT: &str = "simulator";
const XCRUN: &str = "/usr/bin/xcrun";
/// `simctl push` answers in well under a second; a hung one must not hold a
/// delivery run.
const SIMCTL_TIMEOUT: Duration = Duration::from_secs(10);

pub struct SimulatorPush {
    /// The bundle id `simctl` delivers to: the same topic APNs would use.
    topic: String,
}

impl SimulatorPush {
    /// Present only where `simctl` exists: a Mac with Xcode.
    pub fn from_env() -> Option<Self> {
        if !Path::new(XCRUN).exists() {
            return None;
        }
        let topic = std::env::var("OXY_APNS_TOPIC")
            .ok()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| apns::DEFAULT_TOPIC.to_string());
        Some(Self { topic })
    }

    async fn push_one(&self, token: &str, signal: &PushSignal) -> Outcome {
        let Some(udid) = udid_of(token) else {
            return Outcome::Failed("not a simulator token".into());
        };
        let body = match serde_json::to_vec(&simulator_payload(signal, &self.topic)) {
            Ok(b) => b,
            Err(e) => return Outcome::Failed(format!("payload: {e}")),
        };
        // Arguments go straight to the process, never through a shell; the
        // UDID was parsed as a UUID above.
        let child = Command::new(XCRUN)
            .args(["simctl", "push", udid, &self.topic, "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => return Outcome::Failed(format!("spawn simctl: {e}")),
        };
        if let Some(mut stdin) = child.stdin.take() {
            if let Err(e) = stdin.write_all(&body).await {
                return Outcome::Failed(format!("write to simctl: {e}"));
            }
        }
        match tokio::time::timeout(SIMCTL_TIMEOUT, child.wait_with_output()).await {
            Ok(Ok(out)) if out.status.success() => Outcome::Delivered,
            // A shut-down or erased simulator is never `Gone`: it comes back,
            // and the row costs nothing meanwhile.
            Ok(Ok(out)) => Outcome::Failed(format!(
                "simctl push: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )),
            Ok(Err(e)) => Outcome::Failed(format!("simctl push: {e}")),
            Err(_) => Outcome::Failed("simctl push timed out".into()),
        }
    }
}

/// The UDID in a `simulator:<UDID>` token, if it is one.
pub(super) fn udid_of(token: &str) -> Option<&str> {
    let udid = token.strip_prefix(SIMULATOR_TOKEN_PREFIX)?;
    uuid::Uuid::parse_str(udid).ok().map(|_| udid)
}

/// The APNs payload, plus the key `simctl` reads the target app from.
pub(super) fn simulator_payload(signal: &PushSignal, topic: &str) -> serde_json::Value {
    let mut body = apns::payload(signal);
    body["Simulator Target Bundle"] = json!(topic);
    body
}

#[async_trait::async_trait]
impl Push for SimulatorPush {
    async fn send(&self, tokens: &[device_tokens::Model], signal: &PushSignal) -> PushReport {
        let rows: Vec<&device_tokens::Model> = native_push::rows_for(tokens, PLATFORM_APNS)
            .into_iter()
            .filter(|row| row.token.starts_with(SIMULATOR_TOKEN_PREFIX))
            .collect();
        if rows.is_empty() {
            return PushReport::default();
        }
        let (report, _gone) = native_push::fan_out(TRANSPORT, &rows, signal, |token| {
            self.push_one(token, signal)
        })
        .await;
        report
    }

    fn name(&self) -> &'static str {
        TRANSPORT
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_push::tests::signal;

    #[test]
    fn only_a_uuid_after_the_prefix_is_a_simulator() {
        let udid = "16957A09-39EB-4B88-944D-A6FFAA5270F8";
        assert_eq!(udid_of(&format!("simulator:{udid}")), Some(udid));
        assert_eq!(udid_of(udid), None, "a bare UDID is not a simulator token");
        assert_eq!(
            udid_of("simulator:booted"),
            None,
            "simctl aliases are refused"
        );
        assert_eq!(udid_of("simulator:--help"), None);
        assert_eq!(udid_of("simulator:"), None);
    }

    #[test]
    fn the_payload_is_apns_with_the_target_bundle_added() {
        let s = signal("list_overdue");
        let body = simulator_payload(&s, "com.oxygenhq.app");
        assert_eq!(body["Simulator Target Bundle"], "com.oxygenhq.app");
        let mut without = body.clone();
        without
            .as_object_mut()
            .unwrap()
            .remove("Simulator Target Bundle");
        assert_eq!(
            without,
            apns::payload(&s),
            "everything else is what Apple would get"
        );
    }
}
