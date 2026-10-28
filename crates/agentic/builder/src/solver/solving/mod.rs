use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use agentic_core::{
    HumanInputQuestion, SuspendReason, back_target::BackTarget, human_input::SuspendedRunData,
};
use agentic_llm::{LlmError, LlmOutput};

use agentic_core::orchestrator::RunContext;

use crate::{
    tools::{all_tools, apply_edit, is_data_app_path, safe_path},
    types::{BuilderDomain, BuilderError, BuilderSolution, BuilderSpec, ToolExchange},
};

use super::solver::{BuilderSolver, dispatch_tool, make_resume_stage_data, record_tool_exchange};
use agentic_core::solver::DomainSolver;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct WriteFileArgs {
    pub file_path: String,
    pub content: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct EditFileArgs {
    pub file_path: String,
    pub old_string: String,
    pub new_string: String,
    #[serde(default)]
    pub replace_all: bool,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct DeleteFileArgs {
    pub file_path: String,
    #[serde(default)]
    pub description: String,
}

pub(super) enum WriteOp {
    Write(WriteFileArgs),
    Edit(EditFileArgs),
    Delete(DeleteFileArgs),
}

/// Content computed for a single batched write op (all non-first ops in a turn).
struct PrecomputedOpContent {
    op_type: &'static str,
    description: String,
    old_content: String,
    new_content: String,
}

/// Pre-compute old/new content for every write op in `prior_messages` that is NOT
/// the suspended (first) op.  Returns an ordered list of `(file_path, content)`.
///
/// Multiple ops on the same file are preserved in order.  Each op's `old_content`
/// is the projected output of the previous op on that file (chained), so that when
/// the user accepts them sequentially the content is consistent.
///
/// - `write_file`: new_content = args.content
/// - `edit_file`:  new_content = apply_edit(current_content, old_string, new_string)
/// - `delete_file`: new_content = ""
///
/// Also returns one refusal line per op that was left out because it would create a
/// Data App, so the model is told that file was not written instead of being told the
/// whole batch was applied.
async fn precompute_batch_ops(
    workspace_root: &Path,
    prior_messages: &[serde_json::Value],
    suspended_prompt: &str,
) -> (Vec<(String, PrecomputedOpContent)>, Vec<String>) {
    let suspended_parsed = serde_json::from_str::<serde_json::Value>(suspended_prompt).ok();
    let suspended_file: Option<String> = suspended_parsed
        .as_ref()
        .and_then(|v| v["file_path"].as_str().map(String::from));

    // Seed the chain with the suspended op's projected new_content so that
    // subsequent ops on the same file compute against the right base.
    let mut file_states: HashMap<String, String> = HashMap::new();
    if let (Some(fp), Some(nc)) = (
        suspended_file.clone(),
        suspended_parsed
            .as_ref()
            .and_then(|v| v["new_content"].as_str().map(String::from)),
    ) {
        file_states.insert(fp, nc);
    }

    let mut result = Vec::new();
    let mut refused = Vec::new();
    let mut seen_suspended = false;
    for op in extract_all_write_ops(prior_messages) {
        let file_path = match &op {
            WriteOp::Write(a) => a.file_path.clone(),
            WriteOp::Edit(a) => a.file_path.clone(),
            WriteOp::Delete(a) => a.file_path.clone(),
        };
        if !seen_suspended && suspended_file.as_deref() == Some(file_path.as_str()) {
            seen_suspended = true;
            continue; // first op — info comes from suspended_prompt
        }
        let abs = match safe_path(workspace_root, &file_path) {
            Ok(p) => p,
            Err(_) => continue,
        };
        // Data Apps are retired: `execute_write_file` refuses to create one, and a
        // batched write reaches the disk through this list instead, so the same
        // rule applies here. An existing app may still be edited or deleted.
        if is_data_app_path(&file_path) && !abs.exists() && !matches!(op, WriteOp::Delete(_)) {
            tracing::warn!(file = %file_path, "precompute_batch_ops: skipping a new Data App");
            refused.push(format!(
                "'{file_path}' was not written: Data apps are retired and can no longer be \
                 created. A custom app replaces them (a code-first React app published with \
                 `oxyc publish`); tell the user so."
            ));
            continue;
        }
        // Use chained state if a prior op already projected content for this file.
        let old_content = file_states
            .get(&file_path)
            .cloned()
            .unwrap_or_else(String::new);
        let old_content = if old_content.is_empty() {
            tokio::fs::read_to_string(&abs).await.unwrap_or_default()
        } else {
            old_content
        };
        let (op_type, description, new_content) = match &op {
            WriteOp::Write(a) => ("write_file", a.description.clone(), a.content.clone()),
            WriteOp::Edit(a) => {
                let nc = match apply_edit(&old_content, &a.old_string, &a.new_string, a.replace_all)
                {
                    Ok(edited) => edited,
                    Err(err) => {
                        tracing::warn!(
                            file = %file_path,
                            "precompute_batch_ops: edit_file old_string not found, keeping original content: {err}"
                        );
                        old_content.clone()
                    }
                };
                ("edit_file", a.description.clone(), nc)
            }
            WriteOp::Delete(a) => ("delete_file", a.description.clone(), String::new()),
        };
        file_states.insert(file_path.clone(), new_content.clone());
        result.push((
            file_path,
            PrecomputedOpContent {
                op_type,
                description,
                old_content,
                new_content,
            },
        ));
    }
    (result, refused)
}

/// Extract all `write_file`, `edit_file`, and `delete_file` calls from the most
/// recent assistant turn. Handles all three provider message formats:
/// - Anthropic: `{"role":"assistant","content":[{"type":"tool_use",...}]}`
/// - OpenAI Responses API: `[{"type":"function_call","name":...,"arguments":...}]`
/// - OpenAI Chat Completions: `{"role":"assistant","tool_calls":[...]}`
pub(super) fn extract_all_write_ops(prior_messages: &[serde_json::Value]) -> Vec<WriteOp> {
    let mut result = Vec::new();

    let parse_op = |name: &str, args: &serde_json::Value| -> Option<WriteOp> {
        match name {
            "write_file" => serde_json::from_value::<WriteFileArgs>(args.clone())
                .ok()
                .map(WriteOp::Write),
            "edit_file" => serde_json::from_value::<EditFileArgs>(args.clone())
                .ok()
                .map(WriteOp::Edit),
            "delete_file" => serde_json::from_value::<DeleteFileArgs>(args.clone())
                .ok()
                .map(WriteOp::Delete),
            _ => None,
        }
    };
    let parse_op_str = |name: &str, args_str: &str| -> Option<WriteOp> {
        let args: serde_json::Value = serde_json::from_str(args_str).ok()?;
        parse_op(name, &args)
    };

    for m in prior_messages.iter().rev() {
        // Anthropic: role:"assistant", content array with tool_use blocks.
        if m["role"].as_str() == Some("assistant") {
            if let Some(blocks) = m["content"].as_array() {
                for block in blocks {
                    if block["type"].as_str() == Some("tool_use")
                        && let Some(name) = block["name"].as_str()
                        && let Some(op) = parse_op(name, &block["input"])
                    {
                        result.push(op);
                    }
                }
            }
            // OpenAI Chat Completions: tool_calls array.
            if let Some(tool_calls) = m["tool_calls"].as_array() {
                for tc in tool_calls {
                    if let (Some(name), Some(args_str)) = (
                        tc["function"]["name"].as_str(),
                        tc["function"]["arguments"].as_str(),
                    ) && let Some(op) = parse_op_str(name, args_str)
                    {
                        result.push(op);
                    }
                }
            }
            break; // only inspect the most recent assistant turn
        }
        // OpenAI Responses API: Value::Array of flat function_call items.
        if let Some(items) = m.as_array() {
            for item in items {
                if item["type"].as_str() == Some("function_call")
                    && let (Some(name), Some(args_str)) =
                        (item["name"].as_str(), item["arguments"].as_str())
                    && let Some(op) = parse_op_str(name, args_str)
                {
                    result.push(op);
                }
            }
            if !result.is_empty() {
                break;
            }
        }
    }
    result
}

// LLM retry policy (caps + backoff math) is shared with the analytics solver
// and lives in `agentic_llm::retry` so a policy fix propagates to both domains.
use agentic_llm::retry::{
    MAX_RATE_LIMIT_RETRIES, MAX_TRANSIENT_RETRIES, rate_limit_backoff_secs, transient_backoff_secs,
};

/// Maximum number of retries for a generic (non-rate-limit, non-transient)
/// solve error before surfacing the failure to the user.  Prevents infinite
/// retry loops when the LLM keeps returning recoverable errors.
const MAX_SOLVE_RETRIES: u32 = 3;

impl BuilderSolver {
    pub(crate) async fn solve_impl(
        &mut self,
        spec: BuilderSpec,
        run_ctx: &RunContext<BuilderDomain>,
    ) -> Result<BuilderSolution, (BuilderError, BackTarget<BuilderDomain>)> {
        let system = self.build_solving_system_prompt();
        let default_schema = crate::schema_provider::EmptySchemaProvider;
        let schema_ref: &dyn crate::schema_provider::BuilderSchemaProvider =
            match self.schema_provider.as_ref() {
                Some(p) => p.as_ref(),
                None => &default_schema,
            };
        let mut tools = all_tools(schema_ref);
        // Per-phase pruning: onboarding sets an allowlist so dbt /
        // search_text / run_tests / manage_directory and other tools
        // irrelevant to the artifact being authored aren't surfaced to
        // the LLM.  Reduces tool-selection noise and trims ~3-5K tokens
        // off the cached prefix per onboarding phase.
        if let Some(allowlist) = self.tool_allowlist.as_ref() {
            tools.retain(|t| allowlist.iter().any(|name| name == t.name));
        }
        let (current_initial, prior_tool_exchanges, resume_max_tokens_override) =
            match self.resume_data.take() {
                Some(resume) => {
                    let mut prior_tool_exchanges = resume
                        .data
                        .stage_data
                        .get("tool_exchanges")
                        .cloned()
                        .and_then(|value| serde_json::from_value::<Vec<ToolExchange>>(value).ok())
                        .unwrap_or_default();
                    let (initial_messages, resumed_exchanges, max_tokens_override) =
                        self.resume_initial_messages(&spec, resume).await?;
                    prior_tool_exchanges.extend(resumed_exchanges);
                    (initial_messages, prior_tool_exchanges, max_tokens_override)
                }
                None => (
                    self.build_initial_messages(&spec.question, &spec.history),
                    Vec::new(),
                    None,
                ),
            };
        let exchanges = Arc::new(Mutex::new(prior_tool_exchanges));

        let test_runner = self.test_runner.clone();
        let workspace_root = self.project_root.clone();
        let event_tx = self.event_tx.clone();
        let human_input = self.human_input.clone();
        let db_provider = self.db_provider.clone();
        let project_validator = self.project_validator.clone();
        let schema_provider = self.schema_provider.clone();
        let semantic_compiler = self.semantic_compiler.clone();
        let secrets_provider = self.secrets_provider.clone();
        let exchanges_for_tools = Arc::clone(&exchanges);

        let result = self
            .client
            .run_with_tools(
                &system,
                current_initial,
                &tools,
                move |name, params| {
                    let workspace_root = workspace_root.clone();
                    let event_tx = event_tx.clone();
                    let test_runner = test_runner.clone();
                    let human_input = human_input.clone();
                    let db_provider = db_provider.clone();
                    let project_validator = project_validator.clone();
                    let schema_provider = schema_provider.clone();
                    let semantic_compiler = semantic_compiler.clone();
                    let secrets_provider = secrets_provider.clone();
                    let exchanges = Arc::clone(&exchanges_for_tools);
                    Box::pin(async move {
                        let result = dispatch_tool(
                            &name,
                            &params,
                            &workspace_root,
                            &event_tx,
                            test_runner,
                            human_input,
                            db_provider.as_ref(),
                            project_validator.as_ref(),
                            schema_provider.as_ref(),
                            semantic_compiler.as_ref(),
                            secrets_provider.as_ref(),
                        )
                        .await;
                        let mut guard = exchanges.lock().unwrap_or_else(|e| e.into_inner());
                        record_tool_exchange(&mut guard, &name, &params, &result);
                        result
                    })
                },
                &self.event_tx,
                {
                    let mut config = self.solving_loop_config();
                    config.max_tokens_override = resume_max_tokens_override;
                    config
                },
            )
            .await;

        match result {
            Ok(output) => Ok(self.to_solution(spec, output, exchanges)),
            Err(LlmError::Suspended {
                prompt,
                suggestions,
                prior_messages,
            }) => {
                let tool_exchanges = exchanges.lock().unwrap_or_else(|e| e.into_inner()).clone();
                // Pre-compute old/new content for every batched write op that
                // was NOT the suspended (first) op.  Stored in stage_data so
                // resume can apply each file without a TOCTOU re-read.
                let (batch_ops, refused_ops) =
                    precompute_batch_ops(&self.project_root, &prior_messages, &prompt).await;

                // Build pending_op_prompts from the precomputed ops (already in LLM
                // turn order, suspended op excluded).  Multiple ops on the same file
                // are preserved — content is chained so each op sees the previous
                // op's projected output as its old_content.
                let pending_op_prompts: Vec<String> = batch_ops
                    .iter()
                    .map(|(file_path, content)| {
                        serde_json::json!({
                            "type": content.op_type,
                            "file_path": file_path,
                            "old_content": content.old_content,
                            "new_content": content.new_content,
                            "description": content.description,
                        })
                        .to_string()
                    })
                    .collect();

                let mut stage_data = make_resume_stage_data(
                    &spec,
                    &prior_messages,
                    "tool_suspended",
                    &prompt,
                    &suggestions,
                    &tool_exchanges,
                );
                if !pending_op_prompts.is_empty() {
                    stage_data["pending_op_prompts"] =
                        serde_json::to_value(&pending_op_prompts).unwrap_or_default();
                    stage_data["op_results"] = serde_json::json!([]);
                }
                // Ops left out of the batch are reported to the model with the rest of
                // the results, rather than being counted as applied.
                if !refused_ops.is_empty() {
                    stage_data["op_results"] = serde_json::json!(refused_ops);
                }
                <BuilderSolver as DomainSolver<BuilderDomain>>::store_suspension_data(
                    self,
                    SuspendedRunData {
                        from_state: "solving".to_string(),
                        original_input: spec.question.clone(),
                        trace_id: String::new(),
                        stage_data,
                        question: prompt.clone(),
                        suggestions: suggestions.clone(),
                    },
                );
                Err((
                    BuilderError::NeedsUserInput {
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
                ))
            }
            Err(LlmError::MaxToolRoundsReached {
                rounds,
                prior_messages,
            }) => {
                let tool_exchanges = exchanges.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let prompt = format!(
                    "The builder used all {rounds} allotted tool rounds. Continue with more rounds?"
                );
                let suggestions = vec!["Continue".to_string()];
                <BuilderSolver as DomainSolver<BuilderDomain>>::store_suspension_data(
                    self,
                    SuspendedRunData {
                        from_state: "solving".to_string(),
                        original_input: spec.question.clone(),
                        trace_id: String::new(),
                        stage_data: make_resume_stage_data(
                            &spec,
                            &prior_messages,
                            "max_tool_rounds",
                            &prompt,
                            &suggestions,
                            &tool_exchanges,
                        ),
                        question: prompt.clone(),
                        suggestions: suggestions.clone(),
                    },
                );
                Err((
                    BuilderError::NeedsUserInput {
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
                ))
            }
            Err(LlmError::MaxTokensReached {
                current_max_tokens,
                prior_messages,
                ..
            }) => {
                let tool_exchanges = exchanges.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let doubled = current_max_tokens.saturating_mul(2);
                let prompt = format!(
                    "The builder ran out of token budget ({current_max_tokens} tokens). \
                     Continue with double the budget ({doubled} tokens)?"
                );
                let suggestions = vec!["Continue with double budget".to_string()];
                let mut stage_data = make_resume_stage_data(
                    &spec,
                    &prior_messages,
                    "max_tokens",
                    &prompt,
                    &suggestions,
                    &tool_exchanges,
                );
                stage_data["max_tokens_override"] = serde_json::json!(doubled);
                <BuilderSolver as DomainSolver<BuilderDomain>>::store_suspension_data(
                    self,
                    SuspendedRunData {
                        from_state: "solving".to_string(),
                        original_input: spec.question.clone(),
                        trace_id: String::new(),
                        stage_data,
                        question: prompt.clone(),
                        suggestions: suggestions.clone(),
                    },
                );
                Err((
                    BuilderError::NeedsUserInput {
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
                ))
            }
            Err(LlmError::RateLimit {
                message: msg,
                retry_after,
            }) => {
                let current_attempt = run_ctx
                    .retry_ctx
                    .as_ref()
                    .map_or(0, |ctx| ctx.rate_limit_attempt);
                if current_attempt >= MAX_RATE_LIMIT_RETRIES {
                    let summary = format!(
                        "Rate limit exceeded after {} retries — try again later or switch to a model with higher limits.",
                        current_attempt
                    );
                    tracing::error!("builder solving rate limit exhausted: {msg}");
                    <BuilderSolver as DomainSolver<BuilderDomain>>::store_suspension_data(
                        self,
                        SuspendedRunData {
                            from_state: "solving".to_string(),
                            original_input: spec.question.clone(),
                            trace_id: String::new(),
                            stage_data: Default::default(),
                            question: summary.clone(),
                            suggestions: vec![],
                        },
                    );
                    Err((
                        BuilderError::Llm(summary.clone()),
                        BackTarget::Suspend {
                            reason: SuspendReason::HumanInput {
                                questions: vec![HumanInputQuestion {
                                    prompt: summary,
                                    suggestions: vec![],
                                }],
                            },
                        },
                    ))
                } else {
                    let computed = rate_limit_backoff_secs(current_attempt);
                    // Prefer the provider's `Retry-After` hint over blind
                    // exponential backoff, clamped so a hostile/absurd header
                    // can't stall the run.
                    let delay_secs = retry_after
                        .map(|d| d.as_secs().clamp(1, 300))
                        .unwrap_or(computed);
                    tracing::warn!(
                        attempt = current_attempt + 1,
                        delay_secs,
                        honored_retry_after = retry_after.is_some(),
                        "builder rate limited — backing off before retry"
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;
                    let retry_ctx = run_ctx
                        .retry_ctx
                        .clone()
                        .unwrap_or_default()
                        .advance_rate_limit(msg.clone());
                    Err((BuilderError::Llm(msg), BackTarget::Solve(spec, retry_ctx)))
                }
            }
            Err(LlmError::Transient(msg)) => {
                // Transient transport failure (connection error, timeout, or
                // HTTP 5xx).  Retry with jittered exponential backoff on the
                // same path as rate limits, using the independent transient
                // `attempt` counter so it doesn't share the 429 budget.
                let current_attempt = run_ctx.retry_ctx.as_ref().map_or(0, |ctx| ctx.attempt);
                if current_attempt >= MAX_TRANSIENT_RETRIES {
                    let summary = format!(
                        "The model provider had a transient failure that persisted after {} retries — try again shortly. ({msg})",
                        current_attempt
                    );
                    tracing::error!("builder solving transient error exhausted: {msg}");
                    <BuilderSolver as DomainSolver<BuilderDomain>>::store_suspension_data(
                        self,
                        SuspendedRunData {
                            from_state: "solving".to_string(),
                            original_input: spec.question.clone(),
                            trace_id: String::new(),
                            stage_data: Default::default(),
                            question: summary.clone(),
                            suggestions: vec![],
                        },
                    );
                    Err((
                        BuilderError::Llm(summary.clone()),
                        BackTarget::Suspend {
                            reason: SuspendReason::HumanInput {
                                questions: vec![HumanInputQuestion {
                                    prompt: summary,
                                    suggestions: vec![],
                                }],
                            },
                        },
                    ))
                } else {
                    let delay_secs = transient_backoff_secs(current_attempt);
                    tracing::warn!(
                        attempt = current_attempt + 1,
                        delay_secs,
                        "builder transient LLM error — backing off before retry"
                    );
                    tokio::time::sleep(std::time::Duration::from_secs_f64(delay_secs)).await;
                    let retry_ctx = run_ctx
                        .retry_ctx
                        .clone()
                        .unwrap_or_default()
                        .advance(msg.clone());
                    Err((BuilderError::Llm(msg), BackTarget::Solve(spec, retry_ctx)))
                }
            }
            Err(ref err @ (LlmError::Http(_) | LlmError::Auth(_))) => {
                // Non-transient errors — retrying won't help.  Surface the
                // error to the user via Suspend so the pipeline stops.
                let msg = err.to_string();
                tracing::error!("builder solving fatal LLM error: {msg}");
                <BuilderSolver as DomainSolver<BuilderDomain>>::store_suspension_data(
                    self,
                    SuspendedRunData {
                        from_state: "solving".to_string(),
                        original_input: spec.question.clone(),
                        trace_id: String::new(),
                        stage_data: Default::default(),
                        question: msg.clone(),
                        suggestions: vec![],
                    },
                );
                Err((
                    BuilderError::Llm(msg.clone()),
                    BackTarget::Suspend {
                        reason: SuspendReason::HumanInput {
                            questions: vec![HumanInputQuestion {
                                prompt: format!("Builder encountered a fatal error: {msg}"),
                                suggestions: vec![],
                            }],
                        },
                    },
                ))
            }
            Err(err) => {
                let current_attempt = run_ctx.retry_ctx.as_ref().map_or(0, |ctx| ctx.attempt);
                if current_attempt >= MAX_SOLVE_RETRIES {
                    let msg = format!("Builder failed after {} retries: {err}", current_attempt);
                    tracing::error!("{msg}");
                    <BuilderSolver as DomainSolver<BuilderDomain>>::store_suspension_data(
                        self,
                        SuspendedRunData {
                            from_state: "solving".to_string(),
                            original_input: spec.question.clone(),
                            trace_id: String::new(),
                            stage_data: Default::default(),
                            question: msg.clone(),
                            suggestions: vec![],
                        },
                    );
                    Err((
                        BuilderError::Llm(msg.clone()),
                        BackTarget::Suspend {
                            reason: SuspendReason::HumanInput {
                                questions: vec![HumanInputQuestion {
                                    prompt: msg,
                                    suggestions: vec![],
                                }],
                            },
                        },
                    ))
                } else {
                    tracing::warn!(
                        "builder solving LLM error (attempt {}, will retry): {err}",
                        current_attempt + 1
                    );
                    let retry_ctx = run_ctx
                        .retry_ctx
                        .clone()
                        .unwrap_or_default()
                        .advance(err.to_string());
                    Err((
                        BuilderError::Llm(err.to_string()),
                        BackTarget::Solve(spec, retry_ctx),
                    ))
                }
            }
        }
    }

    fn to_solution(
        &self,
        spec: BuilderSpec,
        output: LlmOutput,
        exchanges: Arc<Mutex<Vec<ToolExchange>>>,
    ) -> BuilderSolution {
        let tool_exchanges = exchanges.lock().unwrap_or_else(|e| e.into_inner()).clone();
        BuilderSolution {
            question: spec.question,
            history: spec.history,
            draft_text: output.text,
            tool_exchanges,
            prior_messages: output.prior_messages,
        }
    }
}

mod resumption;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// An Anthropic-format assistant turn that batches several tool calls.
    fn turn(calls: &[(&str, serde_json::Value)]) -> Vec<serde_json::Value> {
        let blocks: Vec<_> = calls
            .iter()
            .enumerate()
            .map(|(i, (name, input))| {
                json!({"type": "tool_use", "id": format!("t{i}"), "name": name, "input": input})
            })
            .collect();
        vec![json!({"role": "assistant", "content": blocks})]
    }

    fn write(path: &str) -> (&'static str, serde_json::Value) {
        (
            "write_file",
            json!({"file_path": path, "content": "x: 1\n", "description": "d"}),
        )
    }

    /// `write_file` refuses to create a Data App, but a batched turn reaches the disk
    /// through `precompute_batch_ops`: the first call suspends and the rest are queued
    /// without going through the tool. The queue must apply the same rule.
    #[tokio::test]
    async fn a_batched_turn_cannot_queue_a_new_data_app() {
        let dir = std::env::temp_dir().join(format!("batch_data_app_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let root = dir.canonicalize().unwrap();
        tokio::fs::write(root.join("old.app.yml"), "tasks: []\n")
            .await
            .unwrap();

        let messages = turn(&[
            write("semantics/orders.view.yml"),
            write("apps/orders.app.yml"),
            (
                "edit_file",
                json!({
                    "file_path": "apps/ghost.app.yml", "old_string": "a", "new_string": "b",
                    "description": "d"
                }),
            ),
            write("old.app.yml"),
            write("semantics/customers.view.yml"),
        ]);
        let suspended =
            json!({"file_path": "semantics/orders.view.yml", "new_content": "x: 1\n"}).to_string();

        let (queued, refused) = precompute_batch_ops(&root, &messages, &suspended).await;
        let paths: Vec<&str> = queued.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths, ["old.app.yml", "semantics/customers.view.yml"]);
        // The model is told which writes were not made.
        assert_eq!(refused.len(), 2, "{refused:?}");
        assert!(refused[0].contains("'apps/orders.app.yml' was not written"));
        assert!(refused[1].contains("'apps/ghost.app.yml' was not written"));
        assert!(refused.iter().all(|r| r.contains("custom app")));

        tokio::fs::remove_dir_all(&dir).await.ok();
    }
}
