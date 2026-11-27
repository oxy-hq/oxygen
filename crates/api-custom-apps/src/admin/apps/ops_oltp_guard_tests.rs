//! The OLTP guard's message when a delete would orphan an org's store.

use super::oltp_store_blocks_message;
use oxy_oltp::schema::WriterRef;
use uuid::Uuid;

/// The guard must name a remedy the reader can actually perform.
///
/// This started as a CLI-only message, and the Rust `oxy oltp deprovision
/// --writer` it named had no `--env`: it read `OXY_DATABASE_URL` and acted on
/// the local control plane. So every operator who hit this guard against a
/// cloud environment was handed a command that could not reach the
/// environment that refused them. That command is deleted; its replacement,
/// `oxyc oltp deprovision`, is a client of the route and takes `--env`.
///
/// Asserted rather than reviewed because the failure is invisible: the
/// message still reads as helpful, and you only discover it is not by
/// running it and watching it talk to the wrong database.
#[test]
fn names_the_http_route_before_the_cli() {
    let org = Uuid::nil();
    let writer = WriterRef::app("store_ops").expect("valid writer");
    let msg = oltp_store_blocks_message("deleted", org, &writer);

    let route = msg
        .find("/oltp/deprovision-writer")
        .expect("must name the per-writer route — it is the only remote-capable remedy");
    assert!(
        msg.contains("app:store_ops"),
        "must name the DERIVED writer, not the raw slug: {msg}"
    );

    // Order matters, not just presence: the route is the remedy, the CLI
    // is its terminal spelling.
    let cli = msg
        .find("oxyc oltp deprovision")
        .expect("should name the oxyc client of the route");
    assert!(
        route < cli,
        "the route must come before the CLI mention: {msg}"
    );
    assert!(
        msg.contains("--env <env>") && msg.contains("--writer app:store_ops --yes"),
        "the CLI line must reach a remote environment and drop only this writer: {msg}"
    );
    assert!(
        !msg.contains("`oxy oltp deprovision"),
        "must not name the deleted Rust command, which cannot reach a remote environment: {msg}"
    );
    // The two doors differ: this console is `Action::PlatformApps`, the
    // route is `Action::PlatformOltp`, and an App Operator holds the first
    // and not the second. Without this clause the reader most likely to hit
    // the guard is sent to a route that 403s them.
    assert!(
        msg.contains("platform-OLTP standing"),
        "must name the standing the route requires: {msg}"
    );

    // It must never name the whole-tenant escape hatch: that destroys every
    // other app's schema in the org, which is the opposite of this guard.
    assert!(
        !msg.contains("DELETE /api/admin/orgs"),
        "must not point at the whole-tenant deprovision: {msg}"
    );
}
