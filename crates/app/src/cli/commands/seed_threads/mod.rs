//! `oxy seed` — demo conversation threads for the seeded workspace.
//!
//! Gives a freshly-seeded box a Chat surface with history in it: the threads
//! list, the recents rail and the analytics thread view all render real
//! transcripts instead of an empty state, so a developer can open the feature
//! they're working on without first spending an LLM call to manufacture one.
//!
//! **Threads are user-scoped**, and that shapes everything here. `get_threads`
//! filters on `user_id = <caller>` (and `get_thread` does the same unless the
//! caller holds an operator session), so a thread owned by a fabricated user
//! would be invisible to whoever actually logs in — seeded and useless. Each
//! fixture is therefore materialized **once per member of the Local org**,
//! which is exactly the set `bind_org_admin_emails` creates from
//! `OXY_GLOBAL_ADMINS`. Whoever signs in sees their own copy.
//!
//! A thread is four tables, and the fixtures carry all four:
//!
//! ```text
//! threads ──┬── agentic_runs ──┬── agentic_run_events   (the transcript)
//!           │                  └── analytics_run_extensions
//!           └─ (user_id, project_id)
//! ```
//!
//! Events are stored **raw**, exactly as the pipeline wrote them (`state_enter`,
//! `llm_token`, `state_exit`, …), not as the `step_start` / `text_delta` shapes
//! the API returns. The read path runs them through the domain's stateful
//! `RowProcessor`, which is what synthesizes `step_end` metadata and squashes
//! token deltas — so a fixture recorded from the API response would double-
//! process and render wrong. `wow-sales-growth.json` is a verbatim dump of a
//! real run's rows for that reason; see the fixture table below.
//!
//! Idempotent — a thread whose deterministic id is already present is left
//! untouched, so a re-run never clobbers a conversation someone continued.

use agentic_runtime::entity::{run, run_event};
use airhouse::LOCAL_ORG_ID;
use chrono::{Duration, Utc};
use entity::prelude::{OrgMembers, Threads, Users};
use entity::{org_members, threads, users};
use oxy::theme::StyledText;
use oxy_shared::errors::OxyError;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, DatabaseTransaction,
    EntityTrait, QueryFilter, QuerySelect, TransactionTrait,
};
use serde::Deserialize;
use uuid::Uuid;

/// The fixtures, newest first, with how long before "now" each thread is dated.
///
/// Staggered ages (rather than one timestamp) so the threads list, the recents
/// rail and the "grouped by day" headers all have something to sort and bucket
/// — a seed where every row shares a created_at exercises none of that.
///
/// | Fixture | Origin | Exercises |
/// | ------- | ------ | --------- |
/// | `wow-sales-growth` | **Captured verbatim** from a real run | Full trace: 4 tool calls, a 27KB tool result, semantic shortcut, 105-row query, line chart |
/// | `orders-by-channel` | Authored | Short trace ending in a bar chart |
/// | `repeat-customer-rate` | Authored | Text-only answer, no chart — single-row scalar result |
///
/// The authored two use the same event vocabulary and payload shapes as the
/// captured one; they exist so the list isn't a single row and so the
/// no-chart and small-result render paths are covered.
const FIXTURES: &[Fixture] = &[
    Fixture {
        json: include_str!("fixtures/wow-sales-growth.json"),
        age_minutes: 95,
    },
    Fixture {
        json: include_str!("fixtures/orders-by-channel.json"),
        age_minutes: 26 * 60,
    },
    Fixture {
        json: include_str!("fixtures/repeat-customer-rate.json"),
        age_minutes: 4 * 24 * 60,
    },
];

struct Fixture {
    json: &'static str,
    age_minutes: i64,
}

/// How long a seeded run "took". Sets the run's `updated_at` and the spacing
/// of its events — one constant because two would drift, and a larger event
/// step than run duration stamps the last event after the run finished.
const RUN_DURATION_SECS: i64 = 81;

/// A seeded thread and the single run behind it, as stored on disk.
#[derive(Deserialize)]
struct ThreadFixture {
    /// Stable key for the deterministic ids — renaming it re-seeds the thread.
    slug: String,
    title: String,
    input: String,
    #[serde(default)]
    output: String,
    source: String,
    source_type: String,
    #[serde(default = "empty_references")]
    references: String,
    run: RunFixture,
    events: Vec<EventFixture>,
}

#[derive(Deserialize)]
struct RunFixture {
    question: String,
    answer: Option<String>,
    agent_id: String,
    thinking_mode: Option<String>,
    task_status: String,
}

