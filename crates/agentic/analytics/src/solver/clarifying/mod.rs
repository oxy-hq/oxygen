//! **Clarifying** pipeline stage.
//!
//! Owns:
//! - [`AnalyticsSolver::clarify_impl`] — classifies the question, attempts semantic shortcut, or forwards to Specifying
//! - [`AnalyticsSolver::general_inquiry_impl`] — GeneralInquiry short-circuit
//! - [`build_clarifying_handler`] — `StateHandler` factory

use std::sync::Arc;

use agentic_core::{
    HumanInputQuestion, SuspendReason,
    back_target::{BackTarget, RetryContext},
    human_input::SuspendedRunData,
    orchestrator::{CompletedTurn, RunContext, SessionMemory, StateHandler, TransitionResult},
    solver::DomainSolver,
    state::ProblemState,
    tools::ToolError,
};

use crate::catalog::Catalog;
use crate::events::AnalyticsEvent;
use crate::llm::{LlmError, ThinkingConfig, ToolLoopConfig};
use crate::schemas::triage_response_schema;
use crate::tools::execute_clarifying_tool;
use crate::types::{DomainHypothesis, QueryRequestItem, QuestionType, SolutionSource};
use crate::{AnalyticsAnswer, AnalyticsDomain, AnalyticsError, AnalyticsIntent, AnalyticsSolution};

use super::{
    AnalyticsSolver, emit_domain,
    prompts::{
        GENERAL_INQUIRY_SYSTEM_PROMPT, OPPORTUNITY_SYSTEM_PROMPT, ROOT_CAUSE_SYSTEM_PROMPT,
        TRIAGE_SYSTEM_PROMPT, format_history_section, format_session_turns_section,
    },
    resuming::{ask_user_tool_def, handle_ask_user},
};

// Prompt builders

mod prompts;
pub(super) use prompts::{build_delegation_request, build_triage_user_prompt};

pub(crate) enum ClarifyOutcome {
    /// Normal path: pass the intent to Specifying.
    Intent(AnalyticsIntent),
    /// Fast path: airlayer compiled SQL during Clarifying — go straight to Executing.
    SemanticShortcut(AnalyticsSolution),
}

// Solver impl methods

/// Read a `propose_semantic_query` call: the accepted proposal, or the JSON
/// telling the model why it was refused.
///
/// The deserialisation error is REPORTED, not swallowed. `unwrap_or_default()`
/// turned a malformed proposal into an empty one — no measures, no dimensions,
/// no filters — which then sailed through the gate below (nothing to object to)
/// and came back "accepted" carrying the model's confidence. The shape that
/// triggers it is the one the rejection message steers toward: Cube's native
/// `date_range: "last week"` is a string where `TimeDimensionItem` wants
/// `Option<Vec<String>>`, and serde fails the whole item on it. The one input
/// that cannot be gated is the one that skips the gate.
fn evaluate_proposal(
    params: serde_json::Value,
) -> Result<(QueryRequestItem, f32), serde_json::Value> {
    let confidence = params["confidence"].as_f64().unwrap_or(0.0) as f32;
    let hint = crate::types::query_request::QUERY_REJECTION_HINT;
    let item: QueryRequestItem = match serde_json::from_value(params) {
        Ok(item) => item,
        Err(e) => {
            return Err(serde_json::json!({
                "status": "rejected",
                "reason": format!("could not read the proposed query: {e}. {hint}"),
            }));
        }
    };
    // Reject an operator the semantic model cannot compile, rather than letting
    // it through to be guessed at. A model that invents `last_week` and passes
    // the two ends of the range used to get `IN (start, end)` — every row inside
    // the week dropped, answered as "no sales last week". The model retries with
    // a real operator instead.
    let bad = crate::types::query_request::query_problems(&item);
    if !bad.is_empty() {
        return Err(serde_json::json!({
            "status": "rejected",
            "reason": format!("{}. {hint}", bad.join("; ")),
            "valid_operators": crate::types::query_request::FILTER_OPERATORS,
        }));
    }
    Ok((item, confidence))
}

