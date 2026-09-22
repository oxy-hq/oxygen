//! Pairwise LLM-based consistency evaluator.
//!
//! Compares all C(N,2) pairs of candidate answers and picks the one that
//! wins the most pairwise comparisons — i.e. the answer the candidates most
//! agree on.

use std::collections::HashMap;

use agentic_core::evaluator::{ConsistencyEvaluator, EvalError, EvalResult};
use agentic_core::hub_task::bind_current_hub;
use async_trait::async_trait;

use crate::LlmClient;

/// Maximum number of output tokens per pairwise comparison call.
/// A single letter ("A" or "B") is all we need.
const PAIRWISE_MAX_TOKENS: u32 = 1;

const DEFAULT_SYSTEM_PROMPT: &str = "\
You are comparing two candidate answers to the same question. \
Determine which answer is more consistent with what a consensus of respondents would agree on. \
Reply with ONLY the letter: A or B";

/// Consistency evaluator that uses pairwise LLM comparisons.
///
/// For N answers, generates all C(N,2) pairs, asks the LLM to pick the
/// consensus answer for each pair, and returns the answer with the most
/// pairwise wins.
pub struct LlmConsistencyEvaluator {
    client: LlmClient,
}

impl LlmConsistencyEvaluator {
    pub fn new(client: LlmClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ConsistencyEvaluator for LlmConsistencyEvaluator {
    async fn evaluate(
        &self,
        question: &str,
        answers: &[String],
        custom_prompt: Option<&str>,
    ) -> Result<EvalResult, EvalError> {
        let n = answers.len();

        if n == 0 {
            return Err(EvalError("no answers to evaluate".into()));
        }
        if n == 1 {
            return Ok(EvalResult {
                selected_index: 0,
                score: 1.0,
                reasoning: "single answer".into(),
            });
        }

        // Short-circuit: all answers identical.
        if answers.windows(2).all(|w| w[0] == w[1]) {
            return Ok(EvalResult {
                selected_index: 0,
                score: 1.0,
                reasoning: "all answers identical".into(),
            });
        }

        // Build all C(N,2) pairs and run comparisons concurrently.
        let system = custom_prompt.unwrap_or(DEFAULT_SYSTEM_PROMPT);
        let mut join_set = tokio::task::JoinSet::new();

        for i in 0..n {
            for j in (i + 1)..n {
                let client = self.client.clone();
                let sys = system.to_string();
                let user_msg = format!(
                    "Question: {question}\n\nAnswer A:\n{}\n\nAnswer B:\n{}",
                    answers[i], answers[j],
                );
                join_set.spawn(bind_current_hub(async move {
                    let response = client
                        .complete_with_max_tokens(&sys, &user_msg, PAIRWISE_MAX_TOKENS)
                        .await;
                    (i, j, response)
                }));
            }
        }

        // Tally votes.
        let mut votes: HashMap<usize, usize> = HashMap::new();
        for idx in 0..n {
            votes.insert(idx, 0);
        }

        while let Some(result) = join_set.join_next().await {
            let (i, j, response) = result.map_err(|e| EvalError(format!("join error: {e}")))?;
            match response {
                Ok(text) => {
                    let winner = if parse_ab_response(&text) == "A" {
                        i
                    } else {
                        j
                    };
                    *votes.entry(winner).or_insert(0) += 1;
                }
                Err(e) => {
                    tracing::warn!(pair_i = i, pair_j = j, error = %e, "pairwise comparison failed, skipping");
                }
            }
        }

        // Pick the answer with the most wins.
        let (best_idx, best_count) = votes.into_iter().max_by_key(|(_, count)| *count).unwrap(); // safe: n >= 2

        let max_possible = n - 1;
        let score = best_count as f64 / max_possible as f64;

        Ok(EvalResult {
            selected_index: best_idx,
            score,
            reasoning: format!("{best_count}/{max_possible} pairwise wins"),
        })
    }
}

/// Parse a pairwise comparison response for "A" or "B".
/// Scans lines from the end; defaults to "B" on ambiguity.
fn parse_ab_response(response: &str) -> &'static str {
    for line in response.lines().rev() {
        let trimmed = line.trim();
        if trimmed == "A" || trimmed.starts_with('A') {
            return "A";
        }
        if trimmed == "B" || trimmed.starts_with('B') {
            return "B";
        }
    }
    // Default to "B" on ambiguity (same convention as existing AgentPicker).
    "B"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Chunk, LlmError, LlmProvider, ThinkingConfig, types::ResponseSchema};
    use agentic_core::tools::ToolDef;
    use futures_core::Stream;
    use serde_json::Value;
    use std::collections::VecDeque;
    use std::pin::Pin;
    use std::sync::Mutex;