/// One row of `agentic_run_events`, raw as the pipeline wrote it.
#[derive(Deserialize)]
struct EventFixture {
    seq: i64,
    event_type: String,
    payload: serde_json::Value,
    #[serde(default)]
    attempt: i32,
}

fn empty_references() -> String {
    "[]".to_string()
}

/// Deterministic UUID v5 for a seeded row — stable across machines and re-runs.
/// Namespaced by `kind` so a thread and its run can share a key without colliding.
fn seed_id(kind: &str, key: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("oxy.thread-seed.{kind}.{key}").as_bytes(),
    )
}

/// Seed every fixture for every member of the Local org, in `workspace_id`.
///
/// Skips (does not error) on a non-local database: this fabricates rows that
/// look like real user conversations, and the caller folds it into the demo
/// seed, so a remote `OXY_DATABASE_URL` must cost nothing rather than fail.
pub async fn seed_demo_threads(
    conn: &DatabaseConnection,
    workspace_id: Uuid,
) -> Result<(), OxyError> {
    if !super::seed_partners::is_local_db() {
        println!(
            "{} skipping thread seed — OXY_DATABASE_URL is not local \
             (set OXY_SEED_ALLOW_REMOTE=1 to force)",
            "⚠️".info()
        );
        return Ok(());
    }

    let owners = local_org_members(conn).await?;
    if owners.is_empty() {
        // No members means nobody has been bound from OXY_GLOBAL_ADMINS yet.
        // Seeding for a user who cannot log in would produce rows no request
        // can ever read, so say why instead of reporting a hollow success.
        println!(
            "{} no Local org members — skipping thread seed. \
             Set OXY_GLOBAL_ADMINS and re-run so threads land on an account you can sign in as.",
            "⚠️".warning()
        );
        return Ok(());
    }

    let fixtures = parse_fixtures()?;
    let mut created = 0u32;
    let mut skipped = 0u32;
    for user_id in &owners {
        for (fixture, spec) in &fixtures {
            if ensure_thread(conn, fixture, spec.age_minutes, *user_id, workspace_id).await? {
                created += 1;
            } else {
                skipped += 1;
            }
        }
    }

    println!(
        "{} seeded {created} thread{} across {} member{} ({skipped} already present)",
        "💬".info(),
        if created == 1 { "" } else { "s" },
        owners.len(),
        if owners.len() == 1 { "" } else { "s" }
    );
    Ok(())
}

/// Parse every fixture up front, so a malformed one fails before any writes
/// rather than half way through the second user's copy.
fn parse_fixtures() -> Result<Vec<(ThreadFixture, &'static Fixture)>, OxyError> {
    FIXTURES
        .iter()
        .map(|spec| {
            serde_json::from_str::<ThreadFixture>(spec.json)
                .map(|f| (f, spec))
                .map_err(|e| OxyError::ConfigurationError(format!("invalid thread fixture: {e}")))
        })
        .collect()
}

/// Every user with a membership in the Local org — the accounts a developer
/// can actually sign in as on a seeded box.
async fn local_org_members(conn: &DatabaseConnection) -> Result<Vec<Uuid>, OxyError> {
    OrgMembers::find()
        .filter(org_members::Column::OrgId.eq(LOCAL_ORG_ID))
        .select_only()
        .column(org_members::Column::UserId)
        .into_tuple::<Uuid>()
        .all(conn)
        .await
        .map_err(|e| OxyError::DBError(format!("query Local org members: {e}")))
}

