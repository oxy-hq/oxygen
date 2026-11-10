//! Notification categories against a real Oxy database: a catalogue read from
//! an app's live build, mutes at a category or a whole source that delivery
//! honours (and a critical kind that ignores them), the inbox's filters and
//! `before` paging, a scoped read-all, the catalogue endpoint's tree and what
//! it costs, who may tell whom (per app and batched, one rule), what tapping an
//! action resolves to, a function action run for real through the V8
//! isolate on the tapper's own session, and the settings document — its
//! patch, the old routes agreeing with it, the backfill of the old mute
//! tables, and the reasons delivery records (quiet hours' summary included).
//!
//! Run with:
//! `cargo nextest run -p oxy-api-notifications --test integration -E 'test(notification_categories)'`

mod actions;
mod audience;
mod catalog;
mod delivery;
mod fixture;
mod inbox;
mod settings;
mod settings_delivery;
