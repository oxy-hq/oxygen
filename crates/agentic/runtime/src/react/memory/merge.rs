//! Merging the note log once it outgrows the prompt. Runs that write notes
//! do not see each other's (the eval's learning runs read none, and parallel
//! runs see a snapshot), so the log grows with near-duplicates until the
//! prompt cuts its oldest notes. One model call rewrites the live notes into
//! fewer, and the code, not the prompt, holds the result to the caps.
//!
//! Run it with one writer: the eval calls it once per notes file, after every
//! case of the pass has finished.

use agentic_core::react::{Decision, Engine, Entry, Transcript, Turn, TurnUsage};
use agentic_core::tools::ToolDef;
use serde_json::{Value, json};

use super::{MAX_NOTE_CHARS, MemoryError, MemoryStore, Note, NoteFact, NoteOp, fits, latest};

/// What a merge aims for, below [`super::MAX_PROMPT_CHARS`], so later runs
/// have room to add notes before the next merge.
pub const MERGE_TARGET_CHARS: usize = 3_000;

/// The longest merged note accepted. A merged note may join two facts, so it
/// may run past [`MAX_NOTE_CHARS`], which the prompt still asks for; the
/// prompt-size check ([`fits`]) holds the total either way.
pub const MAX_MERGED_NOTE_CHARS: usize = MAX_NOTE_CHARS * 3 / 2;

const SAVE_TOOL: &str = "save_notes";

const MERGE_SYSTEM: &str = "You keep the notes an analytics agent wrote for itself about one \
    workspace: how to answer a kind of question (the view, measure and filter), query shapes \
    that failed and the ones that worked, data quirks, and user corrections. The list is too \
    long for the agent's prompt. Rewrite it shorter.";

/// What one merge did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MergeReport {
    pub before: usize,
    pub after: usize,
    /// The merge call's usage, to count apart from the answers' cost.
    pub usage: TurnUsage,
}

#[derive(thiserror::Error, Debug)]
pub enum MergeError {
    #[error(transparent)]
    Store(#[from] MemoryError),
    #[error("memory merge: the model call failed: {0}")]
    Engine(String),
    /// The reply broke a cap or was empty: nothing was written.
    #[error("memory merge: reply not used, nothing written: {0}")]
    Invalid(String),
}

/// Merges `store`'s live notes when the prompt cannot list them all.
/// `Ok(None)`: they fit, and no model call was made. On any error nothing is
/// written. On success the merged notes are appended first and the old ids
/// deleted after, so a failure part way leaves duplicates, never a loss.
pub async fn merge_notes(
    engine: &dyn Engine,
    store: &dyn MemoryStore,
    run_id: &str,
) -> Result<Option<MergeReport>, MergeError> {
    let old = latest(store.facts().await?);
    if fits(&old) {
        return Ok(None);
    }
    let (merged, usage) = ask_until_valid(engine, &old).await?;
    let now = chrono::Utc::now();
    let fact = |note_id: String, op, text: String| NoteFact {
        note_id,
        op,
        text,
        run_id: run_id.to_string(),
        recorded_at: now,
    };
    // `latest` lists the last appended first among equal times: append in
    // reverse so the prompt keeps the model's order, most useful first.
    for n in merged.iter().rev() {
        store
            .append(fact(n.note_id.clone(), NoteOp::Put, n.text.clone()))
            .await?;
    }
    for n in &old {
        store
            .append(fact(n.note_id.clone(), NoteOp::Delete, String::new()))
            .await?;
    }
    Ok(Some(MergeReport {
        before: old.len(),
        after: merged.len(),
        usage,
    }))
}

/// How many replies a merge asks for before it gives up.
const MERGE_ATTEMPTS: usize = 3;

/// Asks for the merged list, and on a reply that breaks a cap, tells the
/// model why and asks again, up to [`MERGE_ATTEMPTS`] replies. Usage sums
/// every reply.
async fn ask_until_valid(
    engine: &dyn Engine,
    old: &[Note],
) -> Result<(Vec<Note>, TurnUsage), MergeError> {
    let mut transcript = Transcript {
        system: MERGE_SYSTEM.into(),
        entries: vec![Entry::User(listing(old))],
    };
    let mut usage = TurnUsage::default();
    let mut last = String::new();
    for _ in 0..MERGE_ATTEMPTS {
        let turn = ask(engine, &transcript).await?;
        add(&mut usage, &turn.usage);
        let reason = match parse(&turn) {
            Ok(notes) => return Ok((notes, usage)),
            Err(MergeError::Invalid(reason)) => reason,
            Err(e) => return Err(e),
        };
        let retry = format!("Not saved: {reason}. Call `{SAVE_TOOL}` again with a list that fits.");
        let answers = answer_every_call(&turn, retry);
        transcript.entries.push(Entry::Assistant(turn));
        transcript.entries.extend(answers);
        last = reason;
    }
    Err(MergeError::Invalid(format!(
        "{last} (after {MERGE_ATTEMPTS} replies)"
    )))
}

/// The entries that send `retry` back. The provider refuses a tool call with
/// no result, so every call gets one: `retry` on the call [`parse`] read
/// (the first `save_notes`, else the first call), a short error on the rest.
/// A reply with no call gets `retry` as a user message.
fn answer_every_call(turn: &Turn, retry: String) -> Vec<Entry> {
    let read = (turn.tool_calls.iter())
        .position(|c| c.name == SAVE_TOOL)
        .unwrap_or(0);
    if turn.tool_calls.is_empty() {
        return vec![Entry::User(retry)];
    }
    (turn.tool_calls.iter().enumerate())
        .map(|(i, c)| Entry::ToolResult {
            call_id: c.id.clone(),
            text: if i == read {
                retry.clone()
            } else {
                format!("Ignored: only one `{SAVE_TOOL}` call is read.")
            },
            is_error: true,
        })
        .collect()
}

fn add(total: &mut TurnUsage, u: &TurnUsage) {
    total.input_tokens += u.input_tokens;
    total.output_tokens += u.output_tokens;
    total.cache_read_tokens += u.cache_read_tokens;
    total.cache_write_tokens += u.cache_write_tokens;
    total.cost_usd = match (total.cost_usd, u.cost_usd) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
    };
    if total.model.is_empty() {
        total.model = u.model.clone();
    }
}

