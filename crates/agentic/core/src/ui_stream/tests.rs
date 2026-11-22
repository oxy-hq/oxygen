use super::*;
use crate::events::{CoreEvent, Event, Outcome};

#[test]
fn state_enter_maps_to_step_start() {
    let mut s: UiTransformState<()> = UiTransformState::new();
    let blocks = s.process(Event::Core(CoreEvent::StateEnter {
        state: "clarifying".into(),
        revision: 0,
        trace_id: "t".into(),
        sub_spec_index: None,
    }));
    assert_eq!(blocks.len(), 1);
    assert!(
        matches!(&blocks[0], UiBlock::StepStart { label, summary: None, .. } if label == "clarifying")
    );
}

#[test]
fn summary_fn_populates_step_start_summary() {
    let mut s: UiTransformState<()> = UiTransformState::new().with_summary_fn(|state| {
        if state == "clarifying" {
            Some("Understanding your question".into())
        } else {
            None
        }
    });
    let blocks = s.process(Event::Core(CoreEvent::StateEnter {
        state: "clarifying".into(),
        revision: 0,
        trace_id: "t".into(),
        sub_spec_index: None,
    }));
    assert!(matches!(
        &blocks[0],
        UiBlock::StepStart { summary: Some(s), .. } if s == "Understanding your question"
    ));
}

#[test]
fn summary_fn_returns_none_for_unknown_state() {
    let mut s: UiTransformState<()> = UiTransformState::new().with_summary_fn(|_| None);
    let blocks = s.process(Event::Core(CoreEvent::StateEnter {
        state: "executing".into(),
        revision: 0,
        trace_id: "t".into(),
        sub_spec_index: None,
    }));
    assert!(matches!(
        &blocks[0],
        UiBlock::StepStart { summary: None, .. }
    ));
}

#[test]
fn state_exit_advanced_is_success() {
    let mut s: UiTransformState<()> = UiTransformState::new();
    s.process(Event::Core(CoreEvent::StateEnter {
        state: "clarifying".into(),
        revision: 0,
        trace_id: "t".into(),
        sub_spec_index: None,
    }));
    let blocks = s.process(Event::Core(CoreEvent::StateExit {
        state: "clarifying".into(),
        outcome: Outcome::Advanced,
        trace_id: "t".into(),
        sub_spec_index: None,
    }));
    assert!(matches!(
        &blocks[0],
        UiBlock::StepEnd {
            outcome: Outcome::Advanced,
            ..
        }
    ));
}

#[test]
fn state_exit_failed_is_not_success() {
    let mut s: UiTransformState<()> = UiTransformState::new();
    s.process(Event::Core(CoreEvent::StateEnter {
        state: "executing".into(),
        revision: 0,
        trace_id: "t".into(),
        sub_spec_index: None,
    }));
    let blocks = s.process(Event::Core(CoreEvent::StateExit {
        state: "executing".into(),
        outcome: Outcome::Failed,
        trace_id: "t".into(),
        sub_spec_index: None,
    }));
    assert!(matches!(
        &blocks[0],
        UiBlock::StepEnd {
            outcome: Outcome::Failed,
            ..
        }
    ));
}

#[test]
fn back_edge_buffered_and_used_in_diagnosing_summary() {
    let mut s: UiTransformState<()> = UiTransformState::new();

    // BackEdge should be dropped but buffer the reason.
    let dropped = s.process(Event::Core(CoreEvent::BackEdge {
        from: "executing".into(),
        to: "diagnosing".into(),
        reason: "invalid SQL syntax".into(),
        trace_id: "t".into(),
    }));
    assert!(dropped.is_empty());

    // Next StateEnter for "diagnosing" should consume the buffered reason.
    let blocks = s.process(Event::Core(CoreEvent::StateEnter {
        state: "diagnosing".into(),
        revision: 0,
        trace_id: "t".into(),
        sub_spec_index: None,
    }));
    assert!(matches!(
        &blocks[0],
        UiBlock::StepStart { summary: Some(s), .. } if s == "Recovering: invalid SQL syntax"
    ));

    // Reason should be cleared after consumption.
    let blocks2 = s.process(Event::Core(CoreEvent::StateEnter {
        state: "diagnosing".into(),
        revision: 1,
        trace_id: "t".into(),
        sub_spec_index: None,
    }));
    assert!(matches!(
        &blocks2[0],
        UiBlock::StepStart { summary: None, .. }
    ));
}