/// Insert one thread + run + events, then its analytics extension. Returns
/// `false` when the thread was already there (nothing written).
///
/// **The first three writes are one transaction, and that is load-bearing.**
/// The skip check below keys on the `threads` row alone, and the caller
/// downgrades a failure to a warning — so a partial insert would leave a
/// thread with no run behind, and every later `oxy seed` would find that row,
/// return `false`, and skip it forever. The thread renders empty and nothing
/// short of a manual `DELETE` repairs it. All-or-nothing makes the skip check
/// honest: no thread row means the next run genuinely retries.
async fn ensure_thread(
    conn: &DatabaseConnection,
    fixture: &ThreadFixture,
    age_minutes: i64,
    user_id: Uuid,
    workspace_id: Uuid,
) -> Result<bool, OxyError> {
    let key = format!("{}.{user_id}", fixture.slug);
    let thread_id = seed_id("thread", &key);

    if Threads::find_by_id(thread_id)
        .one(conn)
        .await
        .map_err(|e| OxyError::DBError(format!("query seeded thread {thread_id}: {e}")))?
        .is_some()
    {
        return Ok(false);
    }

    let created_at = Utc::now().fixed_offset() - Duration::minutes(age_minutes);
    let run_id = seed_id("run", &key).to_string();

    let txn = conn
        .begin()
        .await
        .map_err(|e| OxyError::DBError(format!("begin seed transaction: {e}")))?;

    threads::ActiveModel {
        id: ActiveValue::Set(thread_id),
        title: ActiveValue::Set(fixture.title.clone()),
        input: ActiveValue::Set(fixture.input.clone()),
        output: ActiveValue::Set(fixture.output.clone()),
        source: ActiveValue::Set(fixture.source.clone()),
        source_type: ActiveValue::Set(fixture.source_type.clone()),
        references: ActiveValue::Set(fixture.references.clone()),
        created_at: ActiveValue::Set(created_at),
        user_id: ActiveValue::Set(Some(user_id)),
        project_id: ActiveValue::Set(workspace_id),
        is_processing: ActiveValue::Set(false),
        sandbox_info: ActiveValue::Set(None),
    }
    .insert(&txn)
    .await
    .map_err(|e| OxyError::DBError(format!("insert seeded thread {thread_id}: {e}")))?;

    // The run row's own `metadata` carries agent_id/thinking_mode as well as the
    // extension table: the API reads the extension first but falls back to
    // `metadata.agent_id`, and only the fallback survives if the analytics
    // migrator hasn't run on this database yet.
    let metadata = serde_json::json!({
        "agent_id": fixture.run.agent_id,
        "thinking_mode": fixture.run.thinking_mode,
    });
    // Dated as if the run finished shortly after the thread opened, so the
    // thread's "last activity" ordering matches its position in the list.
    let finished_at = created_at + Duration::seconds(RUN_DURATION_SECS);

    run::ActiveModel {
        id: ActiveValue::Set(run_id.clone()),
        question: ActiveValue::Set(fixture.run.question.clone()),
        answer: ActiveValue::Set(fixture.run.answer.clone()),
        error_message: ActiveValue::Set(None),
        thread_id: ActiveValue::Set(Some(thread_id)),
        source_type: ActiveValue::Set(Some(fixture.source_type.clone())),
        metadata: ActiveValue::Set(Some(metadata)),
        parent_run_id: ActiveValue::Set(None),
        schedule_id: ActiveValue::Set(None),
        task_status: ActiveValue::Set(Some(fixture.run.task_status.clone())),
        task_metadata: ActiveValue::Set(None),
        attempt: ActiveValue::Set(0),
        recovery_requested_at: ActiveValue::Set(None),
        driver_id: ActiveValue::Set(None),
        driver_heartbeat_at: ActiveValue::Set(None),
        cancel_requested_at: ActiveValue::Set(None),
        workspace_id: ActiveValue::Set(workspace_id),
        created_at: ActiveValue::Set(created_at),
        updated_at: ActiveValue::Set(finished_at),
    }
    .insert(&txn)
    .await
    .map_err(|e| OxyError::DBError(format!("insert seeded run {run_id}: {e}")))?;

    insert_events(&txn, &run_id, fixture, created_at).await?;

    txn.commit()
        .await
        .map_err(|e| OxyError::DBError(format!("commit seeded thread {thread_id}: {e}")))?;

    // Outside the transaction, and deliberately non-fatal. `insert_run_meta`
    // takes a `&DatabaseConnection` (as does every function in that crate's
    // crud module), so it cannot join the transaction above without widening
    // another crate's API for a seed-only need.
    //
    // Losing it is survivable by design: the API reads `agent_id` from the
    // extension but falls back to the run's own `metadata`, which the
    // transaction already committed, so the thread still renders — only
    // `thinking_mode` comes back null. Hard-erroring here would be the worse
    // trade: this is exactly the call that fails when the analytics migrator
    // hasn't run (it is reachable only from `serve`/`admin`/`airway`/
    // `agentic_cli`, never from the seed path), and it would abort the whole
    // seed over a cosmetic field.
    if let Err(e) = agentic_analytics::insert_run_meta(
        conn,
        &run_id,
        &fixture.run.agent_id,
        fixture.run.thinking_mode.clone(),
    )
    .await
    {
        tracing::warn!(
            run_id = %run_id,
            error = %e,
            "seeded thread has no analytics extension row; agent_id still resolves \
             from run metadata, thinking_mode will read as null"
        );
    }

    Ok(true)
}

