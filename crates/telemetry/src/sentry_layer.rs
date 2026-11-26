//! The Sentry `tracing` layer [`sentry_tracing_layer`] returns: sentry-tracing's
//! per-surface layers (`BreadcrumbLayer`, `ErrorLayer`, `LogLayer`) behind one
//! [`sentry_disposition`] call. Beside `sentry_filter.rs` rather than inside it:
//! that file is over its size budget. The rule itself, and the reason it is
//! asked once rather than split into per-layer filters, live there; this module
//! is only the fan-out.

use sentry::integrations::tracing::{BreadcrumbLayer, ErrorLayer, LogLayer};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

#[cfg(doc)]
use crate::sentry_filter::sentry_tracing_layer;
use crate::sentry_filter::{Disposition, sentry_disposition};

/// Which Sentry surfaces one `tracing` event reaches. The translation of a
/// [`Disposition`] into the per-surface layers; see [`Disposition::surfaces`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Surfaces {
    pub breadcrumb: bool,
    pub event: bool,
    pub log: bool,
}

impl Disposition {
    /// The surfaces this disposition reaches. A pure translation, exhaustive by
    /// `match`, so a new variant cannot compile without saying where it goes.
    pub const fn surfaces(self) -> Surfaces {
        match self {
            Disposition::Event => Surfaces {
                breadcrumb: false,
                event: true,
                log: true,
            },
            Disposition::Breadcrumb => Surfaces {
                breadcrumb: true,
                event: false,
                log: true,
            },
            Disposition::LogOnly => Surfaces {
                breadcrumb: false,
                event: false,
                log: true,
            },
            Disposition::Drop => Surfaces {
                breadcrumb: false,
                event: false,
                log: false,
            },
        }
    }
}

/// The Sentry `tracing` layer [`sentry_tracing_layer`] builds: sentry-tracing's
/// per-surface event layers behind one [`sentry_disposition`] call. Fields are
/// private so the only way to reach a sub-layer is through that call.
pub struct SentryTracingLayer {
    breadcrumb: BreadcrumbLayer,
    error: ErrorLayer,
    log: LogLayer,
}

impl SentryTracingLayer {
    /// Crate-private: [`sentry_tracing_layer`] is the one constructor.
    pub(crate) fn new() -> Self {
        Self {
            breadcrumb: BreadcrumbLayer::new(),
            error: ErrorLayer::new(),
            log: LogLayer::new(),
        }
    }
}

impl<S> Layer<S> for SentryTracingLayer
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &tracing::Event<'_>, ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let surfaces = sentry_disposition(metadata.target(), metadata.level()).surfaces();
        // The legacy layer's order: a breadcrumb lands on the scope before an
        // event on the same line could be captured with it.
        if surfaces.breadcrumb {
            self.breadcrumb.on_event(event, ctx.clone());
        }
        if surfaces.event {
            self.error.on_event(event, ctx.clone());
        }
        if surfaces.log {
            self.log.on_event(event, ctx);
        }
    }
}
