//! **Specifying** pipeline stage.
//!
//! Owns:
//! - [`build_specify_user_prompt`] — prompt builder
//! - [`AnalyticsSolver::specify_impl`] — core LLM call
//! - [`build_specifying_handler`] — `StateHandler` factory (hybrid routing + fan-out)

use std::sync::Arc;

use agentic_core::{
    HumanInputQuestion, SuspendReason,
    back_target::{BackTarget, RetryContext},
    events::{CoreEvent, EventStream},
    human_input::SuspendedRunData,
    orchestrator::{RunContext, SessionMemory, StateHandler, TransitionResult},
    solver::DomainSolver,
    state::ProblemState,
};

use crate::catalog::{Catalog, JoinPath};
use crate::engine::{EngineError, TranslationContext};
use crate::events::AnalyticsEvent;
use crate::llm::{LlmOutput, ThinkingConfig, ToolLoopConfig};
use crate::schemas::{specify_response_schema, specify_response_schema_legacy};
use crate::semantic::SemanticCatalog;
use crate::tools::{
    execute_clarifying_tool, execute_database_lookup_tool, execute_specifying_tool,
};
use crate::types::{QueryRequestEnvelope, ResultShape, SolutionPayload, SolutionSource};
use crate::{AnalyticsDomain, AnalyticsError, AnalyticsIntent, QuerySpec};

use super::{
    AnalyticsSolver, emit_core, emit_domain, fmt_result_shape, infer_result_shape,
    is_retryable_compile_error,
    prompts::{
        SPECIFY_BASE_PROMPT, SPECIFY_QUERY_REQUEST_PROMPT, specify_query_request_type_addendum,
        specify_type_addendum,
    },
    resuming::{ask_user_tool_def, handle_ask_user},
    strip_json_fences,
};

// LLM response types

#[derive(serde::Deserialize)]
struct SpecifyResponseItem {
    resolved_metrics: Vec<String>,
    #[serde(default)]
    resolved_filters: Vec<String>,
    resolved_tables: Vec<String>,
    join_path: Vec<(String, String, String)>,
    #[serde(default)]
    assumptions: Vec<String>,
}

#[derive(serde::Deserialize)]
struct SpecifyResponseEnvelope {
    specs: Vec<SpecifyResponseItem>,
}

// Prompt builder

pub mod prompts;
use prompts::build_specify_query_request_user_prompt;
pub(crate) use prompts::build_specify_user_prompt;

// Solver impl methods