    /// A mock provider that returns pre-programmed text chunks.
    struct MockProvider {
        responses: Mutex<VecDeque<String>>,
    }

    impl MockProvider {
        fn new(responses: Vec<&str>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().map(|s| s.to_string()).collect()),
            }
        }
    }

    #[async_trait]
    impl LlmProvider for MockProvider {
        async fn stream(
            &self,
            _system: &str,
            _system_date_suffix: &str,
            _messages: &[Value],
            _tools: &[ToolDef],
            _thinking: &ThinkingConfig,
            _response_schema: Option<&ResponseSchema>,
            _max_tokens_override: Option<u32>,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<Chunk, LlmError>> + Send>>, LlmError> {
            let text = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| "B".to_string());
            Ok(Box::pin(tokio_stream::iter(vec![Ok(Chunk::Text(text))])))
        }

        fn assistant_message(&self, _blocks: &[crate::ContentBlock]) -> Value {
            Value::Null
        }

        fn tool_result_messages(&self, _results: &[(String, String, bool)]) -> Vec<Value> {
            vec![]
        }

        fn model_name(&self) -> &str {
            "mock"
        }
    }

    fn make_evaluator(responses: Vec<&str>) -> LlmConsistencyEvaluator {
        let client = LlmClient::with_provider(MockProvider::new(responses));
        LlmConsistencyEvaluator::new(client)
    }

    #[test]
    fn test_parse_ab_response_a() {
        assert_eq!(parse_ab_response("A"), "A");
        assert_eq!(parse_ab_response("  A  "), "A");
        assert_eq!(parse_ab_response("A\n"), "A");
    }

    #[test]
    fn test_parse_ab_response_b() {
        assert_eq!(parse_ab_response("B"), "B");
        assert_eq!(parse_ab_response("  B  "), "B");
    }

    #[test]
    fn test_parse_ab_response_ambiguous_defaults_to_b() {
        assert_eq!(parse_ab_response("maybe"), "B");
        assert_eq!(parse_ab_response(""), "B");
    }

    #[tokio::test]
    async fn test_zero_answers_returns_error() {
        let eval = make_evaluator(vec![]);
        let result = eval.evaluate("q", &[], None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_single_answer_returns_immediately() {
        let eval = make_evaluator(vec![]); // no LLM calls expected
        let result = eval
            .evaluate("q", &["answer one".into()], None)
            .await
            .unwrap();
        assert_eq!(result.selected_index, 0);
        assert_eq!(result.score, 1.0);
    }

    #[tokio::test]
    async fn test_identical_answers_short_circuit() {
        let eval = make_evaluator(vec![]); // no LLM calls expected
        let answers: Vec<String> = vec!["same".into(), "same".into(), "same".into()];
        let result = eval.evaluate("q", &answers, None).await.unwrap();
        assert_eq!(result.selected_index, 0);
        assert_eq!(result.score, 1.0);
    }

    #[tokio::test]
    async fn test_pairwise_majority_wins() {
        // 3 answers → 3 pairs: (0,1), (0,2), (1,2)
        // Mock always returns "A", so:
        //   (0,1) → A wins → vote for 0
        //   (0,2) → A wins → vote for 0
        //   (1,2) → A wins → vote for 1
        // Answer 0 gets 2 votes, answer 1 gets 1, answer 2 gets 0
        let eval = make_evaluator(vec!["A", "A", "A"]);
        let answers: Vec<String> = vec!["first".into(), "second".into(), "third".into()];
        let result = eval.evaluate("q", &answers, None).await.unwrap();
        assert_eq!(result.selected_index, 0);
        assert_eq!(result.score, 1.0); // 2/2 wins for answer 0
    }

    #[tokio::test]
    async fn test_pairwise_b_wins() {
        // 3 pairs, all return "B":
        //   (0,1) → B wins → vote for 1
        //   (0,2) → B wins → vote for 2
        //   (1,2) → B wins → vote for 2
        // Answer 2 gets 2 votes
        let eval = make_evaluator(vec!["B", "B", "B"]);
        let answers: Vec<String> = vec!["first".into(), "second".into(), "third".into()];
        let result = eval.evaluate("q", &answers, None).await.unwrap();
        assert_eq!(result.selected_index, 2);
        assert_eq!(result.score, 1.0); // 2/2 wins
    }

    #[tokio::test]
    async fn test_custom_prompt_is_used() {
        // We can't directly inspect what system prompt was used with this mock,
        // but we verify the evaluator doesn't crash with a custom prompt.
        let eval = make_evaluator(vec!["A", "A", "A"]);
        let answers: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        let result = eval
            .evaluate("q", &answers, Some("Custom eval prompt"))
            .await
            .unwrap();
        assert!(result.score > 0.0);
    }
}
