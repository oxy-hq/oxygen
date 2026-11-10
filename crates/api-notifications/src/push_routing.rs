//! The registered [`Push`]: one transport per `device_tokens.platform`.
//!
//! Web Push, APNs and FCM are configured independently, and each is handed
//! only its own platform's rows — an APNs token posted to a Web Push endpoint
//! would be a POST to a URL that is not one. A row whose platform has no
//! configured transport is left alone and counted in the log line, so "this
//! user's phone never buzzes" is answerable from the logs.

use std::sync::Arc;

use entity::device_tokens;
use tracing::info;

use super::apns::{Apns, PLATFORM_APNS};
use super::deliver::{Push, PushReport, PushSignal, set_push};
use super::fcm::{Fcm, PLATFORM_FCM};
use super::web_push::{PLATFORM_WEB, WebPush};

/// An `apns` token of the form `simulator:<UDID>`: an iOS Simulator running a
/// Debug build, which has no APNs token. Routed to [`SIMULATOR_ROUTE`], which
/// exists only in a debug server on a Mac (`simulator_push`); a release build
/// has no such route, so these rows are never sent to Apple.
pub(super) const SIMULATOR_TOKEN_PREFIX: &str = "simulator:";
/// The route key for simulator tokens.
const SIMULATOR_ROUTE: &str = "apns-simulator";

/// The route a device row takes: its platform, except a simulator token.
fn route_key(device: &device_tokens::Model) -> &str {
    if device.platform == PLATFORM_APNS && device.token.starts_with(SIMULATOR_TOKEN_PREFIX) {
        SIMULATOR_ROUTE
    } else {
        &device.platform
    }
}

/// Whether a registration may store `token` for `platform`: a simulator token
/// only for `apns`, only as `simulator:<UUID>`, and only in a debug build.
pub(super) fn simulator_token_allowed(platform: &str, token: &str) -> bool {
    if !token.starts_with(SIMULATOR_TOKEN_PREFIX) {
        return true;
    }
    let udid = &token[SIMULATOR_TOKEN_PREFIX.len()..];
    cfg!(debug_assertions) && platform == PLATFORM_APNS && uuid::Uuid::parse_str(udid).is_ok()
}

struct Route {
    /// A platform (`web`, `apns`, `fcm`), or [`SIMULATOR_ROUTE`].
    platform: &'static str,
    transport: Arc<dyn Push>,
}

#[derive(Default)]
pub struct RoutingPush {
    routes: Vec<Route>,
}

impl RoutingPush {
    /// Route `platform`'s rows to `transport`.
    pub fn with(mut self, platform: &'static str, transport: Arc<dyn Push>) -> Self {
        self.routes.push(Route {
            platform,
            transport,
        });
        self
    }

    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }

    /// The transports, for the boot log line.
    pub fn names(&self) -> Vec<&'static str> {
        self.routes.iter().map(|r| r.transport.name()).collect()
    }

    /// Every transport the environment configures. Each one that is not says
    /// why in its own startup warning.
    pub fn from_env() -> Self {
        let mut routing = Self::default();
        if let Some(w) = WebPush::from_env() {
            routing = routing.with(PLATFORM_WEB, Arc::new(w));
        }
        if let Some(a) = Apns::from_env() {
            routing = routing.with(PLATFORM_APNS, Arc::new(a));
        }
        if let Some(f) = Fcm::from_env() {
            routing = routing.with(PLATFORM_FCM, Arc::new(f));
        }
        #[cfg(debug_assertions)]
        if let Some(s) = super::simulator_push::SimulatorPush::from_env() {
            routing = routing.with(SIMULATOR_ROUTE, Arc::new(s));
        }
        routing
    }
}

#[async_trait::async_trait]
impl Push for RoutingPush {
    async fn send(&self, tokens: &[device_tokens::Model], signal: &PushSignal) -> PushReport {
        let sends = self.routes.iter().filter_map(|route| {
            let rows: Vec<device_tokens::Model> = tokens
                .iter()
                .filter(|t| route_key(t) == route.platform)
                .cloned()
                .collect();
            (!rows.is_empty()).then(|| async move { route.transport.send(&rows, signal).await })
        });
        let reports = futures::future::join_all(sends).await;

        let unrouted = tokens
            .iter()
            .filter(|t| !self.routes.iter().any(|r| r.platform == route_key(t)))
            .count();
        if unrouted > 0 {
            info!(
                user = %signal.user_id,
                devices = unrouted,
                "push skipped devices on a platform with no configured transport"
            );
        }
        reports
            .into_iter()
            .fold(PushReport::default(), |sum, r| PushReport {
                delivered: sum.delivered + r.delivered,
                failed: sum.failed + r.failed,
            })
    }

    fn name(&self) -> &'static str {
        "routing"
    }

    fn configured(&self) -> bool {
        self.routes.iter().any(|r| r.transport.configured())
    }

    fn handles(&self, device: &device_tokens::Model) -> bool {
        self.routes
            .iter()
            .any(|r| r.platform == route_key(device) && r.transport.configured())
    }
}