impl AnalyticsSolver {
    /// Core specify logic; uses the LLM with specifying tools to resolve an
    /// [`AnalyticsIntent`] into one or more [`QuerySpec`]s.
    #[tracing::instrument(
        skip_all,
        fields(
            oxy.name = "analytics.specify",
            oxy.span_type = "analytics",
            solution_source = tracing::field::Empty,
            spec_count = tracing::field::Empty,
        )
    )]
    pub(crate) async fn specify_impl(
        &mut self,
        intent: AnalyticsIntent,
        retry_ctx: Option<&RetryContext>,
    ) -> Result<Vec<QuerySpec>, (AnalyticsError, BackTarget<AnalyticsDomain>)> {
        // Short-circuit: the LLM already selected an automation.
        // Skip LLM resolution and hand the path straight to Executing.
        // (SQL files are intercepted earlier in `build_specifying_handler`'s
        // step-0 short-circuit and never reach this method.)
        if let Some(ref file_path) = intent.selected_automation.clone() {
            return Ok(vec![QuerySpec {
                intent,
                resolved_metrics: vec![],
                resolved_filters: vec![],
                resolved_tables: vec![],
                join_path: vec![],
                expected_result_shape: ResultShape::Table { columns: vec![] },
                assumptions: vec![],
                solution_source: SolutionSource::Automation {
                    file_path: file_path.clone(),
                },
                precomputed: None,
                context: None,
                connector_name: self.default_connector.clone(),
                query_request_item: None,
                query_request: None,
                compile_error: None,
            }]);
        }

        let user_prompt = build_specify_user_prompt(&intent, &self.catalog, retry_ctx);

        // On resume, rebuild the full message history from the persisted
        // conversation snapshot and append the appropriate continuation.
        let mut resume_max_tokens_override: Option<u32> = None;
        let mut resume_extra_rounds: u32 = 0;
        let initial: crate::llm::InitialMessages = if let Some(resume) = self.resume_data.take() {
            let prior: Vec<serde_json::Value> = resume.data.stage_data["conversation_history"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            match resume.data.stage_data["suspension_type"].as_str() {
                Some("max_tokens") => {
                    resume_max_tokens_override = resume.data.stage_data["max_tokens_override"]
                        .as_u64()
                        .map(|v| v as u32);
                    crate::llm::InitialMessages::Messages(
                        crate::llm::LlmClient::build_continue_messages(&prior),
                    )
                }
                Some("max_tool_rounds") => {
                    resume_extra_rounds =
                        resume.data.stage_data["extra_rounds"].as_u64().unwrap_or(0) as u32;
                    crate::llm::InitialMessages::Messages(
                        crate::llm::LlmClient::build_continue_messages(&prior),
                    )
                }
                _ => {
                    // ask_user or legacy pre-ground suspension.
                    if !prior.is_empty() {
                        // In-ground ask_user: continue the in-progress tool loop.
                        let msgs = self.client.build_resume_messages(
                            &prior,
                            &resume.data.question,
                            &resume.data.suggestions,
                            &resume.answer,
                        );
                        crate::llm::InitialMessages::Messages(msgs)
                    } else {
                        // Pre-ground ambiguity: no conversation history yet.
                        // Calling build_resume_messages with an empty prior would
                        // produce a tool_result with no matching tool_use block,
                        // which the Anthropic API rejects. Start fresh instead.
                        crate::llm::InitialMessages::User(format!(
                            "{user_prompt}\n\nUser answered the clarifying question \"{q}\": {a}",
                            q = resume.data.question,
                            a = resume.answer,
                        ))
                    }
                }
            }
        } else {
            crate::llm::InitialMessages::User(user_prompt)
        };

        let tools = self.tools_for_state_specifying();
        let type_addendum = specify_type_addendum(&intent.question_type);
        let specify_prompt = format!("{SPECIFY_BASE_PROMPT}{type_addendum}");
        let default_dialect = self
            .connectors
            .get(&self.default_connector)
            .map(|c| c.dialect().as_str());
        let system_prompt =
            self.build_system_prompt("specifying", &specify_prompt, default_dialect);
        let thinking = self.thinking_for_state("specifying", ThinkingConfig::Disabled);
        let max_rounds = self.max_tool_rounds_for_state("specifying", 5) + resume_extra_rounds;
        let catalog = Arc::clone(&self.catalog);
        let human_input = Arc::clone(&self.human_input);
        let connectors = self.connectors.clone();
        let default_connector = self.default_connector.clone();
        let schema_cache = Arc::clone(&self.schema_cache);
        let intent_for_stage = intent.clone();
        let output = self
            .client_for_state("specifying")
            .run_with_tools(
                &system_prompt,
                initial,
                &tools,
                |name: String, params| {
                    let catalog = Arc::clone(&catalog);
                    let human_input = Arc::clone(&human_input);
                    let connectors = connectors.clone();
                    let default_connector = default_connector.clone();
                    let schema_cache = Arc::clone(&schema_cache);
                    Box::pin(async move {
                        // ask_user intercepted before generic dispatcher — see resuming.rs.
                        if name == "ask_user" {
                            handle_ask_user(&params, human_input.as_ref())
                                .map(|v| Box::new(v) as Box<dyn agentic_core::tools::ToolOutput>)
                        } else if name == "search_catalog" {
                            execute_clarifying_tool(&name, params, &*catalog)
                                .map(|v| Box::new(v) as Box<dyn agentic_core::tools::ToolOutput>)
                        } else if name == "list_tables" || name == "describe_table" {
                            execute_database_lookup_tool(
                                &name,
                                params,
                                &connectors,
                                &default_connector,
                                &schema_cache,
                            )
                            .await
                            .map(|v| Box::new(v) as Box<dyn agentic_core::tools::ToolOutput>)
                        } else {
                            execute_specifying_tool(
                                &name,
                                params,
                                &*catalog,
                                &connectors,
                                &default_connector,
                            )
                            .await
                            .map(|v| Box::new(v) as Box<dyn agentic_core::tools::ToolOutput>)
                        }
                    })
                },
                &self.event_tx,
                ToolLoopConfig {
                    max_tool_rounds: max_rounds,
                    state: "specifying".into(),
                    thinking,
                    response_schema: Some(specify_response_schema_legacy()),
                    max_tokens_override: resume_max_tokens_override.or(self.max_tokens),
                    sub_spec_index: None,
                    system_date_hint: Some(self.system_hint()),
                },
            )
            .await
            .map_err(|e| self.handle_llm_error(e, &intent_for_stage))?;

        let envelope = parse_llm_response(output, &intent)?;

        let specs: Result<Vec<QuerySpec>, _> = envelope
            .specs
            .into_iter()
            .map(|resp| {
                let expected_result_shape =
                    infer_result_shape(&intent.dimensions, &resp.resolved_metrics);
                let connector_name = resolve_connector(
                    &resp.resolved_tables,
                    &self.catalog,
                    &self.default_connector,
                );
                Ok(QuerySpec {
                    intent: intent.clone(),
                    resolved_metrics: resp.resolved_metrics,
                    resolved_filters: resp.resolved_filters,
                    resolved_tables: resp.resolved_tables,
                    join_path: resp.join_path,
                    expected_result_shape,
                    assumptions: resp.assumptions,
                    solution_source: Default::default(),
                    precomputed: None,
                    context: None,
                    connector_name,
                    query_request_item: None,
                    query_request: None,
                    compile_error: None,
                })
            })
            .collect();
        if let Ok(ref s) = specs {
            let span = tracing::Span::current();
            span.record("spec_count", s.len());
            if let Some(first) = s.first() {
                span.record("solution_source", format!("{:?}", first.solution_source));
            }
        }
        specs
    }

    /// Airlayer-native specify: runs the LLM with the query-request prompt and
    /// schema, producing a [`QueryRequestEnvelope`] instead of SQL fragments.
    #[tracing::instrument(
        skip_all,
        fields(
            oxy.name = "analytics.specify_query_request",
            oxy.span_type = "analytics",
        )
    )]
    pub(crate) async fn specify_impl_query_request(
        &mut self,
        intent: AnalyticsIntent,
        retry_ctx: Option<&RetryContext>,
    ) -> Result<QueryRequestEnvelope, (AnalyticsError, BackTarget<AnalyticsDomain>)> {
        // Short-circuit: automation already selected.
        //
        // In the live pipeline, `build_specifying_handler`'s step-0 short-
        // circuit returns before this method is called when
        // `selected_automation` is set, so this arm is reachable only via
        // direct unit-test calls. SQL files never reach here — they are
        // always handled by step-0 in the handler.
        //
        // The empty `QueryRequestItem` returned here would fail the
        // `envelope.specs.is_empty()` guard upstream if it were ever fed
        // back into the LLM-driven path; producing a single empty item
        // keeps the contract of "non-empty specs" intact for callers that
        // bypass the handler.
        if intent.selected_automation.is_some() {
            return Ok(QueryRequestEnvelope {
                specs: vec![crate::types::QueryRequestItem {
                    measures: vec![],
                    dimensions: vec![],
                    filters: vec![],
                    time_dimensions: vec![],
                    order: vec![],
                    limit: None,
                    assumptions: vec![],
                }],
            });
        }

        let user_prompt =
            build_specify_query_request_user_prompt(&intent, &self.catalog, retry_ctx);

        let mut resume_max_tokens_override: Option<u32> = None;
        let mut resume_extra_rounds: u32 = 0;
        let initial: crate::llm::InitialMessages = if let Some(resume) = self.resume_data.take() {
            let prior: Vec<serde_json::Value> = resume.data.stage_data["conversation_history"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            match resume.data.stage_data["suspension_type"].as_str() {
                Some("max_tokens") => {
                    resume_max_tokens_override = resume.data.stage_data["max_tokens_override"]
                        .as_u64()
                        .map(|v| v as u32);
                    crate::llm::InitialMessages::Messages(
                        crate::llm::LlmClient::build_continue_messages(&prior),
                    )
                }
                Some("max_tool_rounds") => {
                    resume_extra_rounds =
                        resume.data.stage_data["extra_rounds"].as_u64().unwrap_or(0) as u32;
                    crate::llm::InitialMessages::Messages(
                        crate::llm::LlmClient::build_continue_messages(&prior),
                    )
                }
                _ => {
                    // ask_user or legacy pre-ground suspension.
                    if !prior.is_empty() {
                        // In-ground ask_user: continue the in-progress tool loop.
                        let msgs = self.client.build_resume_messages(
                            &prior,
                            &resume.data.question,
                            &resume.data.suggestions,
                            &resume.answer,
                        );
                        crate::llm::InitialMessages::Messages(msgs)
                    } else {
                        // Pre-ground ambiguity: no conversation history yet.
                        // Calling build_resume_messages with an empty prior would
                        // produce a tool_result with no matching tool_use block,
                        // which the Anthropic API rejects. Start fresh instead.
                        crate::llm::InitialMessages::User(format!(
                            "{user_prompt}\n\nUser answered the clarifying question \"{q}\": {a}",
                            q = resume.data.question,
                            a = resume.answer,
                        ))
                    }
                }
            }
        } else {
            crate::llm::InitialMessages::User(user_prompt)
        };

        let tools = self.tools_for_state_specifying();
        let type_addendum = specify_query_request_type_addendum(&intent.question_type);
        let specify_prompt = format!("{SPECIFY_QUERY_REQUEST_PROMPT}{type_addendum}");
        let default_dialect = self
            .connectors
            .get(&self.default_connector)
            .map(|c| c.dialect().as_str());
        let system_prompt =
            self.build_system_prompt("specifying", &specify_prompt, default_dialect);
        let thinking = self.thinking_for_state("specifying", ThinkingConfig::Disabled);
        let max_rounds = self.max_tool_rounds_for_state("specifying", 5) + resume_extra_rounds;
        let catalog = Arc::clone(&self.catalog);
        let human_input = Arc::clone(&self.human_input);
        let connectors = self.connectors.clone();
        let default_connector = self.default_connector.clone();
        let schema_cache = Arc::clone(&self.schema_cache);
        let intent_for_stage = intent.clone();
        let output = self
            .client_for_state("specifying")
            .run_with_tools(
                &system_prompt,
                initial,
                &tools,
                |name: String, params| {
                    let catalog = Arc::clone(&catalog);
                    let human_input = Arc::clone(&human_input);
                    let connectors = connectors.clone();
                    let default_connector = default_connector.clone();
                    let schema_cache = Arc::clone(&schema_cache);
                    Box::pin(async move {
                        if name == "ask_user" {
                            handle_ask_user(&params, human_input.as_ref())
                                .map(|v| Box::new(v) as Box<dyn agentic_core::tools::ToolOutput>)
                        } else if name == "search_catalog" {
                            execute_clarifying_tool(&name, params, &*catalog)
                                .map(|v| Box::new(v) as Box<dyn agentic_core::tools::ToolOutput>)
                        } else if name == "list_tables" || name == "describe_table" {
                            execute_database_lookup_tool(
                                &name,
                                params,
                                &connectors,
                                &default_connector,
                                &schema_cache,
                            )
                            .await
                            .map(|v| Box::new(v) as Box<dyn agentic_core::tools::ToolOutput>)
                        } else {
                            execute_specifying_tool(
                                &name,
                                params,
                                &*catalog,
                                &connectors,
                                &default_connector,
                            )
                            .await
                            .map(|v| Box::new(v) as Box<dyn agentic_core::tools::ToolOutput>)
                        }
                    })
                },
                &self.event_tx,
                ToolLoopConfig {
                    max_tool_rounds: max_rounds,
                    state: "specifying".into(),
                    thinking,
                    response_schema: Some(specify_response_schema()),
                    max_tokens_override: resume_max_tokens_override.or(self.max_tokens),
                    sub_spec_index: None,
                    system_date_hint: Some(self.system_hint()),
                },
            )
            .await
            .map_err(|e| self.handle_llm_error(e, &intent_for_stage))?;

        parse_query_request_response(output, &intent)
    }

    /// Returns the tool list for the specifying state.
    ///
    /// `ask_user` is listed so the LLM can invoke it, but it is intercepted
    /// inside the tool loop before `execute_tool` is reached (see resuming.rs).
    pub(super) fn tools_for_state_specifying(&self) -> Vec<agentic_core::tools::ToolDef> {
        let has_semantic = !self.catalog.is_empty();
        let has_metric_tree = self.metric_tree_runner.is_some();
        let mut tools = crate::tools::specifying_tools(has_semantic, has_metric_tree);
        tools.push(ask_user_tool_def());
        tools
    }

    /// Converts any [`crate::llm::LlmError`] from the specifying LLM call into
    /// the `(AnalyticsError, BackTarget)` pair expected by the solver protocol.
    ///
    /// Stores suspension data for the three resumable variants; maps the
    /// catch-all to a plain `Specify` back-target for an immediate retry.
    fn handle_llm_error(
        &mut self,
        error: crate::llm::LlmError,
        intent: &AnalyticsIntent,
    ) -> (AnalyticsError, BackTarget<AnalyticsDomain>) {
        let intent_value = serde_json::to_value(intent).unwrap_or_default();
        match error {
            crate::llm::LlmError::Suspended {
                prompt,
                suggestions,
                prior_messages,
            } => {
                self.store_suspension_data(SuspendedRunData {
                    from_state: "specifying".to_string(),
                    original_input: intent.raw_question.clone(),
                    trace_id: String::new(),
                    stage_data: serde_json::json!({
                        "intent": intent_value,
                        "conversation_history": prior_messages,
                    }),
                    question: prompt.clone(),
                    suggestions: suggestions.clone(),
                });
                (
                    AnalyticsError::NeedsUserInput {
                        prompt: prompt.clone(),
                    },
                    BackTarget::Suspend {
                        reason: SuspendReason::HumanInput {
                            questions: vec![HumanInputQuestion {
                                prompt,
                                suggestions,
                            }],
                        },
                    },
                )
            }
            crate::llm::LlmError::MaxTokensReached {
                current_max_tokens,
                prior_messages,
                ..
            } => {
                let doubled = current_max_tokens.saturating_mul(2);
                let prompt = format!(
                    "The model ran out of token budget ({current_max_tokens} tokens). \
                     Continue with double the budget ({doubled} tokens)?"
                );
                self.store_suspension_data(SuspendedRunData {
                    from_state: "specifying".to_string(),
                    original_input: intent.raw_question.clone(),
                    trace_id: String::new(),
                    stage_data: serde_json::json!({
                        "intent": intent_value,
                        "conversation_history": prior_messages,
                        "suspension_type": "max_tokens",
                        "max_tokens_override": doubled,
                    }),
                    question: prompt.clone(),
                    suggestions: vec!["Continue with double budget".to_string()],
                });
                (
                    AnalyticsError::NeedsUserInput {
                        prompt: prompt.clone(),
                    },
                    BackTarget::Suspend {
                        reason: SuspendReason::HumanInput {
                            questions: vec![HumanInputQuestion {
                                prompt,
                                suggestions: vec!["Continue with double budget".to_string()],
                            }],
                        },
                    },
                )
            }
            crate::llm::LlmError::MaxToolRoundsReached {
                rounds,
                prior_messages,
            } => {
                let prompt = format!(
                    "The agent used all {rounds} allotted tool rounds. \
                     Continue with more rounds?"
                );
                self.store_suspension_data(SuspendedRunData {
                    from_state: "specifying".to_string(),
                    original_input: intent.raw_question.clone(),
                    trace_id: String::new(),
                    stage_data: serde_json::json!({
                        "intent": intent_value,
                        "conversation_history": prior_messages,
                        "suspension_type": "max_tool_rounds",
                        "extra_rounds": rounds,
                    }),
                    question: prompt.clone(),
                    suggestions: vec!["Continue".to_string()],
                });
                (
                    AnalyticsError::NeedsUserInput {
                        prompt: prompt.clone(),
                    },
                    BackTarget::Suspend {
                        reason: SuspendReason::HumanInput {
                            questions: vec![HumanInputQuestion {
                                prompt,
                                suggestions: vec!["Continue".to_string()],
                            }],
                        },
                    },
                )
            }
            e => (
                AnalyticsError::NeedsUserInput {
                    prompt: format!("LLM call failed during specify: {e}"),
                },
                BackTarget::Specify(intent.clone(), Default::default()),
            ),
        }
    }

    /// VendorEngine resolution path.
    ///
    /// Attempts translation via the configured vendor engine.  Returns
    /// `Some(result)` when the engine produces a spec, or `None` to fall
    /// through to the semantic-model / LLM paths.
    async fn specifying_try_vendor_engine(
        &mut self,
        intent: &AnalyticsIntent,
    ) -> Option<TransitionResult<AnalyticsDomain>> {
        let engine = self.engine.clone()?;

        let known = intent
            .metrics
            .iter()
            .all(|m| self.catalog.get_metric_definition(m).is_some());
        if !known {
            return None;
        }

        let translation_ctx = build_translation_context(&self.catalog, intent);
        let vq = match engine.translate(&translation_ctx, intent) {
            Ok(vq) => vq,
            // Metric exists but cannot be expressed in vendor format — fall through.
            Err(EngineError::TranslationFailed(_)) | Err(_) => return None,
        };

        let vendor_name = engine.vendor_name().to_owned();
        let resolved_tables: Vec<String> = translation_ctx
            .metrics
            .iter()
            .map(|m| m.table.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        let connector_name =
            resolve_connector(&resolved_tables, &self.catalog, &self.default_connector);
        let spec = QuerySpec {
            intent: intent.clone(),
            resolved_metrics: translation_ctx
                .metrics
                .iter()
                .map(|m| format!("{}.{}", m.table, m.name))
                .collect(),
            resolved_filters: intent.filters.clone(),
            join_path: extract_join_paths(&translation_ctx.join_paths),
            resolved_tables,
            expected_result_shape: infer_result_shape(
                &intent.dimensions,
                &translation_ctx
                    .metrics
                    .iter()
                    .map(|m| m.name.clone())
                    .collect::<Vec<_>>(),
            ),
            assumptions: vec![],
            solution_source: SolutionSource::VendorEngine(vendor_name.clone()),
            precomputed: Some(SolutionPayload::Vendor(vq)),
            context: None,
            connector_name,
            query_request_item: None,
            query_request: None,
            compile_error: None,
        };
        emit_spec_resolved(
            &self.event_tx,
            &spec,
            &format!("VendorEngine({vendor_name})"),
        )
        .await;
        // VendorEngine produces a precomputed payload — go straight to Executing.
        Some(TransitionResult::ok(ProblemState::Executing(
            crate::AnalyticsSolution {
                payload: spec
                    .precomputed
                    .clone()
                    .unwrap_or(SolutionPayload::Sql(String::new())),
                solution_source: spec.solution_source.clone(),
                connector_name: spec.connector_name.clone(),
                semantic_query: None,
            },
        )))
    }

    /// Primary semantic-model path: LLM → QueryRequest → airlayer compile.
    ///
    /// 1. LLM produces a structured `QueryRequest` (view.member references)
    /// 2. Try `engine.compile_query` on each request item
    /// 3. On success: precomputed SQL, Solving skips to Executing
    /// 4. On retryable error: retry Specify with error hint
    /// 5. On non-retryable error: forward to Solving with QueryRequest +
    ///    compile error (Solving handles `translate_to_raw_context` + LLM fallback)
    async fn specifying_primary(
        &mut self,
        intent: AnalyticsIntent,
        retry_ctx: Option<&RetryContext>,
    ) -> TransitionResult<AnalyticsDomain> {
        let envelope = match self
            .specify_impl_query_request(intent.clone(), retry_ctx)
            .await
        {
            Ok(env) => env,
            Err((err, back)) => {
                return TransitionResult::diagnosing(ProblemState::Diagnosing { error: err, back });
            }
        };

        let mut specs: Vec<QuerySpec> = Vec::with_capacity(envelope.specs.len());

        // The same gate `propose_semantic_query` applies, BEFORE the loop and
        // over every item.
        //
        // The first version returned `NeedsUserInput` from the parser, which
        // `diagnosing` routes to Fatal: a one-value `date_range` — a formatting
        // slip on the path that carries most of the traffic — killed the run and
        // put the validator's own text in front of the user. Routing it like a
        // compile failure gets the retry that already exists: the reason reaches
        // the model through the hint, the offending item through `spec_hint`,
        // and `max_iterations` bounds the loop.
        //
        // **The envelope is re-specified as a whole.** Checking every item up
        // front does not keep the healthy ones — it decides WHICH item the hint
        // names, and reports every problem at once instead of the first. Doing
        // it before the loop also means a bad item cannot be preceded by
        // siblings that were compiled through airlayer, emitted a tool-call pair
        // and then thrown away, so "which item" no longer depends on compile
        // order.
        let problems: Vec<(usize, Vec<String>)> = envelope
            .specs
            .iter()
            .enumerate()
            .filter_map(|(i, item)| {
                let p = crate::types::query_request::query_problems(item);
                (!p.is_empty()).then_some((i, p))
            })
            .collect();
        if let Some((first_bad, _)) = problems.first() {
            // Names which spec the hint below carries. `format_spec_hint_section`
            // frames it as "Previous query (use as a starting point)", so with
            // two bad specs the model would otherwise read spec 1's shape beside
            // complaints about spec 3.
            let reason = format!(
                "{}. The query below is spec {}. {}",
                problems
                    .iter()
                    .map(|(i, p)| format!("spec {}: {}", i + 1, p.join("; ")))
                    .collect::<Vec<_>>()
                    .join(" | "),
                first_bad + 1,
                crate::types::query_request::QUERY_REJECTION_HINT
            );
            let bad_item = &envelope.specs[*first_bad];
            // A ToolCall before the ToolResult, like every other pair in this
            // loop. `analyticsSteps.ts` attaches a `tool_result` to the last
            // STREAMING artifact, so a result with no matching call is stamped
            // onto whichever artifact happens to be open — or dropped. Either
            // way the one visible trace of "the model proposed something invalid
            // and is retrying" does not render on the pill a reader would click.
            let call_input = serde_json::to_string(&serde_json::json!({
                "measures": bad_item.measures,
                "dimensions": bad_item.dimensions,
                "filters": bad_item.filters,
                "time_dimensions": bad_item.time_dimensions,
            }))
            .unwrap_or_default();
            emit_core(
                &self.event_tx,
                CoreEvent::ToolCall {
                    name: "compile_semantic_query".to_string(),
                    input: call_input,
                    llm_duration_ms: 0,
                    sub_spec_index: None,
                },
            )
            .await;
            emit_core(
                &self.event_tx,
                CoreEvent::ToolResult {
                    name: "compile_semantic_query".to_string(),
                    output: serde_json::to_string(&serde_json::json!({
                        "success": false,
                        "error": reason,
                    }))
                    .unwrap_or_default(),
                    duration_ms: 0,
                    is_error: false,
                    sub_spec_index: None,
                },
            )
            .await;
            let hint = retry_ctx
                .cloned()
                .unwrap_or_default()
                .advance(reason.clone());
            let mut retry_intent = intent.clone();
            // Without this the retry rebuilds the prompt from the intent alone
            // and re-sends an identical request, burning iterations.
            retry_intent.spec_hint = Some(bad_item.clone());
            return TransitionResult::diagnosing(ProblemState::Diagnosing {
                error: AnalyticsError::AirlayerCompileError {
                    error_message: reason,
                },
                back: BackTarget::Specify(retry_intent, hint),
            });
        }

        for item in &envelope.specs {
            let query_request = item.to_query_request();

            // Try compiling via airlayer.
            let compile_input = serde_json::to_string(&serde_json::json!({
                "measures": item.measures,
                "dimensions": item.dimensions,
                "filters": item.filters,
                "time_dimensions": item.time_dimensions,
            }))
            .unwrap_or_default();
            emit_core(
                &self.event_tx,
                CoreEvent::ToolCall {
                    name: "compile_semantic_query".to_string(),
                    input: compile_input,
                    llm_duration_ms: 0,
                    sub_spec_index: None,
                },
            )
            .await;
            let compile_start = std::time::Instant::now();
            match self.catalog.engine().compile_query(&query_request) {
                Ok(result) => {
                    let sql = crate::airlayer_compat::substitute_params(
                        &crate::airlayer_compat::request_dialect(
                            self.catalog.engine(),
                            &query_request,
                        ),
                        &result.sql,
                        &result.params,
                    );
                    let compile_duration_ms = compile_start.elapsed().as_millis() as u64;
                    emit_core(
                        &self.event_tx,
                        CoreEvent::ToolResult {
                            name: "compile_semantic_query".to_string(),
                            output: serde_json::to_string(&serde_json::json!({
                                "success": true,
                                "sql": sql,
                            }))
                            .unwrap_or_default(),
                            duration_ms: compile_duration_ms,
                            is_error: false,
                            sub_spec_index: None,
                        },
                    )
                    .await;
                    tracing::info!(
                        "[specifying_primary] compile SUCCESS: {}",
                        &sql[..sql.len().min(200)]
                    );

                    // Use semantic measure names (view.member) for
                    // resolved_metrics so MetricResolvesRule recognizes them
                    // via `metric_resolves_in_semantic`.  The translated SQL
                    // expressions would fail validation.
                    let translation = self.catalog.translate_to_raw_context(&query_request, "");
                    let connector_name = resolve_connector(
                        &translation.resolved_tables,
                        &self.catalog,
                        &self.default_connector,
                    );

                    let payload = self.build_semantic_payload(sql, &query_request);

                    specs.push(QuerySpec {
                        intent: intent.clone(),
                        resolved_metrics: query_request.measures.clone(),
                        resolved_filters: translation.resolved_filters,
                        resolved_tables: translation.resolved_tables,
                        join_path: translation.join_path,
                        expected_result_shape: infer_result_shape(
                            &intent.dimensions,
                            &item.measures,
                        ),
                        assumptions: item.assumptions.clone(),
                        solution_source: SolutionSource::SemanticLayer,
                        precomputed: Some(payload),
                        context: None,
                        connector_name,
                        query_request_item: Some(item.clone()),
                        query_request: Some(query_request),
                        compile_error: None,
                    });
                }
                Err(e) => {
                    let compile_duration_ms = compile_start.elapsed().as_millis() as u64;
                    let retryable = is_retryable_compile_error(&e);
                    tracing::info!(
                        "[specifying_primary] compile FAILED: {e} (retryable={retryable})"
                    );

                    // "No valid join tree found" — the requested combination of
                    // dimensions/measures cannot be joined in the semantic graph.
                    // Route back to Specifying so the LLM can reformulate the query.
                    if e.to_string().contains("No valid join tree") {
                        emit_core(
                            &self.event_tx,
                            CoreEvent::ToolResult {
                                name: "compile_semantic_query".to_string(),
                                output: serde_json::to_string(&serde_json::json!({
                                    "success": false,
                                    "error": e.to_string(),
                                }))
                                .unwrap_or_default(),
                                duration_ms: compile_duration_ms,
                                is_error: false,
                                sub_spec_index: None,
                            },
                        )
                        .await;
                        let hint = retry_ctx.cloned().unwrap_or_default().advance(format!(
                            "airlayer compilation failed: {e}. \
                             The join graph cannot connect the requested dimensions \
                             and measures. Reformulate the query using members that \
                             can be joined together (e.g. pick a single base view \
                             or use only dimensions/measures from the same view)."
                        ));
                        return TransitionResult::diagnosing(ProblemState::Diagnosing {
                            error: AnalyticsError::SyntaxError {
                                query: String::new(),
                                message: format!("Query compilation error: {e}"),
                            },
                            back: BackTarget::Specify(intent, hint),
                        });
                    }

                    if retryable {
                        // QueryError with "not found" — the LLM picked a wrong
                        // member name.  Route back to Specify with the error so
                        // the LLM can correct it.
                        emit_core(
                            &self.event_tx,
                            CoreEvent::ToolResult {
                                name: "compile_semantic_query".to_string(),
                                output: serde_json::to_string(&serde_json::json!({
                                    "success": false,
                                    "error": e.to_string(),
                                }))
                                .unwrap_or_default(),
                                duration_ms: compile_duration_ms,
                                is_error: false,
                                sub_spec_index: None,
                            },
                        )
                        .await;
                        let hint = retry_ctx.cloned().unwrap_or_default().advance(format!(
                            "airlayer compilation failed: {e}. \
                                 Fix the member names in the query request and try again."
                        ));
                        return TransitionResult::diagnosing(ProblemState::Diagnosing {
                            error: AnalyticsError::AirlayerCompileError {
                                error_message: format!("Query compilation error: {e}"),
                            },
                            back: BackTarget::Specify(intent, hint),
                        });
                    }

                    // Non-retryable (JoinError, SqlGenerationError, SchemaError,
                    // cross-dialect) — forward to Solving with the QueryRequest
                    // and compile error.  Solving will call
                    // translate_to_raw_context and fall back to LLM SQL generation.
                    emit_core(
                        &self.event_tx,
                        CoreEvent::ToolResult {
                            name: "compile_semantic_query".to_string(),
                            output: serde_json::to_string(&serde_json::json!({
                                "success": false,
                                "error": e.to_string(),
                            }))
                            .unwrap_or_default(),
                            duration_ms: compile_duration_ms,
                            is_error: false,
                            sub_spec_index: None,
                        },
                    )
                    .await;
                    let connector_name =
                        resolve_connector(&[], &self.catalog, &self.default_connector);

                    specs.push(QuerySpec {
                        intent: intent.clone(),
                        resolved_metrics: vec![],
                        resolved_filters: vec![],
                        resolved_tables: vec![],
                        join_path: vec![],
                        expected_result_shape: infer_result_shape(
                            &intent.dimensions,
                            &item.measures,
                        ),
                        assumptions: item.assumptions.clone(),
                        solution_source: SolutionSource::LlmWithSemanticContext,
                        precomputed: None,
                        context: None,
                        connector_name,
                        query_request_item: Some(item.clone()),
                        query_request: Some(query_request),
                        compile_error: Some(e.to_string()),
                    });
                }
            }
        }

        for spec in &specs {
            if let Err(err) = self.validator.validate_specified(spec, &self.catalog) {
                return diagnose_validation_error(&self.event_tx, err, spec, retry_ctx, &intent)
                    .await;
            }
        }

        if specs.len() == 1 {
            let spec = specs.into_iter().next().unwrap();
            let source_label = spec_source_label(&spec.solution_source);
            emit_spec_resolved(&self.event_tx, &spec, &source_label).await;
            // Semantic compile success: precomputed SQL → Executing.
            // Compile failure: call solve_impl inline (Mode 2 raw SQL) → Executing.
            self.spec_to_executing(spec, retry_ctx).await
        } else {
            for spec in &specs {
                emit_spec_resolved(
                    &self.event_tx,
                    spec,
                    &spec_source_label(&spec.solution_source),
                )
                .await;
            }
            TransitionResult::pending_fan_out(specs, ProblemState::Specifying(intent))
        }
    }

    /// Legacy LLM fallback (no semantic model).
    ///
    /// Uses the old SQL-fragment specify_impl path.
    async fn specifying_try_llm_fallback_legacy(
        &mut self,
        intent: AnalyticsIntent,
        retry_ctx: Option<&RetryContext>,
    ) -> TransitionResult<AnalyticsDomain> {
        let query_ctx = self.catalog.get_context(&intent);
        let specs = match self.specify_impl(intent.clone(), retry_ctx).await {
            Ok(specs) => specs,
            Err((err, back)) => {
                return TransitionResult::diagnosing(ProblemState::Diagnosing { error: err, back });
            }
        };

        let specs: Vec<QuerySpec> = specs
            .into_iter()
            .map(|mut spec| {
                if !matches!(
                    spec.solution_source,
                    SolutionSource::Automation { .. } | SolutionSource::SqlFile { .. }
                ) {
                    spec.solution_source = SolutionSource::LlmWithSemanticContext;
                }
                spec.context = Some(query_ctx.clone());
                spec
            })
            .collect();

        for spec in &specs {
            if let Err(err) = self.validator.validate_specified(spec, &self.catalog) {
                return diagnose_validation_error(&self.event_tx, err, spec, retry_ctx, &intent)
                    .await;
            }
        }

        if specs.len() == 1 {
            let spec = specs.into_iter().next().unwrap();
            let source_label = spec_source_label(&spec.solution_source);
            emit_spec_resolved(&self.event_tx, &spec, &source_label).await;
            self.spec_to_executing(spec, retry_ctx).await
        } else {
            for spec in &specs {
                emit_spec_resolved(
                    &self.event_tx,
                    spec,
                    &spec_source_label(&spec.solution_source),
                )
                .await;
            }
            TransitionResult::pending_fan_out(specs, ProblemState::Specifying(intent))
        }
    }

    /// Convert a resolved [`QuerySpec`] into an [`Executing`] transition.
    ///
    /// - If the spec has precomputed SQL (semantic compile / vendor engine) →
    ///   transition directly to Executing.
    /// - If the spec needs LLM SQL generation (compile failed) → translate
    ///   the semantic context to raw schema, call `solve_impl` inline (Mode 2),
    ///   then transition to Executing.
    async fn spec_to_executing(
        &mut self,
        mut spec: QuerySpec,
        retry_ctx: Option<&RetryContext>,
    ) -> TransitionResult<AnalyticsDomain> {
        // Fast path: precomputed SQL available (semantic compile / vendor engine).
        if let Some(payload) = spec.precomputed.clone() {
            let semantic_query =
                matches!(spec.solution_source, crate::SolutionSource::SemanticLayer)
                    .then(|| spec.query_request_item.clone())
                    .flatten();
            return TransitionResult::ok(ProblemState::Executing(crate::AnalyticsSolution {
                payload,
                solution_source: spec.solution_source.clone(),
                connector_name: spec.connector_name.clone(),
                semantic_query,
            }));
        }

        // Automation path: delegate to Executing (coordinator handles it).
        // SqlFile has precomputed SQL and is handled by the fast path above.
        if matches!(spec.solution_source, SolutionSource::Automation { .. }) {
            return TransitionResult::ok(ProblemState::Executing(crate::AnalyticsSolution {
                payload: SolutionPayload::Sql(String::new()),
                solution_source: spec.solution_source.clone(),
                connector_name: spec.connector_name.clone(),
                semantic_query: None,
            }));
        }

        // Mode 2: LLM SQL generation.
        // If we have a QueryRequest from a failed semantic compile, translate it
        // to raw schema context so the LLM sees table.column references, not
        // view.member paths.
        if let Some(ref qr) = spec.query_request {
            // Try re-compile once more (may succeed now).
            match self.catalog.engine().compile_query(qr) {
                Ok(result) => {
                    let sql = crate::airlayer_compat::substitute_params(
                        &crate::airlayer_compat::request_dialect(self.catalog.engine(), qr),
                        &result.sql,
                        &result.params,
                    );
                    tracing::info!(
                        "[spec_to_executing] re-compile SUCCESS: {}",
                        &sql[..sql.len().min(200)]
                    );
                    emit_domain(
                        &self.event_tx,
                        AnalyticsEvent::QueryGenerated {
                            sql: sql.clone(),
                            sub_spec_index: None,
                        },
                    )
                    .await;
                    return TransitionResult::ok(ProblemState::Executing(
                        crate::AnalyticsSolution {
                            payload: SolutionPayload::Sql(sql),
                            solution_source: SolutionSource::SemanticLayer,
                            connector_name: spec.connector_name.clone(),
                            semantic_query: spec.query_request_item.clone(),
                        },
                    ));
                }
                Err(e) => {
                    tracing::info!(
                        "[spec_to_executing] re-compile FAILED: {e}, falling back to LLM"
                    );
                    let translation = self.catalog.translate_to_raw_context(qr, &e.to_string());
                    spec.context = Some(translation.context);
                    spec.resolved_metrics = translation.resolved_metrics;
                    spec.resolved_tables = translation.resolved_tables;
                    spec.resolved_filters = translation.resolved_filters;
                    spec.join_path = translation.join_path;
                }
            }
        }

        // LLM SQL generation (Mode 2 raw SQL path).
        match self.solve_impl(spec, retry_ctx).await {
            Ok(solution) => TransitionResult::ok(ProblemState::Executing(solution)),
            Err((err, back)) => {
                TransitionResult::diagnosing(ProblemState::Diagnosing { error: err, back })
            }
        }
    }
}

// Translation context builder

/// Build a [`TranslationContext`] from catalog trait calls for a given intent.
///
/// Called by the Specifying handler before attempting vendor engine translation.
/// Works with any `Catalog` implementation via trait dispatch.
fn build_translation_context(
    catalog: &SemanticCatalog,
    intent: &AnalyticsIntent,
) -> TranslationContext {
    use std::collections::HashSet;

    // Metric definitions for every metric named in the intent.
    let metrics: Vec<_> = intent
        .metrics
        .iter()
        .filter_map(|m| catalog.get_metric_definition(m))
        .collect();

    // Dimension summaries: collect valid dims for each metric, keep those
    // named in the intent, deduplicate by name.
    let intent_dim_names: HashSet<&str> = intent.dimensions.iter().map(String::as_str).collect();
    let mut seen_dims: HashSet<String> = HashSet::new();
    let dimensions: Vec<_> = intent
        .metrics
        .iter()
        .flat_map(|m| catalog.get_valid_dimensions(m))
        .filter(|d| intent_dim_names.contains(d.name.as_str()) && seen_dims.insert(d.name.clone()))
        .collect();

    // Join paths between distinct metric tables.
    let tables: Vec<String> = metrics
        .iter()
        .map(|m| m.table.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let mut join_paths = Vec::new();
    for i in 0..tables.len() {
        for j in (i + 1)..tables.len() {
            if let Some(jp) = catalog.get_join_path(&tables[i], &tables[j]) {
                join_paths.push((tables[i].clone(), tables[j].clone(), jp));
            }
        }
    }

    TranslationContext {
        metrics,
        dimensions,
        join_paths,
    }
}

// Free helpers

fn spec_source_label(source: &SolutionSource) -> String {
    match source {
        SolutionSource::Automation { .. } => "Automation".to_string(),
        SolutionSource::SqlFile { .. } => "SqlFile".to_string(),
        _ => "Llm".to_string(),
    }
}

/// Extract `(table_a, table_b, join_key)` triples from raw [`JoinPath`] records.
///
/// Parses the `" ON "` clause and pulls the key column name from the first
/// qualified reference (`<table>.<column>`).
fn extract_join_paths(join_paths: &[(String, String, JoinPath)]) -> Vec<(String, String, String)> {
    join_paths
        .iter()
        .filter_map(|(a, b, jp)| {
            jp.path
                .split(" ON ")
                .nth(1)
                .and_then(|on| on.split('.').nth(1))
                .and_then(|kk| kk.split_whitespace().next())
                .map(|k| (a.clone(), b.clone(), k.to_string()))
        })
        .collect()
}

/// Return the connector name associated with the first recognized table, or
/// fall back to `default`.
fn resolve_connector(tables: &[String], catalog: &SemanticCatalog, default: &str) -> String {
    tables
        .iter()
        .find_map(|t| catalog.connector_for_table(t).map(|s| s.to_string()))
        .unwrap_or_else(|| default.to_string())
}

/// Emit a [`AnalyticsEvent::SpecResolved`] event for `spec`.
async fn emit_spec_resolved(
    event_tx: &Option<EventStream<AnalyticsEvent>>,
    spec: &QuerySpec,
    source: &str,
) {
    emit_domain(
        event_tx,
        AnalyticsEvent::SpecResolved {
            resolved_metrics: spec.resolved_metrics.clone(),
            resolved_tables: spec.resolved_tables.clone(),
            join_path: spec.join_path.clone(),
            result_shape: fmt_result_shape(&spec.expected_result_shape),
            assumptions: spec.assumptions.clone(),
            solution_source: source.to_string(),
        },
    )
    .await;
}

/// Emit a [`AnalyticsEvent::ValidationFailed`] event and build a
/// `Diagnosing` [`TransitionResult`] that re-tries `Specify` with a hint.
async fn diagnose_validation_error(
    event_tx: &Option<EventStream<AnalyticsEvent>>,
    err: AnalyticsError,
    spec: &QuerySpec,
    retry_ctx: Option<&RetryContext>,
    intent: &AnalyticsIntent,
) -> TransitionResult<AnalyticsDomain> {
    emit_domain(
        event_tx,
        AnalyticsEvent::ValidationFailed {
            state: "specifying".to_string(),
            reason: err.to_string(),
            model_response: format!("{spec:#?}"),
        },
    )
    .await;
    let hint = retry_ctx
        .cloned()
        .unwrap_or_default()
        .advance(err.to_string());
    let mut retry_intent = intent.clone();
    retry_intent.spec_hint = spec.query_request_item.clone();
    TransitionResult::diagnosing(ProblemState::Diagnosing {
        error: err,
        back: BackTarget::Specify(retry_intent, hint),
    })
}

/// Parse the raw [`LlmOutput`] from `specify_impl` into a
/// [`SpecifyResponseEnvelope`].
///
/// Tries the structured-response field first; falls back to JSON-fence-stripped
/// text.  Returns an `Err` that routes back to `Specify` on any parse failure.
fn parse_llm_response(
    output: LlmOutput,
    intent: &AnalyticsIntent,
) -> Result<SpecifyResponseEnvelope, (AnalyticsError, BackTarget<AnalyticsDomain>)> {
    let envelope: SpecifyResponseEnvelope = if let Some(structured) = output.structured_response {
        serde_json::from_value(structured).map_err(|e| {
            let msg = format!("failed to deserialise structured specify response: {e}");
            (
                AnalyticsError::NeedsUserInput { prompt: msg },
                BackTarget::Specify(intent.clone(), Default::default()),
            )
        })?
    } else {
        if output.text.trim().is_empty() {
            let msg = "LLM returned empty text (no structured response); retrying".to_string();
            return Err((
                AnalyticsError::NeedsUserInput { prompt: msg },
                BackTarget::Specify(intent.clone(), Default::default()),
            ));
        }
        let raw = strip_json_fences(&output.text).to_owned();
        serde_json::from_str::<SpecifyResponseEnvelope>(&raw).map_err(|e| {
            let raw_full = &output.text;
            let msg = format!("failed to parse specify response as JSON: {e}\nRaw: {raw_full}");
            (
                AnalyticsError::NeedsUserInput { prompt: msg },
                BackTarget::Specify(intent.clone(), Default::default()),
            )
        })?
    };

    if envelope.specs.is_empty() {
        return Err((
            AnalyticsError::NeedsUserInput {
                prompt: "LLM returned an empty specs array; retrying".to_string(),
            },
            BackTarget::Specify(intent.clone(), Default::default()),
        ));
    }

    Ok(envelope)
}

/// Parse the raw [`LlmOutput`] into a [`QueryRequestEnvelope`] (airlayer-native).
fn parse_query_request_response(
    output: LlmOutput,
    intent: &AnalyticsIntent,
) -> Result<QueryRequestEnvelope, (AnalyticsError, BackTarget<AnalyticsDomain>)> {
    let envelope: QueryRequestEnvelope = if let Some(structured) = output.structured_response {
        serde_json::from_value(structured).map_err(|e| {
            let msg = format!("failed to deserialise query request response: {e}");
            (
                AnalyticsError::NeedsUserInput { prompt: msg },
                BackTarget::Specify(intent.clone(), Default::default()),
            )
        })?
    } else {
        if output.text.trim().is_empty() {
            let msg = "LLM returned empty text (no structured response); retrying".to_string();
            return Err((
                AnalyticsError::NeedsUserInput { prompt: msg },
                BackTarget::Specify(intent.clone(), Default::default()),
            ));
        }
        let raw = strip_json_fences(&output.text).to_owned();
        serde_json::from_str::<QueryRequestEnvelope>(&raw).map_err(|e| {
            let raw_full = &output.text;
            let msg =
                format!("failed to parse query request response as JSON: {e}\nRaw: {raw_full}");
            (
                AnalyticsError::NeedsUserInput { prompt: msg },
                BackTarget::Specify(intent.clone(), Default::default()),
            )
        })?
    };

    if envelope.specs.is_empty() {
        return Err((
            AnalyticsError::NeedsUserInput {
                prompt: "LLM returned an empty specs array; retrying".to_string(),
            },
            BackTarget::Specify(intent.clone(), Default::default()),
        ));
    }

    Ok(envelope)
}

// State handler (hybrid routing + fan-out)

/// Build the `StateHandler` for the **specifying** state.
///
/// Tries paths in order:
/// 1. Automation short-circuit
/// 2. VendorEngine (first-pass only)
/// 3. Primary path (LLM → QueryRequest → airlayer compile) when semantic model exists
/// 4. Legacy path (LLM → SQL fragments) when no semantic model
pub(super) fn build_specifying_handler()
-> StateHandler<AnalyticsDomain, AnalyticsSolver, AnalyticsEvent> {
    StateHandler {
        next: "executing",
        execute: Arc::new(
            |solver: &mut AnalyticsSolver,
             state,
             _events,
             run_ctx: &RunContext<AnalyticsDomain>,
             _memory: &SessionMemory<AnalyticsDomain>| {
                Box::pin(async move {
                    let intent = match state {
                        ProblemState::Specifying(i) => i,
                        _ => unreachable!("specifying handler called with wrong state"),
                    };
                    let retry_ctx = run_ctx.retry_ctx.clone();

                    // ── 0. Automation/SQL file short-circuit ─────────────
                    if let Some(ref file_path) = intent.selected_automation.clone() {
                        let default_conn = solver.default_connector.clone();

                        // Guard against path traversal for ALL automation types
                        // (SQL, automation, .procedure.yml, etc.) — not just .sql.
                        // A relative path containing `..` that escapes the workspace
                        // root is rejected here before any read or execution.
                        if let Some(ref ws) = solver.workspace_path {
                            let abs_path = if file_path.is_absolute() {
                                file_path.clone()
                            } else {
                                ws.join(file_path)
                            };
                            // Use canonicalize when the path exists (resolves symlinks).
                            // Fall back to lexical normalization so that non-existent
                            // traversal paths (e.g. `../../etc/passwd.sql`) still have
                            // their `..` components collapsed before the prefix check —
                            // the raw abs_path would still start_with(ws) before `..`
                            // is resolved, letting a traversal slip through.
                            let canonical_ws =
                                std::fs::canonicalize(ws).unwrap_or_else(|_| normalize_path(ws));
                            let canonical_abs = std::fs::canonicalize(&abs_path)
                                .unwrap_or_else(|_| normalize_path(&abs_path));
                            if !canonical_abs.starts_with(&canonical_ws) {
                                return TransitionResult::diagnosing(ProblemState::Diagnosing {
                                    error: crate::AnalyticsError::FileReadError {
                                        file_path: file_path.display().to_string(),
                                        message: "path escapes workspace root".to_string(),
                                    },
                                    back: agentic_core::back_target::BackTarget::Clarify(
                                        intent,
                                        Default::default(),
                                    ),
                                });
                            }
                        }

                        // SQL files are executed directly as verified queries.
                        if file_path.extension().is_some_and(|e| e == "sql") {
                            // Resolve workspace-relative paths (as returned by
                            // `search_automations`) against the workspace root.
                            let abs_path = if file_path.is_absolute() {
                                file_path.clone()
                            } else if let Some(ref ws) = solver.workspace_path {
                                ws.join(file_path)
                            } else {
                                file_path.clone()
                            };

                            // Guard against path traversal: reject any resolved
                            // path that escapes the workspace root (e.g. a path
                            // containing `..` that the LLM produced). This
                            // mirrors the analogous check in the automation domain
                            // (`pipeline_ref::resolve_pipeline_ref`).
                            //
                            // The syntactic `..`-component check MUST come first
                            // and is filesystem-independent: the target file
                            // legitimately may not exist yet, and
                            // `std::fs::canonicalize` errors on a missing path —
                            // its `unwrap_or_else` fallback then leaves the `..`
                            // segments in place, so a lexical `starts_with`
                            // against the canonicalized workspace wrongly judges
                            // `<ws>/../../etc/passwd.sql` as contained. (It only
                            // appeared to work where the workspace sat behind a
                            // symlink — e.g. macOS `/var`→`/private/var` — whose
                            // prefix mismatch masked the bug; on a plain `/tmp`
                            // it lets the traversal through.)
                            if file_path
                                .components()
                                .any(|c| matches!(c, std::path::Component::ParentDir))
                            {
                                return TransitionResult::diagnosing(ProblemState::Diagnosing {
                                    error: crate::AnalyticsError::FileReadError {
                                        file_path: file_path.display().to_string(),
                                        message: "path escapes workspace root".to_string(),
                                    },
                                    back: agentic_core::back_target::BackTarget::Clarify(
                                        intent,
                                        Default::default(),
                                    ),
                                });
                            }

                            // Defence-in-depth: even without `..` segments, a
                            // symlink inside the workspace could point out of it.
                            // Canonicalize both sides and require containment.
                            // (Only meaningful when the target exists; harmless
                            // otherwise.)
                            if let Some(ref ws) = solver.workspace_path {
                                let canonical_ws = std::fs::canonicalize(ws)
                                    .unwrap_or_else(|_| normalize_path(ws));
                                let canonical_abs = std::fs::canonicalize(&abs_path)
                                    .unwrap_or_else(|_| normalize_path(&abs_path));
                                if !canonical_abs.starts_with(&canonical_ws) {
                                    return TransitionResult::diagnosing(
                                        ProblemState::Diagnosing {
                                            error: crate::AnalyticsError::FileReadError {
                                                file_path: file_path.display().to_string(),
                                                message: "path escapes workspace root".to_string(),
                                            },
                                            back: agentic_core::back_target::BackTarget::Clarify(
                                                intent,
                                                Default::default(),
                                            ),
                                        },
                                    );
                                }
                            }

                            let sql = match tokio::fs::read_to_string(&abs_path).await {
                                Ok(s) => s,
                                Err(e) => {
                                    // Fatal: a missing/unreadable verified SQL
                                    // file is a user-actionable problem, not a
                                    // SQL syntax error or something the LLM can
                                    // recover from by retrying.
                                    return TransitionResult::diagnosing(
                                        ProblemState::Diagnosing {
                                            error: crate::AnalyticsError::FileReadError {
                                                file_path: abs_path.display().to_string(),
                                                message: e.to_string(),
                                            },
                                            back: agentic_core::back_target::BackTarget::Clarify(
                                                intent,
                                                Default::default(),
                                            ),
                                        },
                                    );
                                }
                            };
                            // Honor the optional `database:` annotation in the
                            // SQL file's `/* oxy: ... */` header, falling back
                            // to the agent's default connector only when the
                            // header names none.
                            //
                            // A named-but-unregistered database is passed
                            // through UNCHANGED so `execute_solution` fails
                            // with the routing error. Substituting the default
                            // here would launder the misroute before the guard
                            // can see it -- and this is the worst path to do
                            // that on: a Verified Query is hand-written SQL,
                            // bound to one dialect by whoever wrote it, so its
                            // `database:` is a declaration and never a hint.
                            // Rerouting it runs DuckDB SQL on ClickHouse.
                            let connector_name =
                                agentic_core::subrun::parse_oxy_comment_block(&sql)
                                    .and_then(|b| b.database)
                                    .unwrap_or(default_conn);
                            return TransitionResult::ok(ProblemState::Executing(
                                crate::AnalyticsSolution {
                                    payload: SolutionPayload::Sql(sql),
                                    solution_source: crate::SolutionSource::SqlFile {
                                        file_path: file_path.clone(),
                                    },
                                    connector_name,
                                    semantic_query: None,
                                },
                            ));
                        }
                        // Automation: delegate to coordinator.
                        return TransitionResult::ok(ProblemState::Executing(
                            crate::AnalyticsSolution {
                                payload: SolutionPayload::Sql(String::new()),
                                solution_source: crate::SolutionSource::Automation {
                                    file_path: file_path.clone(),
                                },
                                connector_name: default_conn,
                                semantic_query: None,
                            },
                        ));
                    }

                    // ── 1. VendorEngine (first-pass only) ──────────────────────
                    if retry_ctx.is_none()
                        && let Some(result) = solver.specifying_try_vendor_engine(&intent).await
                    {
                        return result;
                    }

                    // ── 2. Primary or legacy path ──────────────────────────────
                    if !solver.catalog.is_empty() {
                        // Has semantic model: LLM → QueryRequest → compile
                        solver.specifying_primary(intent, retry_ctx.as_ref()).await
                    } else {
                        // No semantic model: legacy SQL fragment resolution
                        solver
                            .specifying_try_llm_fallback_legacy(intent, retry_ctx.as_ref())
                            .await
                    }
                })
            },
        ),
        diagnose: None,
    }
}

/// Normalize a path lexically (no I/O) by resolving `.` and `..` components.
/// Used as the fallback when `std::fs::canonicalize` fails (e.g. file does
/// not exist) so path-traversal guards still fire correctly.
fn normalize_path(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut components: Vec<Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // Only pop a normal component — preserve leading `..` on
                // relative paths and the root prefix on absolute ones.
                if matches!(components.last(), Some(Component::Normal(_))) {
                    components.pop();
                }
            }
            other => components.push(other),
        }
    }
    components.iter().collect()
}