async fn ask(engine: &dyn Engine, transcript: &Transcript) -> Result<Turn, MergeError> {
    // The engine streams events; nobody reads them here, so drain them in
    // the same task (a spawn would need a Sentry hub of its own).
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let decide = async move {
        let decided = engine.decide(transcript, &[save_def()], &tx).await;
        drop(tx);
        decided
    };
    let (decided, ()) = tokio::join!(decide, async { while rx.recv().await.is_some() {} });
    match decided {
        Ok(Decision::Reply(t) | Decision::CallTools(t)) => Ok(t),
        Err(e) => Err(MergeError::Engine(e.to_string())),
    }
}

/// The user turn: the rules, then every live note, newest first.
fn listing(old: &[Note]) -> String {
    let mut s = format!(
        "Merge these {} notes into a shorter list and save it with `{SAVE_TOOL}`, most useful \
         first. Join notes that say the same thing or overlap. Where two notes disagree, the \
         newer one (higher in the list) wins. Drop a note only when another note covers it or \
         it is about one question and no other. Keep view, measure, column and filter names \
         exactly. Limits, checked by code: each note at most {MAX_NOTE_CHARS} characters (never over \
         {MAX_MERGED_NOTE_CHARS}), and \
         all of them together at most about {MERGE_TARGET_CHARS} characters (the {} notes \
         below have {} characters). Count before you save. Never add a fact that is not in the notes.\n\n\
         Notes, newest first:\n",
        old.len(),
        old.len(),
        old.iter().map(|n| n.text.chars().count()).sum::<usize>()
    );
    for n in old {
        s.push_str(&format!("- {}\n", n.text));
    }
    s
}

fn save_def() -> ToolDef {
    ToolDef {
        name: SAVE_TOOL,
        description: "Save the merged notes. They replace every note listed.",
        parameters: json!({
            "type": "object",
            "properties": {"notes": {"type": "array", "items": {"type": "string"}}},
            "required": ["notes"]
        }),
        strict: false,
    }
}

/// The merged notes from `turn`'s `save_notes` call, with new ids, when they
/// are not empty, each note is within [`MAX_MERGED_NOTE_CHARS`] and the prompt
/// lists them all.
fn parse(turn: &Turn) -> Result<Vec<Note>, MergeError> {
    let bad = |m: String| Err(MergeError::Invalid(m));
    let Some(call) = turn.tool_calls.iter().find(|c| c.name == SAVE_TOOL) else {
        return bad(format!("no {SAVE_TOOL} call"));
    };
    let Some(items) = call.input.get("notes").and_then(Value::as_array) else {
        return bad("`notes` is not a list".into());
    };
    let texts: Vec<&str> = (items.iter())
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();
    if texts.is_empty() {
        return bad("no notes".into());
    }
    let long: Vec<String> = (texts.iter().enumerate())
        .map(|(i, t)| (i + 1, t.chars().count()))
        .filter(|(_, n)| *n > MAX_MERGED_NOTE_CHARS)
        .map(|(i, n)| format!("note {i} has {n}"))
        .collect();
    if !long.is_empty() {
        return bad(format!(
            "{} (at most {MAX_MERGED_NOTE_CHARS} characters each, {MAX_NOTE_CHARS} is better); \
             shorten or split them",
            long.join(", ")
        ));
    }
    let now = chrono::Utc::now();
    let notes: Vec<Note> = (texts.into_iter())
        .map(|t| Note {
            note_id: format!("n{}", &uuid::Uuid::new_v4().simple().to_string()[..8]),
            text: t.to_string(),
            recorded_at: now,
        })
        .collect();
    if !fits(&notes) {
        let total: usize = notes.iter().map(|n| n.text.chars().count()).sum();
        return bad(format!(
            "{} notes with {total} characters in all still overflow the prompt; merge or \
             drop notes until they total about {MERGE_TARGET_CHARS}",
            notes.len()
        ));
    }
    Ok(notes)
}

#[cfg(test)]
#[path = "merge_tests.rs"]
mod tests;
