use std::time::{Duration, Instant};

use agentic_core::events::{CoreEvent, Outcome};

use super::*;

type Ev = AgenticEvent<AnalyticsEvent>;

fn core(ev: CoreEvent) -> Ev {
    AgenticEvent::Core(ev)
}

fn llm_start(state: &str) -> Ev {
    core(CoreEvent::LlmStart {
        state: state.into(),
        prompt_tokens: 10,
        sub_spec_index: None,
    })
}

fn llm_token() -> Ev {
    core(CoreEvent::LlmToken {
        token: "x".into(),
        sub_spec_index: None,
    })
}

fn thinking_token() -> Ev {
    core(CoreEvent::ThinkingToken {
        token: "t".into(),
        sub_spec_index: None,
    })
}

fn llm_end(state: &str, duration_ms: u64) -> Ev {
    core(CoreEvent::LlmEnd {
        state: state.into(),
        output_tokens: 7,
        cache_creation_input_tokens: 2,
        cache_read_input_tokens: 3,
        duration_ms,
        model: "m".into(),
        sub_spec_index: None,
    })
}

fn tool_result(name: &str, duration_ms: u64, is_error: bool) -> Ev {
    core(CoreEvent::ToolResult {
        name: name.into(),
        output: String::new(),
        duration_ms,
        is_error,
        sub_spec_index: None,
    })
}

fn query(duration_ms: u64, success: bool) -> Ev {
    AgenticEvent::Domain(AnalyticsEvent::QueryExecuted {
        query: "select 1".into(),
        row_count: 0,
        duration_ms,
        success,
        error: None,
        columns: vec![],
        rows: vec![],
        source: agentic_analytics::QuerySource::Llm,
        is_preagg: false,
        sub_spec_index: None,
        semantic_query: None,
    })
}

fn enter(state: &str) -> Ev {
    core(CoreEvent::StateEnter {
        state: state.into(),
        revision: 0,
        trace_id: "t".into(),
        sub_spec_index: None,
    })
}

fn exit(state: &str) -> Ev {
    core(CoreEvent::StateExit {
        state: state.into(),
        outcome: Outcome::Advanced,
        trace_id: "t".into(),
        sub_spec_index: None,
    })
}

/// Feed `(offset_ms, event)` pairs at `t0 + offset`, finish at `t0 + end_ms`.
fn run(events: Vec<(u64, Ev)>, end_ms: u64) -> RunTiming {
    let t0 = Instant::now();
    let at = |ms: u64| t0 + Duration::from_millis(ms);
    let mut acc = TimingAccumulator::new(t0);
    for (ms, ev) in &events {
        acc.observe(at(*ms), ev);
    }
    acc.finish(at(end_ms))
}

#[test]
fn timing_sums_llm_tool_query() {
    let t = run(
        vec![
            (10, llm_start("solving")),
            (20, llm_end("solving", 1000)),
            (30, tool_result("lookup", 200, false)),
            (40, llm_start("interpreting")),
            (50, llm_end("interpreting", 500)),
            (60, query(300, true)),
            (70, query(5, false)),
            (80, tool_result("lookup", 1, true)),
        ],
        100,
    );
    assert_eq!((t.llm_calls, t.llm_ms), (2, 1500));
    assert_eq!((t.tool_calls, t.tool_ms), (2, 201));
    assert_eq!((t.query_count, t.query_ms, t.query_failures), (2, 305, 1));
    assert_eq!((t.output_tokens, t.cache_read_input_tokens), (14, 6));
    assert_eq!(t.cache_creation_input_tokens, 4);
    let solving = &t.llm_by_state["solving"];
    assert_eq!((solving.calls, solving.ms), (1, 1000));
    let lookup = &t.tools_by_name["lookup"];
    assert_eq!((lookup.calls, lookup.ms, lookup.errors), (2, 201, 1));
    assert_eq!((t.wall_ms, t.startup_ms), (100, 10));
}

#[test]
fn ttft_measured_from_llm_start_to_first_token() {
    let t = run(
        vec![
            (0, llm_start("solving")),
            (120, thinking_token()),
            (150, llm_token()),
            (400, llm_end("solving", 400)),
            (500, llm_start("solving")),
            (530, llm_token()),
            (600, llm_end("solving", 100)),
        ],
        900,
    );
    assert_eq!(t.llm_ttft_rounds, 2);
    assert_eq!(t.llm_ttft_ms, 120 + 30);
    assert_eq!(t.llm_stream_ms, (400 - 120) + (100 - 30));
}

/// A round with no streamed token (structured response / tool calls only)
/// counts in `llm_ms` and `llm_calls`, but in neither TTFT nor stream time.
#[test]
fn unmeasured_round_excluded_from_ttft_and_stream() {
    let t = run(
        vec![
            (0, llm_start("solving")),
            (50, llm_token()),
            (300, llm_end("solving", 300)),
            (400, llm_start("specifying")),
            (900, llm_end("specifying", 500)),
        ],
        1000,
    );
    assert_eq!((t.llm_calls, t.llm_ms), (2, 800));
    assert_eq!(t.llm_ttft_rounds, 1);
    assert_eq!(t.llm_ttft_ms, 50);
    assert_eq!(t.llm_stream_ms, 300 - 50);
}

#[test]
fn states_accumulate_reentries() {
    let t = run(
        vec![
            (0, enter("solving")),
            (100, exit("solving")),
            (150, enter("executing")),
            (160, exit("executing")),
            (200, enter("solving")),
            (450, exit("solving")),
            // Still open at run end: closed at the end instant.
            (500, enter("interpreting")),
        ],
        600,
    );
    assert_eq!(t.states["solving"], 100 + 250);
    assert_eq!(t.states["executing"], 10);
    assert_eq!(t.states["interpreting"], 100);
}

#[test]
fn fanout_disables_other_ms() {
    let t = run(
        vec![
            (
                0,
                core(CoreEvent::FanOut {
                    spec_count: 2,
                    trace_id: "t".into(),
                }),
            ),
            (10, llm_end("solving", 50)),
        ],
        1000,
    );
    assert!(t.fanout);
    assert_eq!(t.other_ms, None);
    let json = serde_json::to_value(&t).unwrap();
    assert!(
        json["other_ms"].is_null(),
        "other_ms must serialize as null"
    );
}

#[test]
fn other_ms_is_remainder_floored() {
    let t = run(
        vec![
            (100, llm_end("solving", 400)),
            (600, tool_result("lookup", 200, false)),
            (900, query(100, true)),
        ],
        1000,
    );
    // 1000 wall − 100 startup − 400 llm − 200 tool − 100 query.
    assert_eq!(t.other_ms, Some(200));

    let over = run(vec![(100, llm_end("solving", 5000))], 1000);
    assert_eq!(over.other_ms, Some(0));
}

#[test]
fn no_events_means_startup_is_whole_wall() {
    let t = run(vec![], 250);
    assert_eq!((t.wall_ms, t.startup_ms, t.other_ms), (250, 250, Some(0)));
}

/// The headless pump drops events on a full sink, so a lost `FanOut` must not
/// yield a numeric `other_ms`: any event tagged with a sub-spec index implies
/// fan-out.
#[test]
fn sub_spec_events_imply_fanout() {
    let tagged = core(CoreEvent::LlmEnd {
        state: "solving".into(),
        output_tokens: 1,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
        duration_ms: 50,
        model: "m".into(),
        sub_spec_index: Some(1),
    });
    let t = run(vec![(10, tagged)], 1000);
    assert!(t.fanout);
    assert_eq!(t.other_ms, None);
}
