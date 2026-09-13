//! Process-global singleton for the active [`ObservabilityStore`].
//!
//! Certain subsystems (metric context finalization, intent classification,
//! telemetry) need access to the store without explicit plumbing. This module
//! holds a `OnceLock<Arc<dyn ObservabilityStore>>` that the application
//! entrypoint sets during startup.

use std::sync::{Arc, OnceLock};

use oxy_shared::errors::OxyError;

use crate::store::ObservabilityStore;

static GLOBAL_STORE: OnceLock<Arc<dyn ObservabilityStore>> = OnceLock::new();

/// Register a global ObservabilityStore instance. Subsequent calls are no-ops
/// (first one wins).
pub fn set_global(store: Arc<dyn ObservabilityStore>) {
    let _ = GLOBAL_STORE.set(store);
}

/// The global store, or an error naming what actually enables it.
///
/// **`--enterprise` does not enable observability, and three call sites used to
/// say it did.** The gate is `OXY_OBSERVABILITY_BACKEND`: unset means capture is
/// off in every mode, and a removed label (`duckdb`, `postgres`, `airhouse`)
/// fails validation and leaves the global unset too. Both produce this error,
/// and pointing at `--enterprise` sent operators to a flag that changes nothing
/// here — including on `oxy start --enterprise`, where the flag is accepted
/// because `StartArgs` flattens `ServeArgs`.
///
/// The real cause is printed at startup by `observability_boot::finalize` in
/// oxy-app (a label error from `backend_enabled`, a connection error from each
/// failed open); this is the message that repeats, so it is the one that has to
/// be right.
pub fn require_global() -> Result<&'static Arc<dyn ObservabilityStore>, OxyError> {
    get_global().ok_or_else(|| {
        OxyError::RuntimeError(
            "Observability storage not initialized. Set OXY_OBSERVABILITY_BACKEND=clickhouse \
             (with OXY_CLICKHOUSE_URL / _USER / _PASSWORD / _DATABASE — `oxy start` provisions \
             the container and sets these for you). If it is already set, check startup logs \
             for a backend error: a removed label such as 'duckdb' fails validation and leaves \
             observability disabled."
                .into(),
        )
    })
}

/// Retrieve the global ObservabilityStore, if one has been registered.
///
/// Prefer [`require_global`] on a path that needs the store — it returns the
/// actionable error instead of a bare `None`.
pub fn get_global() -> Option<&'static Arc<dyn ObservabilityStore>> {
    GLOBAL_STORE.get()
}
