use crate::config::constants::AGENT_SOURCE_PROMPT;
use oxy_shared::errors::OxyError;

use super::{Document, Metadata, OutputContainer, ReferenceKind};

#[derive(Clone, Debug)]
pub enum RelevantContextGetter {
    Id,
    Content,
}

impl RelevantContextGetter {
    pub fn get(&self, document: &Document) -> String {
        match self {
            RelevantContextGetter::Id => document.id.clone(),
            RelevantContextGetter::Content => document.content.clone(),
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct TargetOutput {
    pub output: String,
    pub task_description: Option<String>,
    pub relevant_contexts: Vec<String>,
    pub references: Vec<ReferenceKind>,
    pub duration_ms: f64,
    pub input_tokens: i32,
    pub output_tokens: i32,
    /// Last successfully executed SQL the agent produced (execution grader).
    pub final_sql: Option<String>,
    /// Number of LLM calls the run made.
    pub llm_rounds: u32,
    /// Wall-time breakdown of the run (agentic eval target only).
    pub timing: Option<super::RunTiming>,
    /// Agent runtime that produced the run (`react` / `fsm`); eval target only.
    pub runtime: Option<String>,
    /// Model the run's LLM calls reported (first `LlmEnd`); eval target only.
    pub model: Option<String>,
}

#[derive(Debug)]
pub struct OutputGetter<'a> {
    pub value: &'a OutputContainer,
    pub relevant_context_getter: &'a RelevantContextGetter,
}

impl TryFrom<OutputGetter<'_>> for TargetOutput {
    type Error = OxyError;

    fn try_from(getter: OutputGetter) -> Result<Self, Self::Error> {
        let (output, task_description, relevant_contexts, references) = match getter.value {
            OutputContainer::Single(output) => {
                let output = output.to_string();
                (output, None, vec![], vec![])
            }
            OutputContainer::Metadata { value } | OutputContainer::Consistency { value, .. } => {
                let Metadata {
                    output,
                    metadata,
                    references,
                } = value;
                let output = output.to_string();
                let task_description = metadata.get(AGENT_SOURCE_PROMPT).cloned();
                let relevant_contexts = references
                    .iter()
                    .filter_map(|r| match r {
                        ReferenceKind::Retrieval(r) => Some(
                            r.documents
                                .iter()
                                .map(|doc| getter.relevant_context_getter.get(doc))
                                .collect::<Vec<_>>(),
                        ),
                        _ => None,
                    })
                    .flatten()
                    .collect::<Vec<_>>();
                (
                    output,
                    task_description,
                    relevant_contexts,
                    references.clone(),
                )
            }
            _ => {
                return Err(OxyError::RuntimeError(format!(
                    "Failed to convert OutputContainer to TargetOutput: {getter:?}"
                )));
            }
        };
        Ok(Self {
            output,
            task_description,
            relevant_contexts,
            references,
            duration_ms: 0.0,
            input_tokens: 0,
            output_tokens: 0,
            final_sql: None,
            llm_rounds: 0,
            timing: None,
            runtime: None,
            model: None,
        })
    }
}