/// Apply a proposal to the slot the solver reads its answer from.
///
/// **A rejection CLEARS the slot.** The tool loop runs up to five rounds and
/// the slot is not per-round: a model that proposed something valid, then
/// refined it into something the gate refuses, used to leave the earlier
/// proposal sitting there. The loop ended, the solver read the slot, and the
/// run answered from a query the model had already replaced — silently, and
/// with the *first* proposal's confidence attached. Clearing costs a re-propose
/// in the rare case; not clearing answers the wrong question convincingly.
fn record_proposal(
    slot: &std::sync::Mutex<Option<(QueryRequestItem, f32)>>,
    params: serde_json::Value,
) -> serde_json::Value {
    match evaluate_proposal(params) {
        Ok(accepted) => {
            *slot.lock().expect("poisoned") = Some(accepted);
            serde_json::json!({ "status": "accepted" })
        }
        Err(rejection) => {
            *slot.lock().expect("poisoned") = None;
            rejection
        }
    }
}

impl AnalyticsSolver {
    /// Core clarify logic — classifies the question type, detects ambiguities,
    /// attempts a semantic shortcut, then forwards to Specifying.
    #[tracing::instrument(
        skip_all,
        fields(
            oxy.name = "analytics.clarify",
            oxy.span_type = "analytics",
            question_type = tracing::field::Empty,
            semantic_confidence = tracing::field::Empty,
        )
    )]
    pub(crate) async fn clarify_impl(
        &mut self,
        intent: AnalyticsIntent,
        retry_ctx: Option<&RetryContext>,
        session_turns: &[CompletedTurn<AnalyticsDomain>],
    ) -> Result<ClarifyOutcome, (AnalyticsError, BackTarget<AnalyticsDomain>)> {
        let topics_section = self.catalog.topics_summary();
        let user_prompt = build_triage_user_prompt(&intent, session_turns, &topics_section);
        let system_prompt = self.build_system_prompt("clarifying", TRIAGE_SYSTEM_PROMPT, None);
        let thinking = self.thinking_for_state("clarifying", ThinkingConfig::Disabled);

        // Triage tools: catalog/automation search only. RCA questions get
        // routed via QuestionType::RootCause to a dedicated handler that
        // owns the metric-tree tools and their answer path. Keeping
        // triage lean (a) keeps the compiled grammar small enough for
        // strict-mode providers, and (b) preserves the staged architecture
        // — triage classifies; downstream handlers execute.
        let mut tools = crate::tools::triage_tools(self.metric_tree_runner.is_some());
        tools.push(ask_user_tool_def());

        let subrun_runner = self.subrun_runner.clone();
        let catalog = Arc::clone(&self.catalog);
        let human_input = Arc::clone(&self.human_input);

        // Shared slot for the semantic query proposed via the
        // `propose_semantic_query` tool call.  The closure captures a clone
        // of the Arc; after the tool loop we read it out.
        let proposed_query: Arc<std::sync::Mutex<Option<(QueryRequestItem, f32)>>> =
            Arc::new(std::sync::Mutex::new(None));

        // On resume after ask_user suspension, rebuild messages from the
        // persisted conversation snapshot so the LLM continues with full
        // context (catalog results already present).
        let had_user_answer = self.resume_data.is_some();
        let initial: crate::llm::InitialMessages = if let Some(resume) = self.resume_data.take() {
            let prior: Vec<serde_json::Value> = resume.data.stage_data["conversation_history"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if !prior.is_empty() {
                let msgs = self.client.build_resume_messages(
                    &prior,
                    &resume.data.question,
                    &resume.data.suggestions,
                    &resume.answer,
                );
                crate::llm::InitialMessages::Messages(msgs)
            } else {
                // No prior conversation — start fresh with the user's answer appended.
                crate::llm::InitialMessages::User(format!(
                    "{user_prompt}\n\nUser answered the clarifying question \"{q}\": {a}",
                    q = resume.data.question,
                    a = resume.answer,
                ))
            }
        } else if retry_ctx.is_some() {
            crate::llm::InitialMessages::User(user_prompt.clone())
        } else {
            crate::llm::InitialMessages::User(user_prompt.clone())
        };

        let output = self
            .client_for_state("clarifying")
            .run_with_tools(
                &system_prompt,
                initial,
                &tools,
                |name: String, params| {
                    let subrun_runner = subrun_runner.clone();
                    let catalog = Arc::clone(&catalog);
                    let human_input = Arc::clone(&human_input);
                    let proposed_query = Arc::clone(&proposed_query);
                    Box::pin(async move {
                        if name == "ask_user" {
                            handle_ask_user(&params, human_input.as_ref())
                                .map(|v| Box::new(v) as Box<dyn agentic_core::tools::ToolOutput>)
                        } else if name == "search_automations" {
                            let query = params["query"].as_str().unwrap_or("").to_string();
                            let refs = match subrun_runner.as_ref() {
                                Some(runner) => runner.search(&query).await,
                                None => vec![],
                            };
                            let items: Vec<serde_json::Value> = refs
                                .iter()
                                .map(|r| {
                                    serde_json::json!({
                                        "name": r.name,
                                        "path": r.path.display().to_string(),
                                        "description": r.description,
                                    })
                                })
                                .collect();
                            Ok(Box::new(serde_json::json!({ "automations": items }))
                                as Box<dyn agentic_core::tools::ToolOutput>)
                        } else if name == "search_catalog" {
                            execute_clarifying_tool(&name, params, &*catalog)
                                .map(|v| Box::new(v) as Box<dyn agentic_core::tools::ToolOutput>)
                        } else if name == "propose_semantic_query" {
                            Ok(Box::new(record_proposal(&proposed_query, params))
                                as Box<dyn agentic_core::tools::ToolOutput>)
                        } else {
                            Err(ToolError::UnknownTool(name))
                        }
                    })
                },
                &self.event_tx,
                ToolLoopConfig {
                    max_tool_rounds: 5,
                    state: "clarifying".into(),
                    thinking,
                    response_schema: Some(triage_response_schema()),
                    max_tokens_override: self.max_tokens,
                    sub_spec_index: None,
                    system_date_hint: Some(self.system_hint()),
                },
            )
            .await;

        // Handle ask_user suspension: store prior_messages so we can resume
        // the LLM conversation with full context (catalog results etc.).
        let output = match output {
            Err(LlmError::Suspended {
                prompt,
                suggestions,
                prior_messages,
            }) => {
                self.store_suspension_data(SuspendedRunData {
                    from_state: "clarifying".to_string(),
                    original_input: intent.raw_question.clone(),
                    trace_id: String::new(),
                    stage_data: serde_json::json!({
                        "conversation_history": prior_messages,
                    }),
                    question: prompt.clone(),
                    suggestions: suggestions.clone(),
                });
                let questions = vec![HumanInputQuestion {
                    prompt: prompt.clone(),
                    suggestions,
                }];
                return Err((
                    AnalyticsError::NeedsUserInput { prompt },
                    BackTarget::Suspend {
                        reason: SuspendReason::HumanInput { questions },
                    },
                ));
            }
            other => other.map_err(|e| {
                let msg = format!("LLM call failed during clarifying: {e}");
                (
                    AnalyticsError::NeedsUserInput { prompt: msg },
                    BackTarget::Clarify(intent.clone(), Default::default()),
                )
            })?,
        };

        let hypothesis: DomainHypothesis = if let Some(structured) = output.structured_response {
            serde_json::from_value(structured).map_err(|e| {
                let msg = format!("failed to deserialise clarifying response: {e}");
                (
                    AnalyticsError::NeedsUserInput { prompt: msg },
                    BackTarget::Clarify(intent.clone(), Default::default()),
                )
            })?
        } else {
            if output.text.trim().is_empty() {
                let msg = "clarifying: LLM returned empty text".to_string();
                return Err((
                    AnalyticsError::NeedsUserInput { prompt: msg },
                    BackTarget::Clarify(intent.clone(), Default::default()),
                ));
            }
            DomainHypothesis {
                summary: output.text.trim().to_string(),
                question_type: QuestionType::GeneralInquiry, // default to general inquiry if no structured response
                confidence: 0.0,
                ambiguities: vec![],
                time_scope: None,
                ambiguity_questions: vec![],
                semantic_query: None,
                semantic_confidence: 0.0,
                selected_automation_path: None,
                missing_members: vec![],
            }
        };

        emit_domain(
            &self.event_tx,
            AnalyticsEvent::TriageCompleted {
                summary: hypothesis.summary.clone(),
                question_type: format!("{:?}", hypothesis.question_type),
                confidence: hypothesis.confidence,
                ambiguities: hypothesis.ambiguities.clone(),
            },
        )
        .await;

        // Extract the semantic query proposed via the `propose_semantic_query`
        // tool call (if any).  This replaces the former `hypothesis.semantic_query`
        // field which was removed from the triage response schema to reduce
        // grammar size for strict-mode providers.
        let (semantic_query, semantic_confidence) = proposed_query
            .lock()
            .expect("poisoned")
            .take()
            .unwrap_or_default();

        let span = tracing::Span::current();
        span.record("question_type", format!("{:?}", hypothesis.question_type));
        span.record("semantic_confidence", semantic_confidence);

        if hypothesis.question_type == QuestionType::GeneralInquiry {
            return Ok(ClarifyOutcome::Intent(AnalyticsIntent {
                raw_question: intent.raw_question,
                summary: hypothesis.summary.clone(),
                question_type: QuestionType::GeneralInquiry,
                metrics: vec![],
                dimensions: vec![],
                filters: vec![],
                history: intent.history,
                spec_hint: None,
                selected_automation: None,
                semantic_query: semantic_query.clone(),
                semantic_confidence,
            }));
        }

        if hypothesis.question_type == QuestionType::Opportunity {
            return Ok(ClarifyOutcome::Intent(AnalyticsIntent {
                raw_question: intent.raw_question,
                summary: hypothesis.summary.clone(),
                question_type: QuestionType::Opportunity,
                metrics: vec![],
                dimensions: vec![],
                filters: vec![],
                history: intent.history,
                spec_hint: None,
                selected_automation: None,
                semantic_query: semantic_query.clone(),
                semantic_confidence,
            }));
        }

        if hypothesis.question_type == QuestionType::RootCause {
            // Carry the intent forward unchanged; the FSM routes
            // RootCause to root_cause_impl (see general_or_root_cause
            // dispatcher in this module).
            return Ok(ClarifyOutcome::Intent(AnalyticsIntent {
                raw_question: intent.raw_question,
                summary: hypothesis.summary.clone(),
                question_type: QuestionType::RootCause,
                metrics: vec![],
                dimensions: vec![],
                filters: vec![],
                history: intent.history,
                spec_hint: None,
                selected_automation: None,
                semantic_query: semantic_query.clone(),
                semantic_confidence,
            }));
        }

        // Attempt semantic shortcut: if the LLM called `propose_semantic_query`
        // with high confidence, try to compile it locally (fast, no LLM) and
        // skip Specifying/Solving.  semantic_query is always carried forward on
        // the intent regardless of whether the shortcut fires.
        const SEMANTIC_CONFIDENCE_THRESHOLD: f32 = 0.85;

        if semantic_confidence >= SEMANTIC_CONFIDENCE_THRESHOLD
            && !semantic_query.measures.is_empty()
        {
            let measures = semantic_query.measures.clone();
            let dimensions = semantic_query.dimensions.clone();

            emit_domain(
                &self.event_tx,
                AnalyticsEvent::SemanticShortcutAttempted {
                    measures: measures.clone(),
                    dimensions: dimensions.clone(),
                    filters: semantic_query.filters.clone(),
                    time_dimensions: semantic_query.time_dimensions.clone(),
                    confidence: semantic_confidence,
                },
            )
            .await;

            let query_request = semantic_query.to_query_request();
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

                    emit_domain(
                        &self.event_tx,
                        AnalyticsEvent::SemanticShortcutResolved { sql: sql.clone() },
                    )
                    .await;

                    let translation = self.catalog.translate_to_raw_context(&query_request, "");
                    let connector_name = translation
                        .resolved_tables
                        .iter()
                        .find_map(|t| self.catalog.connector_for_table(t).map(|s| s.to_string()))
                        .unwrap_or_else(|| self.default_connector.clone());

                    return Ok(ClarifyOutcome::SemanticShortcut(AnalyticsSolution {
                        payload: self.build_semantic_payload(sql, &query_request),
                        solution_source: SolutionSource::SemanticLayer,
                        connector_name,
                        semantic_query: Some(semantic_query.clone()),
                    }));
                }
                Err(e) => {
                    // Silent fallback — log the error but proceed to Specifying.
                    tracing::info!(
                        "[clarifying] semantic shortcut compile failed, falling through to Specifying: {e}"
                    );
                }
            }
        }

        // ── Builder delegation: ask the builder agent to create missing members ──
        //
        // HARD-DISABLED: the analytics → builder hand-off (the "Builder Agent"
        // auto-invocation that fires from Clarifying when the semantic model is
        // missing members) is currently too buggy to ship, so it is gated off
        // here.  With delegation disabled the pipeline simply falls through to
        // Specifying and answers the question as best it can with the existing
        // catalog — the same behavior the original code used on delegation
        // failure.  Flip `BUILDER_DELEGATION_ENABLED` back to `true` to restore
        // the hand-off once it is stable.  Explicit builder usage (Cmd+I file
        // edits, `.app.yml` creation) is unaffected — those go through the
        // builder pipeline directly, not this branch.
        //
        // When the triage LLM reports missing semantic members (measures or
        // dimensions that the catalog doesn't have) and confidence is below the
        // shortcut threshold, suspend the pipeline and delegate to the builder
        // agent.  On success the pipeline resumes into Clarifying with an
        // updated catalog; on failure it falls through to Specifying as before.
        //
        // Guard: skip delegation when resuming (`had_user_answer`).  If we are
        // re-entering Clarifying after a builder delegation that failed, the
        // triage LLM will report the same missing members — without this guard
        // we would delegate again in an infinite loop.  When the builder
        // succeeded, the catalog is fresh and the members should be found, so
        // `missing_members` will be empty and this branch won't fire anyway.
        const BUILDER_DELEGATION_ENABLED: bool = false;
        if BUILDER_DELEGATION_ENABLED
            && !had_user_answer
            && !hypothesis.missing_members.is_empty()
            && semantic_confidence < SEMANTIC_CONFIDENCE_THRESHOLD
        {
            let (request, context) =
                build_delegation_request(&intent.raw_question, &hypothesis.missing_members);

            self.store_suspension_data(SuspendedRunData {
                from_state: "clarifying".to_string(),
                original_input: intent.raw_question.clone(),
                trace_id: String::new(), // filled by orchestrator
                stage_data: serde_json::json!({}),
                question: request.clone(),
                suggestions: vec![],
            });

            return Err((
                AnalyticsError::NeedsUserInput {
                    prompt: format!(
                        "Delegating to builder: creating {} missing semantic member(s)",
                        hypothesis.missing_members.len()
                    ),
                },
                BackTarget::Suspend {
                    reason: SuspendReason::Delegation {
                        target: agentic_core::delegation::DelegationTarget::Agent {
                            agent_id: "__builder__".to_string(),
                        },
                        request,
                        context,
                        policy: None,
                    },
                },
            ));
        }

        // Ground is dropped: pass the raw question and triage-derived question_type
        // directly to Specifying, which now owns catalog discovery + resolution in one loop.
        // Propagate any automation selected during triage.
        //
        // When the user answered a clarifying question, the hypothesis summary
        // captures the disambiguated intent (e.g. "running performance" instead
        // of just "performance").  Enrich raw_question so Specifying sees the
        // full context — without this the user's answer is lost between stages.
        let enriched_question = if had_user_answer {
            format!(
                "{}\n\nClarified intent: {}",
                intent.raw_question, hypothesis.summary
            )
        } else {
            intent.raw_question
        };
        Ok(ClarifyOutcome::Intent(AnalyticsIntent {
            raw_question: enriched_question,
            summary: hypothesis.summary,
            question_type: hypothesis.question_type,
            metrics: vec![],
            dimensions: vec![],
            filters: vec![],
            history: intent.history,
            spec_hint: None,
            selected_automation: hypothesis
                .selected_automation_path
                .map(std::path::PathBuf::from),
            semantic_query,
            semantic_confidence,
        }))
    }

    /// Answer a [`QuestionType::GeneralInquiry`] directly without SQL.
    #[tracing::instrument(
        skip_all,
        fields(
            oxy.name = "analytics.general_inquiry",
            oxy.span_type = "analytics",
        )
    )]
    pub(crate) async fn general_inquiry_impl(
        &mut self,
        intent: &AnalyticsIntent,
        session_turns: &[CompletedTurn<AnalyticsDomain>],
    ) -> Result<AnalyticsAnswer, (AnalyticsError, BackTarget<AnalyticsDomain>)> {
        let table_names = Catalog::table_names(&*self.catalog);
        let schema_context = self.catalog.to_prompt_string();
        let session_section = format_session_turns_section(session_turns);
        let history_section = format_history_section(&intent.history);

        let user_prompt = format!(
            "{session_section}{history_section}Question: {raw_question}\n\n\
             Available tables: {tables}\n\n\
             Schema overview:\n{schema}",
            raw_question = intent.raw_question,
            tables = if table_names.is_empty() {
                "(none)".to_string()
            } else {
                table_names.join(", ")
            },
            schema = schema_context,
        );

        let system_prompt =
            self.build_system_prompt("clarifying", GENERAL_INQUIRY_SYSTEM_PROMPT, None);
        let thinking = self.thinking_for_state("clarifying", ThinkingConfig::Disabled);
        let output = self
            .client_for_state("clarifying")
            .run_with_tools(
                &system_prompt,
                &user_prompt,
                &[],
                |_name: String, _params| {
                    Box::pin(async {
                        Err::<
                            Box<dyn agentic_core::tools::ToolOutput>,
                            agentic_core::tools::ToolError,
                        >(ToolError::UnknownTool(
                            "no tools in general inquiry".into(),
                        ))
                    })
                },
                &self.event_tx,
                ToolLoopConfig {
                    max_tool_rounds: 0,
                    state: "clarifying".into(),
                    thinking,
                    response_schema: None,
                    max_tokens_override: self.max_tokens,
                    sub_spec_index: None,
                    system_date_hint: Some(self.system_hint()),
                },
            )
            .await
            .map_err(|e| {
                let msg = format!("LLM call failed during general inquiry: {e}");
                (
                    AnalyticsError::NeedsUserInput { prompt: msg },
                    BackTarget::Clarify(intent.clone(), Default::default()),
                )
            })?;

        Ok(AnalyticsAnswer {
            text: output.text,
            display_blocks: vec![],
            spec_hint: None,
        })
    }

    /// Answer a [`QuestionType::RootCause`] question via airlayer's
    /// `explain_metric` tool. Bypasses Specifying / Solving entirely.
    #[tracing::instrument(
        skip_all,
        fields(
            oxy.name = "analytics.root_cause",
            oxy.span_type = "analytics",
        )
    )]
    pub(crate) async fn root_cause_impl(
        &mut self,
        intent: &AnalyticsIntent,
        session_turns: &[CompletedTurn<AnalyticsDomain>],
    ) -> Result<AnalyticsAnswer, (AnalyticsError, BackTarget<AnalyticsDomain>)> {
        self.metric_tree_inquiry_impl(
            intent,
            session_turns,
            ROOT_CAUSE_SYSTEM_PROMPT,
            "root cause",
        )
        .await
    }

    /// Answer a [`QuestionType::Opportunity`] question via airlayer's
    /// `find_opportunities` tool. Bypasses Specifying / Solving entirely.
    #[tracing::instrument(
        skip_all,
        fields(
            oxy.name = "analytics.opportunity",
            oxy.span_type = "analytics",
        )
    )]
    pub(crate) async fn opportunity_impl(
        &mut self,
        intent: &AnalyticsIntent,
        session_turns: &[CompletedTurn<AnalyticsDomain>],
    ) -> Result<AnalyticsAnswer, (AnalyticsError, BackTarget<AnalyticsDomain>)> {
        self.metric_tree_inquiry_impl(
            intent,
            session_turns,
            OPPORTUNITY_SYSTEM_PROMPT,
            "opportunity",
        )
        .await
    }

    /// Shared LLM-tool-loop scaffolding for the dedicated metric-tree handlers.
    /// Distinguished only by system prompt and tracing label; both share the
    /// same metric-tree + search_catalog tool surface and the same answer
    /// path (write a user-facing answer directly from the tool result).
    async fn metric_tree_inquiry_impl(
        &mut self,
        intent: &AnalyticsIntent,
        session_turns: &[CompletedTurn<AnalyticsDomain>],
        system_prompt_template: &str,
        label: &str,
    ) -> Result<AnalyticsAnswer, (AnalyticsError, BackTarget<AnalyticsDomain>)> {
        let Some(runner) = self.metric_tree_runner.clone() else {
            return Err((
                AnalyticsError::NeedsUserInput {
                    prompt: format!(
                        "This workspace does not have a metric tree configured, \
                         so {label} questions cannot be answered."
                    ),
                },
                BackTarget::Clarify(intent.clone(), Default::default()),
            ));
        };

        let catalog = Arc::clone(&self.catalog);
        let session_section = format_session_turns_section(session_turns);
        let history_section = format_history_section(&intent.history);
        let schema_context = self.catalog.to_prompt_string();

        let user_prompt = format!(
            "{session_section}{history_section}Question: {raw_question}\n\n\
             Schema overview:\n{schema}",
            raw_question = intent.raw_question,
            schema = schema_context,
        );

        let system_prompt = self.build_system_prompt("clarifying", system_prompt_template, None);
        let thinking = self.thinking_for_state("clarifying", ThinkingConfig::Disabled);

        let mut tools = crate::tools::metric_tree_tools();
        tools.extend(crate::tools::triage_tools(true));
        let anomaly_store = self.anomaly_store.clone();
        if anomaly_store.is_some() {
            tools.extend(crate::tools::anomaly_tools());
        }
        let workspace_id = self.workspace_id;

        let output = self
            .client_for_state("clarifying")
            .run_with_tools(
                &system_prompt,
                &user_prompt,
                &tools,
                |name: String, params| {
                    let runner = runner.clone();
                    let catalog = Arc::clone(&catalog);
                    let anomaly_store = anomaly_store.clone();
                    Box::pin(async move {
                        if crate::tools::is_metric_tree_tool(&name) {
                            crate::tools::execute_metric_tree_tool(&name, params, runner)
                                .await
                                .map(|v| Box::new(v) as Box<dyn agentic_core::tools::ToolOutput>)
                        } else if name == "search_catalog" {
                            execute_clarifying_tool(&name, params, &*catalog)
                                .map(|v| Box::new(v) as Box<dyn agentic_core::tools::ToolOutput>)
                        } else if matches!(
                            name.as_str(),
                            "list_anomalies" | "detect_anomalies" | "explain_anomaly"
                        ) {
                            let store = anomaly_store.ok_or_else(|| {
                                ToolError::Execution("anomaly store not configured".into())
                            })?;
                            let ctx = crate::tools::AnomalyToolContext {
                                workspace_id,
                                store: store.as_ref(),
                                runner: runner.as_ref(),
                            };
                            crate::tools::execute_anomaly_tool(&name, params, &ctx)
                                .await
                                .map(|v| Box::new(v) as Box<dyn agentic_core::tools::ToolOutput>)
                        } else {
                            Err(ToolError::UnknownTool(name))
                        }
                    })
                },
                &self.event_tx,
                ToolLoopConfig {
                    max_tool_rounds: 6,
                    state: "clarifying".into(),
                    thinking,
                    response_schema: None,
                    max_tokens_override: self.max_tokens,
                    sub_spec_index: None,
                    system_date_hint: Some(self.system_hint()),
                },
            )
            .await
            .map_err(|e| {
                let msg = format!("LLM call failed during {label}: {e}");
                (
                    AnalyticsError::NeedsUserInput { prompt: msg },
                    BackTarget::Clarify(intent.clone(), Default::default()),
                )
            })?;

        Ok(AnalyticsAnswer {
            text: output.text,
            display_blocks: vec![],
            spec_hint: None,
        })
    }
}