/// Register every configured transport, if there is at least one. Returns
/// whether anything registered, for the caller's boot log line.
pub fn register_if_configured() -> bool {
    let routing = RoutingPush::from_env();
    if routing.is_empty() {
        return false;
    }
    let names = routing.names().join(", ");
    set_push(Arc::new(routing));
    info!(transports = %names, "push registered");
    true
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::native_push::tests::{row, signal};

    /// Records the tokens it was handed and reports each as delivered, or as
    /// failed when `fail` is set.
    struct Fake {
        name: &'static str,
        fail: bool,
        seen: Mutex<Vec<String>>,
    }

    impl Fake {
        fn new(name: &'static str, fail: bool) -> Arc<Self> {
            Arc::new(Self {
                name,
                fail,
                seen: Mutex::new(Vec::new()),
            })
        }
        fn seen(&self) -> Vec<String> {
            self.seen.lock().expect("lock").clone()
        }
    }

    #[async_trait::async_trait]
    impl Push for Fake {
        async fn send(&self, tokens: &[device_tokens::Model], _: &PushSignal) -> PushReport {
            self.seen
                .lock()
                .expect("lock")
                .extend(tokens.iter().map(|t| t.token.clone()));
            match self.fail {
                true => PushReport {
                    delivered: 0,
                    failed: tokens.len(),
                },
                false => PushReport {
                    delivered: tokens.len(),
                    failed: 0,
                },
            }
        }
        fn name(&self) -> &'static str {
            self.name
        }
    }

    #[tokio::test]
    async fn each_transport_gets_only_its_platform_and_reports_are_summed() {
        let (web, apns, fcm) = (
            Fake::new("web", false),
            Fake::new("apns", false),
            Fake::new("fcm", true),
        );
        let routing = RoutingPush::default()
            .with("web", web.clone())
            .with("apns", apns.clone())
            .with("fcm", fcm.clone());
        let tokens = [
            row("web", "w1"),
            row("apns", "a1"),
            row("fcm", "f1"),
            row("apns", "a2"),
            row("web", "w2"),
        ];
        let report = routing.send(&tokens, &signal("k")).await;
        assert_eq!(web.seen(), ["w1", "w2"]);
        assert_eq!(apns.seen(), ["a1", "a2"]);
        assert_eq!(fcm.seen(), ["f1"]);
        assert_eq!(
            report,
            PushReport {
                delivered: 4,
                failed: 1
            }
        );
    }

    #[tokio::test]
    async fn an_unconfigured_platform_is_skipped_and_an_idle_transport_not_called() {
        let (web, apns) = (Fake::new("web", false), Fake::new("apns", false));
        let routing = RoutingPush::default()
            .with("web", web.clone())
            .with("apns", apns.clone());
        // No FCM transport: the fcm row goes nowhere; no apns rows: not called.
        let tokens = [row("web", "w1"), row("fcm", "f1")];
        let report = routing.send(&tokens, &signal("k")).await;
        assert_eq!(web.seen(), ["w1"]);
        assert!(apns.seen().is_empty());
        assert_eq!(
            report,
            PushReport {
                delivered: 1,
                failed: 0
            }
        );
        assert_eq!(routing.names(), ["web", "apns"]);
        assert!(routing.configured());
    }

    #[test]
    fn nothing_configured_is_empty_and_not_configured() {
        let routing = RoutingPush::default();
        assert!(routing.is_empty());
        assert!(!routing.configured());
    }

    #[test]
    fn a_platform_handles_only_where_its_own_sender_is_configured() {
        // Web Push set up, APNs/FCM not: a phone's rows reach no sender, so the
        // ledger says `not_configured` rather than `no_devices`.
        let routing = RoutingPush::default().with(PLATFORM_WEB, Fake::new("web", false));
        assert!(routing.handles(&row(PLATFORM_WEB, "https://push.example/1")));
        assert!(!routing.handles(&row(PLATFORM_APNS, "ab12")));
        assert!(!routing.handles(&row(PLATFORM_FCM, "f-1")));
    }

    #[test]
    fn a_simulator_token_takes_its_own_route_and_never_apns() {
        let sim = row(
            PLATFORM_APNS,
            "simulator:16957A09-39EB-4B88-944D-A6FFAA5270F8",
        );
        let phone = row(PLATFORM_APNS, "ab12");
        assert_eq!(route_key(&sim), SIMULATOR_ROUTE);
        assert_eq!(route_key(&phone), PLATFORM_APNS);
        // With only real APNs configured, a simulator row reaches no sender.
        let routing = RoutingPush::default().with(PLATFORM_APNS, Fake::new("apns", false));
        assert!(routing.handles(&phone));
        assert!(!routing.handles(&sim));
    }

    #[tokio::test]
    async fn real_apns_never_receives_a_simulator_row() {
        let apns = Fake::new("apns", false);
        let simulator = Fake::new("simulator", false);
        let routing = RoutingPush::default()
            .with(PLATFORM_APNS, apns.clone())
            .with(SIMULATOR_ROUTE, simulator.clone());
        let sim_token = "simulator:16957A09-39EB-4B88-944D-A6FFAA5270F8";
        let rows = [row(PLATFORM_APNS, "ab12"), row(PLATFORM_APNS, sim_token)];
        routing.send(&rows, &signal("list_overdue")).await;
        assert_eq!(apns.seen(), ["ab12"]);
        assert_eq!(simulator.seen(), [sim_token]);
    }

    #[test]
    fn a_simulator_token_registers_only_as_an_apns_uuid_in_a_debug_build() {
        let udid = "16957A09-39EB-4B88-944D-A6FFAA5270F8";
        assert!(
            simulator_token_allowed(PLATFORM_APNS, "ab12"),
            "a real token is untouched"
        );
        assert_eq!(
            simulator_token_allowed(PLATFORM_APNS, &format!("simulator:{udid}")),
            cfg!(debug_assertions)
        );
        assert!(!simulator_token_allowed(
            PLATFORM_FCM,
            &format!("simulator:{udid}")
        ));
        assert!(!simulator_token_allowed(PLATFORM_APNS, "simulator:booted"));
    }
}
