//! Tool-call ids that are non-empty and unique within a run.
//!
//! The react runtime matches a result to its call by id on resume, so a
//! provider that sends an empty or repeated id would mis-pair results. The
//! rewrite happens on the folded stream, before `Turn.native` is rendered, so
//! the id in `native` (and a provider-native tool item's `call_id`) is the id
//! the runtime sees.

use std::collections::HashSet;

use agentic_core::react::{Entry, Transcript};
use serde_json::Value;

use super::fold::Folded;
use crate::ContentBlock;

/// Ids already used by the assistant turns of `t`, and how many there are.
pub(super) fn taken_ids(t: &Transcript) -> (HashSet<String>, usize) {
    let mut ids = HashSet::new();
    let mut turns = 0;
    for entry in &t.entries {
        if let Entry::Assistant(turn) = entry {
            turns += 1;
            ids.extend(turn.tool_calls.iter().map(|c| c.id.clone()));
        }
    }
    (ids, turns)
}

/// Rewrites every missing or repeated id in `f` to `call_<turn>_<call>`. The
/// first use of an id keeps it. Calls in `f.tool_calls` and the matching
/// `ToolUse` blocks in `f.ordered` and `f.in_order` are the same calls in the
/// same order.
pub(super) fn ensure_unique(f: &mut Folded, mut taken: HashSet<String>, turn_index: usize) {
    let mut ordered_pos = tool_use_positions(&f.ordered).into_iter();
    let mut in_order_pos = tool_use_positions(&f.in_order).into_iter();
    for (call_index, tc) in f.tool_calls.iter_mut().enumerate() {
        let (a, b) = (ordered_pos.next(), in_order_pos.next());
        if !tc.id.is_empty() && taken.insert(tc.id.clone()) {
            continue;
        }
        let new_id = fresh(&taken, turn_index, call_index);
        taken.insert(new_id.clone());
        tracing::warn!(old = %tc.id, new = %new_id, "react engine: rewrote a missing or repeated tool-call id");
        set_id(&mut tc.provider_data, &new_id);
        tc.id = new_id.clone();
        rename_block(a.and_then(|i| f.ordered.get_mut(i)), &new_id);
        rename_block(b.and_then(|i| f.in_order.get_mut(i)), &new_id);
    }
}

fn tool_use_positions(blocks: &[ContentBlock]) -> Vec<usize> {
    blocks
        .iter()
        .enumerate()
        .filter(|(_, b)| matches!(b, ContentBlock::ToolUse { .. }))
        .map(|(i, _)| i)
        .collect()
}

fn rename_block(block: Option<&mut ContentBlock>, new_id: &str) {
    if let Some(ContentBlock::ToolUse {
        id, provider_data, ..
    }) = block
    {
        *id = new_id.to_string();
        set_id(provider_data, new_id);
    }
}

fn fresh(taken: &HashSet<String>, turn_index: usize, call_index: usize) -> String {
    let mut id = format!("call_{turn_index}_{call_index}");
    while taken.contains(&id) {
        id.push('_');
    }
    id
}

/// A provider-native tool item (OpenAI `function_call`) carries its own
/// `call_id`; Anthropic's wire block is rebuilt from `id` and has none.
fn set_id(provider_data: &mut Option<Value>, id: &str) {
    if let Some(Value::Object(o)) = provider_data
        && o.contains_key("call_id")
    {
        o.insert("call_id".into(), Value::String(id.into()));
    }
}