// State handler

/// Build the `StateHandler` for the **clarifying** state.
pub(super) fn build_clarifying_handler()
-> StateHandler<AnalyticsDomain, AnalyticsSolver, AnalyticsEvent> {
    StateHandler {
        next: "specifying",
        execute: Arc::new(
            |solver: &mut AnalyticsSolver,
             state,
             _events,
             run_ctx: &RunContext<AnalyticsDomain>,
             memory: &SessionMemory<AnalyticsDomain>| {
                Box::pin(async move {
                    let intent = match state {
                        ProblemState::Clarifying(i) => i,
                        _ => unreachable!("clarifying handler called with wrong state"),
                    };
                    let retry_ctx = run_ctx.retry_ctx.clone();
                    match solver
                        .clarify_impl(intent, retry_ctx.as_ref(), memory.turns())
                        .await
                    {
                        Ok(ClarifyOutcome::SemanticShortcut(solution)) => {
                            TransitionResult::ok_to(ProblemState::Executing(solution), "executing")
                        }
                        Ok(ClarifyOutcome::Intent(clarified))
                            if clarified.question_type == QuestionType::GeneralInquiry =>
                        {
                            match solver
                                .general_inquiry_impl(&clarified, memory.turns())
                                .await
                            {
                                Ok(answer) => {
                                    TransitionResult::ok_to(ProblemState::Done(answer), "done")
                                }
                                Err((err, back)) => {
                                    TransitionResult::diagnosing(ProblemState::Diagnosing {
                                        error: err,
                                        back,
                                    })
                                }
                            }
                        }
                        Ok(ClarifyOutcome::Intent(clarified))
                            if clarified.question_type == QuestionType::RootCause =>
                        {
                            match solver.root_cause_impl(&clarified, memory.turns()).await {
                                Ok(answer) => {
                                    TransitionResult::ok_to(ProblemState::Done(answer), "done")
                                }
                                Err((err, back)) => {
                                    TransitionResult::diagnosing(ProblemState::Diagnosing {
                                        error: err,
                                        back,
                                    })
                                }
                            }
                        }
                        Ok(ClarifyOutcome::Intent(clarified))
                            if clarified.question_type == QuestionType::Opportunity =>
                        {
                            match solver.opportunity_impl(&clarified, memory.turns()).await {
                                Ok(answer) => {
                                    TransitionResult::ok_to(ProblemState::Done(answer), "done")
                                }
                                Err((err, back)) => {
                                    TransitionResult::diagnosing(ProblemState::Diagnosing {
                                        error: err,
                                        back,
                                    })
                                }
                            }
                        }
                        Ok(ClarifyOutcome::Intent(clarified)) => {
                            TransitionResult::ok(ProblemState::Specifying(clarified))
                        }
                        Err((err, back)) => {
                            TransitionResult::diagnosing(ProblemState::Diagnosing {
                                error: err,
                                back,
                            })
                        }
                    }
                })
            },
        ),
        diagnose: None,
    }
}