#[test]
fn back_edge_non_diagnosing_state_uses_summary_fn() {
    let mut s: UiTransformState<()> = UiTransformState::new().with_summary_fn(|state| {
        if state == "solving" {
            Some("Generating a query".into())
        } else {
            None
        }
    });

    s.process(Event::Core(CoreEvent::BackEdge {
        from: "executing".into(),
        to: "solving".into(),
        reason: "bad column".into(),
        trace_id: "t".into(),
    }));

    // Non-diagnosing state: summary_fn wins, pending reason is still buffered.
    let blocks = s.process(Event::Core(CoreEvent::StateEnter {
        state: "solving".into(),
        revision: 1,
        trace_id: "t".into(),
        sub_spec_index: None,
    }));
    assert!(matches!(
        &blocks[0],
        UiBlock::StepStart { summary: Some(s), .. } if s == "Generating a query"
    ));
}

#[test]
fn tool_call_without_summary_fn_emits_one_block() {
    let mut s: UiTransformState<()> = UiTransformState::new();
    let blocks = s.process(Event::Core(CoreEvent::ToolCall {
        name: "list_metrics".into(),
        input: "{}".into(),
        llm_duration_ms: 0,
        sub_spec_index: None,
    }));
    assert_eq!(blocks.len(), 1);
    assert!(matches!(&blocks[0], UiBlock::ToolCall { name, .. } if name == "list_metrics"));
}

#[test]
fn tool_call_with_summary_fn_emits_summary_update() {
    let mut s: UiTransformState<()> = UiTransformState::new().with_tool_summary_fn(|tool, _| {
        if tool == "list_metrics" {
            Some("Checking available metrics".into())
        } else {
            None
        }
    });
    let blocks = s.process(Event::Core(CoreEvent::ToolCall {
        name: "list_metrics".into(),
        input: "{}".into(),
        llm_duration_ms: 0,
        sub_spec_index: None,
    }));
    assert_eq!(blocks.len(), 2);
    assert!(matches!(&blocks[0], UiBlock::ToolCall { .. }));
    assert!(matches!(
        &blocks[1],
        UiBlock::StepSummaryUpdate { summary } if summary == "Checking available metrics"
    ));
}

#[test]
fn tool_call_no_match_in_summary_fn_emits_one_block() {
    let mut s: UiTransformState<()> = UiTransformState::new().with_tool_summary_fn(|_, _| None);
    let blocks = s.process(Event::Core(CoreEvent::ToolCall {
        name: "unknown_tool".into(),
        input: "{}".into(),
        llm_duration_ms: 0,
        sub_spec_index: None,
    }));
    assert_eq!(blocks.len(), 1);
}

#[test]
fn back_edge_is_dropped() {
    let mut s: UiTransformState<()> = UiTransformState::new();
    let blocks = s.process(Event::Core(CoreEvent::BackEdge {
        from: "executing".into(),
        to: "solving".into(),
        reason: "bad SQL".into(),
        trace_id: "t".into(),
    }));
    assert!(blocks.is_empty());
}

#[test]
fn llm_token_maps_to_text_delta() {
    let mut s: UiTransformState<()> = UiTransformState::new();
    let blocks = s.process(Event::Core(CoreEvent::LlmToken {
        token: "hello".into(),
        sub_spec_index: None,
    }));
    assert!(matches!(&blocks[0], UiBlock::TextDelta { token, .. } if token == "hello"));
}

#[test]
fn llm_start_dropped_end_emits_usage() {
    let mut s: UiTransformState<()> = UiTransformState::new();
    assert!(
        s.process(Event::Core(CoreEvent::LlmStart {
            state: "s".into(),
            prompt_tokens: 100,
            sub_spec_index: None,
        }))
        .is_empty()
    );
    let blocks = s.process(Event::Core(CoreEvent::LlmEnd {
        state: "s".into(),
        output_tokens: 50,
        input_tokens: 0,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
        duration_ms: 1234,
        model: "test-model".into(),
        sub_spec_index: None,
        advisor_calls: 0,
        advisor_input_tokens: 0,
        advisor_output_tokens: 0,
    }));
    assert_eq!(blocks.len(), 1);
    assert!(matches!(
        &blocks[0],
        UiBlock::LlmUsage {
            prompt_tokens: 100,
            output_tokens: 50,
            duration_ms: 1234,
            sub_spec_index: None,
            ..
        }
    ));
}

#[test]
fn validation_events_dropped() {
    let mut s: UiTransformState<()> = UiTransformState::new();
    assert!(
        s.process(Event::Core(CoreEvent::ValidationPass { state: "s".into() }))
            .is_empty()
    );
    assert!(
        s.process(Event::Core(CoreEvent::ValidationFail {
            state: "s".into(),
            errors: vec![]
        }))
        .is_empty()
    );
}

