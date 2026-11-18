//! The staff endpoints over an org's app events (design §6.8), the routes
//! `oxyc events` calls (`sdk/cli/src/commands/events.ts`, whose requests and
//! camelCase field names are the contract, pinned by the `app_events_api`
//! integration tests). All under `/api/customer-apps/`, on the public router,
//! authenticating inline as the slug-addressed reads beside them do:
//!
//! | Route | What |
//! | --- | --- |
//! | `GET {org}/events/types` | each type, its publisher, schema revision and last publish; per listener and environment its health, read live from the deliveries, and where its subscription came from |
//! | `GET {org}/events/{id}` | the event, its body read from the org's store, its deliveries with every attempt and invocation, its causation both ways |
//! | `GET {org}/events?root=` | every event of one root, in publish order |
//! | `GET {org}/events/find?type=&key=&environment=` | the event route's body, its id derived from the key |
//! | `GET {org}/{app}/events/feed?since=&environment=` | the app's publishes and its listeners' deliveries after `since` |
//! | `GET {org}/{app}/events/dead?environment=` | the app's dead deliveries, grouped by outcome and the error's first line |
//! | `POST {org}/events/{id}/redeliver`, `POST {org}/events/redeliver` | queue a `dead`, `expired` or `held` delivery again: the same delivery, a fresh attempt budget |
//! | `POST {org}/events/replay` | deliveries to a listener for retained events whose body is still kept |
//! | `POST {org}/events/discard`, `POST {org}/events/purge` | pending and held deliveries discarded; events and their bodies deleted. Audited |
//! | `POST {org}/{app}/events/resync` | a function's table cursors moved to now, behind-horizon, broken and paused cleared, optionally one backfill run of the handler. Audited |
//! | `PUT {org}/{app}/environments/{env}/events` | a non-production environment's events switch. Audited |
//!
//! **Who.** Reads take the role `GET …/invocations` takes, oxy-authz
//! `Action::AppAdmin`: of the app for a per-app read (the invocations gate
//! itself, and non-production reach for another environment), and of the org
//! for a read across it — Oxy staff with `manage_apps` over the org, or its
//! officers; an admin of one app does not read the whole org's events.
//! Everything that changes something takes `Action::AppEventsManage`, staff
//! holding `manage_apps` over the org, and answers `409 events_not_enabled`
//! while the org's `app_events` flag is off; the reads answer whatever it says.
//! A sandbox agent token is refused everywhere.
//!
//! **Bodies stay in the org's store.** Only the single-event reply reads one,
//! and a store that cannot be reached or a body that is gone is said
//! (`dataUnavailable`), never a failed route. No list reads bodies. Replay
//! reads each candidate's to know it is still kept; purge deletes them, and
//! deletes nothing when the store cannot be reached.
//!
//! **Bounded.** Each call queues, replays or purges at most a batch, and says
//! how many remain; running it again takes the next.

mod dead;
mod destructive;
mod door;
mod feed;
pub mod handlers;
mod queue;
mod redeliver;
mod replay;
mod reply;
pub mod requests;
mod show;
mod switch;
mod tables;
mod types;

#[cfg(test)]
mod tests;

pub use reply::ApiError;
pub use tables::ResyncBody;