#[cfg(test)]
mod proposal_tests {
    use super::*;
    use std::sync::Mutex;

    fn good() -> serde_json::Value {
        serde_json::json!({
            "confidence": 0.9,
            "measures": ["toast_sales.net_sales"],
            "dimensions": ["toast_sales.location"],
            "filters": [],
            "time_dimensions": [],
        })
    }

    /// The gate's whole purpose: an operator the semantic model cannot compile
    /// is refused, not guessed at.
    #[test]
    fn an_uncompilable_operator_is_refused_with_the_operators_that_work() {
        let mut p = good();
        p["filters"] = serde_json::json!([{
            "member": "toast_sales.business_date",
            "operator": "last_week",
            "values": ["2026-08-10", "2026-08-16"],
        }]);
        let rejection = evaluate_proposal(p).expect_err("`last_week` must be refused");
        assert_eq!(rejection["status"], "rejected");
        assert!(
            rejection["reason"].as_str().unwrap().contains("last_week"),
            "the message must name the operator: {rejection}"
        );
        assert!(
            rejection["valid_operators"]
                .as_array()
                .expect("the model needs the list to retry with")
                .contains(&serde_json::json!("inDateRange")),
            "a refusal that does not say what WOULD work just loops"
        );
    }

    /// A malformed proposal must not deserialize into an empty-but-valid one.
    #[test]
    fn a_shape_serde_cannot_read_is_reported_not_emptied() {
        let mut p = good();
        // Cube's native form: a string where `TimeDimensionItem` wants a list.
        p["time_dimensions"] = serde_json::json!([{
            "dimension": "toast_sales.business_date",
            "granularity": "day",
            "date_range": "last week",
        }]);
        let rejection = evaluate_proposal(p).expect_err("a shape serde rejects must be reported");
        assert!(
            rejection["reason"]
                .as_str()
                .unwrap()
                .contains("could not read"),
            "{rejection}"
        );
    }

