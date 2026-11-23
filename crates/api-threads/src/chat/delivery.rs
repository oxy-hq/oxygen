//! Cross-replica message fan-out.
//!
//! # The problem this exists for
//!
//! Chat is served by the stateless fleet, so two people in the same channel are
//! routinely on different replicas. An in-process `broadcast` channel would
//! deliver a message to everyone on the replica that accepted the POST and to
//! nobody else. The bug is invisible on a single-instance dev box and total in
//! production.
//!
//! # Why not the world-model pattern
//!
//! The world-model bus faced the same problem and answered it differently:
//! `e9a9e9c8a` gave it a durable `world_model_events` table, with replay. That is
//! the right shape when a missed event cannot be reconstructed — a subscriber
//! that was offline has no other way to learn what happened.
//!
//! Chat's state is already durable in `chat_messages`, and the notification here
//! carries no information beyond "look again". A missed wake costs one client a
//! slightly later render, and the very next wake — or a navigation — repairs it
//! completely. Paying for a second durable log to protect a signal whose only
//! content is recoverable from the first would buy nothing.
//!
//! # The mechanism
//!
//! Postgres `LISTEN`/`NOTIFY`, the same primitive `PostgresTaskRouter` already
//! uses to wake workers across this fleet. One dedicated listener connection per
//! process fans a notification out to the in-process subscribers for that
//! channel. No Redis, no new infrastructure, and it rides a Postgres every
//! replica already holds open.
//!
//! # What is deliberately not guaranteed
//!
//! `NOTIFY` is fire-and-forget: a payload dropped while a replica is
//! reconnecting is gone, and nothing replays it. So the notification carries
//! only a **channel id**, never message content — a client that misses one
//! re-reads the channel and is whole again. Treating the notification as the
//! message would make a dropped packet a permanently missing turn in somebody's
//! conversation.
//!
//! That is also why `subscribe` hands back a receiver rather than a stream of
//! rows: the SSE handler re-queries on every wake, so a coalesced burst costs
//! one query instead of being silently truncated.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use tokio::sync::broadcast;
use tracing::{debug, warn};
use uuid::Uuid;

/// The Postgres notification channel. One for all chat, with the channel id in
/// the payload: `LISTEN` takes an identifier rather than a parameter, so a
/// channel-per-chat-channel would mean issuing `LISTEN` at runtime for every
/// channel anybody opens — unbounded, and re-issued on every reconnect.
pub const PG_CHANNEL: &str = "oxy_chat_message";

/// Per-process subscriber registry, keyed by chat channel id.
///
/// Capacity 64: a subscriber that falls this far behind has stopped reading,
/// and `broadcast` drops the oldest rather than blocking the sender. Since the
/// payload is only a wake signal, a lagged receiver loses nothing it cannot
/// recover by re-querying.
static SUBS: OnceLock<Mutex<HashMap<Uuid, broadcast::Sender<()>>>> = OnceLock::new();

fn subs() -> &'static Mutex<HashMap<Uuid, broadcast::Sender<()>>> {
    SUBS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Subscribe to wakes for one channel. The receiver yields `()` — the caller
/// re-queries.
pub fn subscribe(channel_id: Uuid) -> broadcast::Receiver<()> {
    let mut map = match subs().lock() {
        Ok(m) => m,
        Err(poisoned) => poisoned.into_inner(),
    };
    map.entry(channel_id)
        .or_insert_with(|| broadcast::channel(64).0)
        .subscribe()
}

/// Wake this process's subscribers for a channel.
///
/// The listener loop is the ONLY caller. `announce` used to wake directly as
/// well, so the posting replica would not wait for its own notification to
/// round-trip through Postgres — but that replica runs a listener too, so it
/// received the `NOTIFY` it had just sent and every client on it saw the
/// message twice.
fn wake_local(channel_id: Uuid) {
    let mut map = match subs().lock() {
        Ok(m) => m,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(tx) = map.get(&channel_id) {
        // `send` errors only when there are no receivers, which is the common
        // case for a channel nobody has open. Not a failure — but it is the
        // signal that this entry is dead, and dropping it here is what stops the
        // map growing monotonically with every channel ever streamed for the
        // life of the process. A later subscriber simply re-creates it.
        if tx.send(()).is_err() {
            map.remove(&channel_id);
        }
    }
}

/// Announce that a channel has a new message.
///
/// Best-effort by construction. A failed `pg_notify` must not fail the POST:
/// the message is already committed, and the cost of a lost notification is
/// that other replicas see it on their next poll or navigation rather than
/// instantly. Failing the write instead would turn a delivery hiccup into
/// data loss.
pub async fn announce(db: &DatabaseConnection, channel_id: Uuid) {
    // NOT `wake_local` first. This replica runs a listener too, so it receives
    // the `NOTIFY` it is about to send — waking here as well delivered every
    // message twice to any client connected to the posting replica, which reads
    // as a duplicated message rather than a duplicated wake.
    //
    // The round trip through Postgres is a local socket to a database this
    // request is already talking to; the cost is not worth a double-render.
    if let Err(e) = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT pg_notify($1, $2)",
            [PG_CHANNEL.into(), channel_id.to_string().into()],
        ))
        .await
    {
        warn!(
            error = %e,
            %channel_id,
            "chat pg_notify failed; other replicas will see this message on their next read"
        );
    }
}

