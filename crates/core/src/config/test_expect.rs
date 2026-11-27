//! A test case's `expect:` block: behaviours of the run that are checked
//! deterministically from its events, beside (or instead of) the answer text.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Which code path produced the SQL the answer rests on.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExpectQuerySource {
    Semantic,
    Llm,
    Vendor,
    VerifiedSql,
}

impl ExpectQuerySource {
    /// The name the eval record uses for this source (`timing.answer_source`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Semantic => "semantic",
            Self::Llm => "llm",
            Self::Vendor => "vendor",
            Self::VerifiedSql => "verified_sql",
        }
    }
}

/// Behaviours the run must show. Every field is optional; an unset field is not
/// checked, and a case fails when any set field is unmet.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TestExpect {
    /// `true`: the run must stop to ask the user a clarifying question. `false`:
    /// it must answer.
    pub asks_user: Option<bool>,
    /// Whether the run must (`true`) or must not (`false`) draw a chart.
    pub chart: Option<bool>,
    /// Whether a successful query must (`true`) or must not (`false`) be served
    /// from a pre-aggregation.
    pub preagg: Option<bool>,
    /// The source of the query the answer rests on.
    pub query_source: Option<ExpectQuerySource>,
    /// Tools the run must call, by name.
    #[serde(default)]
    pub tools: Vec<String>,
}

impl TestExpect {
    /// The case expects the run to stop on a clarifying question.
    pub fn expects_question(&self) -> bool {
        self.asks_user == Some(true)
    }

    /// No behaviour is checked.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}