/// Write the transcript. Events are spread evenly between the run's start and
/// finish — the stored `created_at` is only ever read as an ordering tiebreak
/// (`seq` is the real order), but a run whose events all share one instant
/// reads as a bug when someone inspects the table.
async fn insert_events(
    txn: &DatabaseTransaction,
    run_id: &str,
    fixture: &ThreadFixture,
    started_at: chrono::DateTime<chrono::FixedOffset>,
) -> Result<(), OxyError> {
    if fixture.events.is_empty() {
        return Ok(());
    }
    // Derived from the same constant the run's `updated_at` uses, so the last
    // event can never be stamped after the run it belongs to finished.
    let step_ms = (RUN_DURATION_SECS * 1_000) / fixture.events.len() as i64;
    let rows: Vec<run_event::ActiveModel> = fixture
        .events
        .iter()
        .enumerate()
        .map(|(i, e)| run_event::ActiveModel {
            id: ActiveValue::NotSet,
            run_id: ActiveValue::Set(run_id.to_string()),
            seq: ActiveValue::Set(e.seq),
            event_type: ActiveValue::Set(e.event_type.clone()),
            payload: ActiveValue::Set(e.payload.clone()),
            attempt: ActiveValue::Set(e.attempt),
            created_at: ActiveValue::Set(started_at + Duration::milliseconds(step_ms * i as i64)),
        })
        .collect();

    run_event::Entity::insert_many(rows)
        .exec(txn)
        .await
        .map_err(|e| OxyError::DBError(format!("insert events for run {run_id}: {e}")))?;
    Ok(())
}