#[test]
fn fan_out_emits_fan_out_start() {
    let mut s: UiTransformState<()> = UiTransformState::new();
    let blocks = s.process(Event::Core(CoreEvent::FanOut {
        spec_count: 3,
        trace_id: "t".into(),
    }));
    assert_eq!(blocks.len(), 1);
    assert!(matches!(&blocks[0], UiBlock::FanOutStart { total: 3 }));
}

#[test]
fn sub_spec_start_emits_sub_spec_start() {
    let mut s: UiTransformState<()> = UiTransformState::new();
    let blocks = s.process(Event::Core(CoreEvent::SubSpecStart {
        index: 0,
        total: 3,
        trace_id: "t".into(),
    }));
    assert_eq!(blocks.len(), 1);
    assert!(matches!(
        &blocks[0],
        UiBlock::SubSpecStart { index: 0, total: 3, label }
        if label.contains("1") && label.contains("3")
    ));
}

#[test]
fn last_sub_spec_end_emits_sub_spec_end_and_fan_out_end() {
    let mut s: UiTransformState<()> = UiTransformState::new();
    s.process(Event::Core(CoreEvent::FanOut {
        spec_count: 2,
        trace_id: "t".into(),
    }));
    s.process(Event::Core(CoreEvent::SubSpecStart {
        index: 0,
        total: 2,
        trace_id: "t".into(),
    }));
    s.process(Event::Core(CoreEvent::SubSpecEnd {
        index: 0,
        trace_id: "t".into(),
    }));
    let blocks = s.process(Event::Core(CoreEvent::SubSpecEnd {
        index: 1,
        trace_id: "t".into(),
    }));
    assert_eq!(blocks.len(), 2);
    assert!(matches!(
        &blocks[0],
        UiBlock::SubSpecEnd {
            index: 1,
            success: true
        }
    ));
    assert!(matches!(&blocks[1], UiBlock::FanOutEnd { success: true }));
}

#[test]
fn intermediate_sub_spec_end_does_not_emit_fan_out_end() {
    let mut s: UiTransformState<()> = UiTransformState::new();
    s.process(Event::Core(CoreEvent::FanOut {
        spec_count: 3,
        trace_id: "t".into(),
    }));
    let blocks = s.process(Event::Core(CoreEvent::SubSpecEnd {
        index: 0,
        trace_id: "t".into(),
    }));
    assert_eq!(blocks.len(), 1);
    assert!(matches!(&blocks[0], UiBlock::SubSpecEnd { .. }));
}

#[test]
fn step_end_label_mirrors_step_start() {
    let mut s: UiTransformState<()> = UiTransformState::new();
    s.process(Event::Core(CoreEvent::StateEnter {
        state: "executing".into(),
        revision: 0,
        trace_id: "t".into(),
        sub_spec_index: None,
    }));
    let blocks = s.process(Event::Core(CoreEvent::StateExit {
        state: "executing".into(),
        outcome: Outcome::Advanced,
        trace_id: "t".into(),
        sub_spec_index: None,
    }));
    assert!(
        matches!(&blocks[0], UiBlock::StepEnd { label, outcome: Outcome::Advanced, .. } if label == "executing")
    );
}

#[test]
fn domain_event_passes_through() {
    let mut s: UiTransformState<()> = UiTransformState::new();
    let blocks = s.process(Event::Domain(()));
    assert_eq!(blocks.len(), 1);
    assert!(matches!(&blocks[0], UiBlock::Domain(())));
}