/// Start the process-wide listener.
///
/// Its own connection, minted outside the SeaORM pool: `LISTEN` binds to a
/// session for its whole lifetime, so a pooled connection would either be
/// pinned forever or lose the subscription the moment it was recycled.
///
/// The factory and TLS posture come from `agentic_runtime::router`, which owns
/// both — the factory is what makes RDS IAM auth work (the token is only
/// checked at connect time, so every reconnect needs a fresh one), and the
/// verification mode is why `require` means encrypt-without-validating here
/// (our RDS and CloudNativePG CAs are not in the Mozilla bundle). Choosing
/// either independently would work on a laptop and fail in production.
pub fn start_listener(
    factory: agentic_runtime::router::ListenerConfigFactory,
    verification: agentic_runtime::router::TlsVerification,
    cancel: tokio_util::sync::CancellationToken,
) {
    // Once per process, whatever the caller does. It was started from
    // `new_agentic_state`, which has two callers in one process — the public
    // router and the internal :3001 one — so an unguarded spawn opened two
    // dedicated LISTEN connections per replica and delivered every NOTIFY
    // twice. (Its caller is the `chat_delivery_listener` background seam now,
    // which `serve`'s boot runs once; the guard stays, for any second caller.) Doubling a *wake* sounds harmless; it is
    // not, because each wake makes every SSE client re-query and emit, so the
    // reader sees the message twice.
    static STARTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if STARTED.set(()).is_err() {
        tracing::debug!("chat listener already running in this process");
        return;
    }
    tokio::spawn(async move {
        loop {
            if cancel.is_cancelled() {
                return;
            }
            match run_listener(&factory, verification, &cancel).await {
                Ok(()) => return,
                Err(e) => {
                    // Reconnect rather than give up. A listener that exits on
                    // the first blip leaves this replica silently non-realtime
                    // for the rest of the process's life — the failure mode is
                    // "chat feels broken for some people", which is very hard
                    // to attribute.
                    warn!(error = %e, "chat listener dropped; reconnecting in 5s");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    });
}

async fn run_listener(
    factory: &agentic_runtime::router::ListenerConfigFactory,
    verification: agentic_runtime::router::TlsVerification,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (client, mut connection) =
        agentic_runtime::router::connect_listener(factory, verification).await?;

    // `tokio_postgres` surfaces notifications through the connection future's
    // poll_message, so the connection has to be driven here rather than
    // spawned and forgotten the way a query-only client would be.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        use futures::stream::StreamExt;
        let mut stream = futures::stream::poll_fn(move |cx| connection.poll_message(cx));
        while let Some(msg) = stream.next().await {
            match msg {
                Ok(tokio_postgres::AsyncMessage::Notification(n)) => {
                    if tx.send(n.payload().to_string()).is_err() {
                        return;
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    warn!(error = %e, "chat listener connection error");
                    return;
                }
            }
        }
    });

    client
        .batch_execute(&format!("LISTEN {PG_CHANNEL}"))
        .await?;
    debug!("chat listener attached");

    loop {
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            payload = rx.recv() => match payload {
                Some(p) => match Uuid::parse_str(&p) {
                    Ok(id) => wake_local(id),
                    // A payload we cannot parse is a bug somewhere else, not a
                    // reason to drop the subscription for everyone.
                    Err(_) => warn!(payload = %p, "chat notification payload was not a uuid"),
                },
                None => return Err("listener connection closed".into()),
            },
        }
    }
}

/// Test seam: how many channels currently have subscribers in this process.
#[cfg(test)]
pub fn subscribed_channels() -> usize {
    subs().lock().map(|m| m.len()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_subscriber_is_woken_and_the_payload_carries_nothing() {
        let id = Uuid::new_v4();
        let mut rx = subscribe(id);
        wake_local(id);
        // The signal is empty on purpose: a client re-queries, so a dropped
        // wake costs latency rather than a permanently missing message.
        assert_eq!(rx.recv().await.unwrap(), ());
    }

    #[tokio::test]
    async fn waking_one_channel_does_not_wake_another() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let mut ra = subscribe(a);
        let mut rb = subscribe(b);
        wake_local(a);
        assert_eq!(ra.recv().await.unwrap(), ());
        assert!(rb.try_recv().is_err(), "a wake must not cross channels");
    }

    #[test]
    fn waking_a_channel_nobody_is_watching_is_not_an_error() {
        // The common case: most channels have no open stream on most replicas.
        wake_local(Uuid::new_v4());
    }
}
