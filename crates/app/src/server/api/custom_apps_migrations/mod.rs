//! Schema migrations for a custom app: declared in the bundle, applied once,
//! recorded, and refused if edited.
//!
//! # The gap this closes
//!
//! `oxy publish` shipped code. Schema did not ship at all: an app's tables
//! arrived by a developer running a hand-maintained `.integrate.sh` that
//! `psql`'d **every** `schemas/*.sql` on **every** pass, with nothing recording
//! what had run. Measured on dev in `customer-apps/dev/delightree-demo`:
//!
//!  - Renaming a launcher-plan row *from the app* made a seed's
//!    `ON CONFLICT (template_id, phase, title)` stop matching, so the next pass
//!    **re-inserted the old row beside the new one** — 17 rows became 18.
//!  - A training-body upsert **restored its own text over an author's edit**, so
//!    the app's writes expired on the next pass.
//!
//! Both were patched with triggers inside the app's own schema. Those triggers
//! were a workaround for this module not existing.
//!
//! # The guarantee
//!
//! A migration runs **exactly once per app, ever**, recorded in
//! `custom_app_migrations`. Re-running is a no-op *by construction* — the
//! ledger is consulted, not the SQL's own defensiveness — so an author who
//! forgets `IF NOT EXISTS` gets the same answer as one who remembers.
//!
//! The load-bearing rule is [`plan`]: a file already in the ledger whose bytes
//! have **changed** is a hard error, not a silent re-run and not a silent skip.
//! That is what makes "edit a migration that already ran" impossible rather than
//! discouraged — an edit is invisible to a ledger keyed on filename alone, and
//! the divergence it creates (tenant A ran v1, tenant B runs v2) is exactly the
//! failure that is unrecoverable once noticed.
//!
//! # Boundaries
//!
//! - The schema is `app_<writer>` where the writer is **derived from the app's
//!   slug host-side** (`oxy_oltp::schema::app_writer_name`), never taken from
//!   the manifest — the same binding `ctx.oltp` uses. A manifest that could name
//!   its own schema is a manifest that can migrate another app's.
//! - SQL runs as the app's **own writer role**, not the tenant owner. The writer
//!   holds `CREATE` inside its one schema and nothing outside it, so containment
//!   is enforced by Postgres rather than by reviewing the file. (This differs
//!   from `oxy_oltp::migrator`, which applies a *workspace's* DDL as the owner
//!   because that DDL legitimately spans schemas.)
//! - Each file runs in its own transaction. A failure part-way leaves earlier
//!   files applied **and recorded**, which is what the ledger is for: fix the
//!   file, re-publish, and only the failed one is retried.
//!
//! # Known limitations
//!
//! The `.sql` files ride **inside the bundle**, so — like every other file in a
//! bundle — they are reachable over the app's own host
//! (`/customer-apps/<org>/<slug>/<dir>/0001.sql`). Nothing here redacts them.
//! A migration must therefore carry no credential and no data the app's own
//! viewers may not see; it is DDL and seed rows, not a secret.
//!
//! Only the **publish** path applies migrations, because that is where the
//! bundle is in hand. The admin console's *Make Live* and *roll back* repoint
//! the channel at a build already in the object store and run nothing. Rolling
//! back is unaffected (those files are in the ledger already); the live hole is
//! `--no-promote` followed by a console Make Live, which puts new code in front
//! of tables that were never created. Closing it needs the build store to list
//! and fetch a build's `<dir>/*.sql` — `custom_apps_build_store` has `get_object`
//! but no list — and then this module's [`apply_on_promote`] unchanged.
//!
//! `CREATE INDEX CONCURRENTLY` cannot run inside a transaction and is **not**
//! special-cased here (`oxy_oltp::migrator` is, via `mentions_concurrently`).
//! Such a file fails loudly with Postgres's own message rather than silently
//! losing its transaction — the safe direction. Lift it here if an app ever
//! needs one.
//!
//! # Layout
//!
//! * [`types`] — [`MigrationError`] (and which of its arms are the author's
//!   fault vs. retryable infra), [`DeclaredMigration`], [`Applied`].
//! * [`plan`] — bundle bytes → a plan. Pure: `collect` the `<dir>/*.sql` files,
//!   `plan` them against a ledger (checksum and rename rules), `declare` from
//!   the manifest. All of the unit tests live here, because all of the
//!   decisions do.
//! * [`apply`] — the plan against a real writer: advisory lock, re-plan, run,
//!   ledger row per file, one transaction per migration.
//! * [`airhouse`] — the same ledger for `airhouseMigrations`, against the app's
//!   schema in its workspace's Airhouse, where every file is also checked
//!   against the schema and DuckLake's rules (no keys, `UNIQUE`, indexes).

mod airhouse;
mod apply;
mod plan;
mod types;

pub(super) use airhouse::apply_airhouse_on_promote;
pub(super) use apply::apply_on_promote;
pub(super) use plan::{declare, declare_airhouse};
pub use types::MigrationError;