#[test]
fn test_interleaved_sub_spec_events() {
    let mut s: UiTransformState<()> = UiTransformState::new();

    // FanOut { 3 }
    let blocks = s.process(Event::Core(CoreEvent::FanOut {
        spec_count: 3,
        trace_id: "t".into(),
    }));
    assert_eq!(blocks.len(), 1);
    assert!(matches!(&blocks[0], UiBlock::FanOutStart { total: 3 }));

    // SubSpecStart { 0 }
    let blocks = s.process(Event::Core(CoreEvent::SubSpecStart {
        index: 0,
        total: 3,
        trace_id: "t".into(),
    }));
    assert_eq!(blocks.len(), 1);
    assert!(matches!(
        &blocks[0],
        UiBlock::SubSpecStart {
            index: 0,
            total: 3,
            ..
        }
    ));

    // SubSpecStart { 1 }
    let blocks = s.process(Event::Core(CoreEvent::SubSpecStart {
        index: 1,
        total: 3,
        trace_id: "t".into(),
    }));
    assert_eq!(blocks.len(), 1);
    assert!(matches!(
        &blocks[0],
        UiBlock::SubSpecStart {
            index: 1,
            total: 3,
            ..
        }
    ));

    // StateEnter("solving", sub_spec_index: Some(0))
    let blocks = s.process(Event::Core(CoreEvent::StateEnter {
        state: "solving".into(),
        revision: 0,
        trace_id: "t".into(),
        sub_spec_index: Some(0),
    }));
    assert_eq!(blocks.len(), 1);
    assert!(matches!(
        &blocks[0],
        UiBlock::StepStart {
            sub_spec_index: Some(0),
            ..
        }
    ));

    // StateEnter("solving", sub_spec_index: Some(1))
    let blocks = s.process(Event::Core(CoreEvent::StateEnter {
        state: "solving".into(),
        revision: 0,
        trace_id: "t".into(),
        sub_spec_index: Some(1),
    }));
    assert_eq!(blocks.len(), 1);
    assert!(matches!(
        &blocks[0],
        UiBlock::StepStart {
            sub_spec_index: Some(1),
            ..
        }
    ));

    // SubSpecEnd { 0 } — first completion, no FanOutEnd yet
    let blocks = s.process(Event::Core(CoreEvent::SubSpecEnd {
        index: 0,
        trace_id: "t".into(),
    }));
    assert_eq!(blocks.len(), 1);
    assert!(matches!(&blocks[0], UiBlock::SubSpecEnd { index: 0, .. }));

    // SubSpecStart { 2 }
    let blocks = s.process(Event::Core(CoreEvent::SubSpecStart {
        index: 2,
        total: 3,
        trace_id: "t".into(),
    }));
    assert_eq!(blocks.len(), 1);
    assert!(matches!(
        &blocks[0],
        UiBlock::SubSpecStart {
            index: 2,
            total: 3,
            ..
        }
    ));

    // SubSpecEnd { 1 } — second completion, no FanOutEnd yet
    let blocks = s.process(Event::Core(CoreEvent::SubSpecEnd {
        index: 1,
        trace_id: "t".into(),
    }));
    assert_eq!(blocks.len(), 1);
    assert!(matches!(&blocks[0], UiBlock::SubSpecEnd { index: 1, .. }));

    // SubSpecEnd { 2 } — third and final completion, FanOutEnd emitted
    let blocks = s.process(Event::Core(CoreEvent::SubSpecEnd {
        index: 2,
        trace_id: "t".into(),
    }));
    assert_eq!(blocks.len(), 2);
    assert!(matches!(&blocks[0], UiBlock::SubSpecEnd { index: 2, .. }));
    assert!(matches!(&blocks[1], UiBlock::FanOutEnd { success: true }));
}

#[test]
fn test_sub_spec_index_propagated_to_ui_blocks() {
    let mut s: UiTransformState<()> = UiTransformState::new();
    let blocks = s.process(Event::Core(CoreEvent::StateEnter {
        state: "solving".into(),
        revision: 0,
        trace_id: "t".into(),
        sub_spec_index: Some(2),
    }));
    assert_eq!(blocks.len(), 1);
    assert!(matches!(
        &blocks[0],
        UiBlock::StepStart { label, sub_spec_index: Some(2), .. } if label == "solving"
    ));
}

#[test]
fn test_none_sub_spec_index_outside_fanout() {
    let mut s: UiTransformState<()> = UiTransformState::new();

    // StateEnter without fan-out context
    let blocks = s.process(Event::Core(CoreEvent::StateEnter {
        state: "clarifying".into(),
        revision: 0,
        trace_id: "t".into(),
        sub_spec_index: None,
    }));
    assert_eq!(blocks.len(), 1);
    assert!(matches!(
        &blocks[0],
        UiBlock::StepStart {
            sub_spec_index: None,
            ..
        }
    ));

    // StateExit without fan-out context
    let blocks = s.process(Event::Core(CoreEvent::StateExit {
        state: "clarifying".into(),
        outcome: Outcome::Advanced,
        trace_id: "t".into(),
        sub_spec_index: None,
    }));
    assert_eq!(blocks.len(), 1);
    assert!(matches!(
        &blocks[0],
        UiBlock::StepEnd {
            sub_spec_index: None,
            ..
        }
    ));

    // LlmToken without fan-out context
    let blocks = s.process(Event::Core(CoreEvent::LlmToken {
        token: "hi".into(),
        sub_spec_index: None,
    }));
    assert_eq!(blocks.len(), 1);
    assert!(matches!(
        &blocks[0],
        UiBlock::TextDelta {
            sub_spec_index: None,
            ..
        }
    ));

    // ToolCall without fan-out context
    let blocks = s.process(Event::Core(CoreEvent::ToolCall {
        name: "some_tool".into(),
        input: "{}".into(),
        llm_duration_ms: 0,
        sub_spec_index: None,
    }));
    assert_eq!(blocks.len(), 1);
    assert!(matches!(
        &blocks[0],
        UiBlock::ToolCall {
            sub_spec_index: None,
            ..
        }
    ));
}