/// Drop every seeded thread. The `threads` row is the only thing that needs
/// deleting: `agentic_runs.thread_id`, `agentic_run_events.run_id` and
/// `analytics_run_extensions.run_id` are all `ON DELETE CASCADE`, so the run,
/// its transcript and its extension go with it.
///
/// Returns the number of threads removed.
///
/// Sweeps **every user in the database**, not the Local-org members the seed
/// writes for. The two sets drift the moment `OXY_GLOBAL_ADMINS` changes
/// between a `seed` and a `--clear`: rebuilding the id list from current
/// membership would silently strand the threads of anyone dropped from that
/// env var, which is precisely the leftover this sweep exists to prevent.
///
/// Deleting by reconstructed id (rather than by a marker column) is what keeps
/// this safe to point at the demo workspace: only ids that hash back to a
/// fixture slug are touched, so a real conversation someone had in the same
/// workspace — same `project_id`, same `source` — can never be caught by it.
pub async fn clear_demo_threads(conn: &DatabaseConnection) -> Result<u64, OxyError> {
    let fixtures = parse_fixtures()?;
    let user_ids: Vec<Uuid> = Users::find()
        .select_only()
        .column(users::Column::Id)
        .into_tuple::<Uuid>()
        .all(conn)
        .await
        .map_err(|e| OxyError::DBError(format!("query users for thread sweep: {e}")))?;

    let ids: Vec<Uuid> = user_ids
        .iter()
        .flat_map(|user_id| {
            fixtures
                .iter()
                .map(move |(f, _)| seed_id("thread", &format!("{}.{user_id}", f.slug)))
        })
        .collect();
    if ids.is_empty() {
        return Ok(0);
    }

    let deleted = Threads::delete_many()
        .filter(threads::Column::Id.is_in(ids))
        .exec(conn)
        .await
        .map_err(|e| OxyError::DBError(format!("delete seeded threads: {e}")))?;
    Ok(deleted.rows_affected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_fixture_parses() {
        let parsed = parse_fixtures().expect("fixtures must parse");
        assert_eq!(parsed.len(), FIXTURES.len());
    }

    /// The slug keys the deterministic ids, so a duplicate would make two
    /// fixtures fight over one thread id and the second would silently
    /// "already exist".
    #[test]
    fn slugs_are_unique() {
        let parsed = parse_fixtures().unwrap();
        let slugs: HashSet<_> = parsed.iter().map(|(f, _)| f.slug.clone()).collect();
        assert_eq!(slugs.len(), parsed.len(), "fixture slugs must be unique");
    }

    /// `agentic_run_events` has a UNIQUE index on `(run_id, seq)` — a fixture
    /// that repeats a seq fails the batch insert at seed time, on a database,
    /// which is a much worse place to find out.
    #[test]
    fn event_seqs_are_unique_within_a_fixture() {
        for (f, _) in parse_fixtures().unwrap() {
            let seqs: HashSet<i64> = f.events.iter().map(|e| e.seq).collect();
            assert_eq!(
                seqs.len(),
                f.events.len(),
                "duplicate event seq in fixture {}",
                f.slug
            );
        }
    }

    /// Every transcript must end in a terminal event. The frontend hangs on a
    /// stream that never reports one, and a seeded thread is replayed through
    /// the same reader as a live one.
    #[test]
    fn every_fixture_terminates() {
        for (f, _) in parse_fixtures().unwrap() {
            let last = f.events.last().expect("fixture has events");
            assert!(
                matches!(last.event_type.as_str(), "done" | "error" | "cancelled"),
                "fixture {} ends on {} — must end on done/error/cancelled",
                f.slug,
                last.event_type
            );
        }
    }

    /// The read path deserializes each payload into the domain's typed event
    /// before handing it to the UI, and **silently drops** anything that fails
    /// — a fixture with a subtly wrong payload seeds green and then renders a
    /// thread with no result in it. (Caught in review: `date_range` is
    /// `[start, end]`, and a plain `"last month"` cost the whole
    /// `query_executed` event.) Parsing against the real type is the only
    /// check that sees this.
    #[test]
    fn semantic_query_payloads_match_the_domain_type() {
        use agentic_analytics::QueryRequestItem;

        let mut checked = 0;
        for (f, _) in parse_fixtures().unwrap() {
            for e in &f.events {
                let spec = match e.event_type.as_str() {
                    "query_executed" => e.payload.get("semantic_query"),
                    "semantic_shortcut_attempted" => Some(&e.payload),
                    _ => None,
                };
                let Some(spec) = spec.filter(|v| !v.is_null()) else {
                    continue;
                };
                serde_json::from_value::<QueryRequestItem>(spec.clone()).unwrap_or_else(|err| {
                    panic!(
                        "fixture {} seq {} ({}) has a semantic query the reader cannot parse, \
                         so the event would be dropped at render time: {err}",
                        f.slug, e.seq, e.event_type
                    )
                });
                checked += 1;
            }
        }
        assert!(checked > 0, "no semantic query payloads were checked");
    }

    /// Mirror of `getShortTitle` in `web-app/src/libs/utils/string.ts` — the
    /// function the Chat panel applies before POSTing a thread.
    fn short_title(message: &str) -> String {
        let words: Vec<&str> = message.trim().split_whitespace().collect();
        let base = words.iter().take(8).copied().collect::<Vec<_>>().join(" ");
        let mut short = if words.len() > 8 {
            base
        } else {
            message.to_string()
        };
        if short.chars().count() > 50 {
            short = short.chars().take(50).collect::<String>() + "...";
        } else if short != message {
            short.push_str("...");
        }
        short
    }

    /// Titles must look like the ones the product writes, or the threads list —
    /// the whole point of this seed — reads as fabricated on a fresh box.
    ///
    /// There are two title-writing paths, and the seed follows the human one:
    /// an API/SDK run takes `question.chars().take(120)` with no ellipsis
    /// (`agentic/http/src/routes/run.rs`, `api-custom-apps/src/agent_ask/mod.rs`), but a
    /// person starting a chat goes through the Chat panel, which sends
    /// `getShortTitle(message)` to `create_thread` — 8 words or 50 chars, plus
    /// an ellipsis. Asserting the `take(120)` shape here would be wrong: it
    /// would reject `wow-sales-growth.json`, whose title is a verbatim capture
    /// of what that panel actually stored.
    #[test]
    fn titles_match_what_the_chat_panel_writes() {
        for (f, _) in parse_fixtures().unwrap() {
            assert_eq!(
                f.title,
                short_title(&f.input),
                "fixture {} has a hand-made title; it must equal getShortTitle(input)",
                f.slug
            );
        }
    }

    /// The run's question is the text the user sent, which is the thread's
    /// `input` — they are two columns holding one fact, and a fixture that
    /// lets them drift renders a header that disagrees with the transcript.
    #[test]
    fn question_matches_input() {
        for (f, _) in parse_fixtures().unwrap() {
            assert_eq!(f.run.question, f.input, "fixture {} drifted", f.slug);
        }
    }

    /// Ids must not move: a developer's bookmark, and the skip-if-present
    /// check that makes re-seeding safe, both depend on the same slug+user
    /// always hashing to the same thread.
    #[test]
    fn seed_ids_are_deterministic_and_namespaced() {
        let user = Uuid::nil();
        let key = format!("wow-sales-growth.{user}");
        assert_eq!(seed_id("thread", &key), seed_id("thread", &key));
        assert_ne!(seed_id("thread", &key), seed_id("run", &key));
    }
}