    #[test]
    fn a_sound_proposal_is_accepted_with_its_confidence() {
        let (item, confidence) = evaluate_proposal(good()).expect("this query compiles");
        assert_eq!(item.measures, vec!["toast_sales.net_sales".to_string()]);
        assert!((confidence - 0.9).abs() < 1e-6);
    }

    /// The regression this seam exists for: five tool rounds share one slot,
    /// so a refused refinement must not leave the superseded proposal behind
    /// for the solver to answer from.
    #[test]
    fn a_refusal_clears_the_proposal_it_supersedes() {
        let slot: Mutex<Option<(QueryRequestItem, f32)>> = Mutex::new(None);

        assert_eq!(record_proposal(&slot, good())["status"], "accepted");
        assert!(
            slot.lock().unwrap().is_some(),
            "an accepted proposal is stored"
        );

        let mut refined = good();
        refined["filters"] = serde_json::json!([{
            "member": "toast_sales.business_date",
            "operator": "last_week",
            "values": ["2026-08-10", "2026-08-16"],
        }]);
        assert_eq!(record_proposal(&slot, refined)["status"], "rejected");
        assert!(
            slot.lock().unwrap().is_none(),
            "the refused round left the earlier proposal in place — the run \
             would answer a question the model had already replaced"
        );
    }

    /// Clearing is not one-way: the model re-proposing must still land.
    #[test]
    fn the_model_can_recover_after_a_refusal() {
        let slot: Mutex<Option<(QueryRequestItem, f32)>> = Mutex::new(None);
        record_proposal(
            &slot,
            serde_json::json!({ "confidence": 0.5, "measures": "not-a-list" }),
        );
        assert!(slot.lock().unwrap().is_none());
        assert_eq!(record_proposal(&slot, good())["status"], "accepted");
        assert!(slot.lock().unwrap().is_some());
    }
}
