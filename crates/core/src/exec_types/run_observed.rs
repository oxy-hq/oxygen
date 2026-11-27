use serde::{Deserialize, Serialize};

/// What an agentic eval run did, read off its event stream: what a case's
/// `expect:` block is checked against. Both runtimes fill it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunObserved {
    /// Every tool call's name, in call order (a tool called twice is listed
    /// twice). Names are the runtime's own: react and the FSM name tools
    /// differently.
    #[serde(default)]
    pub tools: Vec<String>,
    /// `source` of each successful query, in order (`semantic`, `llm`,
    /// `vendor`, `verified_sql`).
    #[serde(default)]
    pub query_sources: Vec<String>,
    /// `source` of the query the answer rests on (`timing.answer_source`).
    #[serde(default)]
    pub answer_source: Option<String>,
    /// A successful query was served from a pre-aggregation.
    #[serde(default)]
    pub preagg: bool,
    /// Charts drawn: react's `ResultChart`, the FSM's non-table `ChartRendered`.
    #[serde(default)]
    pub charts: u32,
    /// The clarifying question the run stopped on; `None` when it answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_user: Option<String>,
}
