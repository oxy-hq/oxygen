use minijinja::{Value, context};

use oxy::exec_runtime::ExecutionContext;
use oxy::exec_types::{Output, TargetOutput};
use oxy_shared::errors::OxyError;

use super::types::Record;

/// Render the LLM-as-judge prompt for one correctness case: the agent's actual
/// answer vs the case's expected answer, plus the case prompt (from the expected
/// side's `task_description`). The rendered prompt is self-contained — it is sent
/// as the single user turn of a one-shot completion.
pub(super) fn render_correctness_prompt(
    execution_context: &ExecutionContext,
    prompt_template: &str,
    actual: &TargetOutput,
    expected: &TargetOutput,
) -> Result<String, OxyError> {
    let prompt = expected.task_description.as_deref().unwrap_or("");
    let ctx = context! {
        actual => Value::from_safe_string(actual.output.to_string()),
        expected => Value::from_safe_string(expected.output.to_string()),
        prompt => Value::from_safe_string(prompt.to_string()),
    };
    execution_context
        .renderer
        .render_once(prompt_template, ctx)
        .map_err(|_| {
            OxyError::RuntimeError("Failed to render correctness evaluation prompt".to_string())
        })
}

/// Parse a correctness judge response into a Record.
/// Scans the last few lines for "PASS" or "FAIL" (case-insensitive).
pub(super) fn parse_correctness_record(output: Output) -> Result<Record, OxyError> {
    let response = match output {
        Output::Text(text) => text,
        _ => {
            return Err(OxyError::RuntimeError(
                "Unsupported output type for correctness solver".to_string(),
            ));
        }
    };

    let trimmed = response.trim();
    let lines: Vec<&str> = trimmed.lines().collect();

    // Scan last 5 lines for PASS/FAIL verdict
    let mut verdict = None;
    for line in lines.iter().rev().take(5) {
        let upper = line.to_uppercase();
        // Strip common formatting characters
        let cleaned = upper.replace(['*', '#'], "").replace('_', " ");
        if cleaned.contains("PASS") && !cleaned.contains("FAIL") {
            verdict = Some("PASS");
            break;
        } else if cleaned.contains("FAIL") {
            verdict = Some("FAIL");
            break;
        }
    }

    let choice = match verdict {
        Some(v) => v.to_string(),
        None => {
            tracing::warn!(
                "Could not parse PASS/FAIL verdict from judge response, defaulting to FAIL. \
                 Last 5 lines: {:?}",
                lines.iter().rev().take(5).collect::<Vec<_>>()
            );
            "FAIL".to_string()
        }
    };
    let score = if choice == "PASS" { 1.0 } else { 0.0 };

    // Full response is stored as CoT reasoning (includes verdict line)
    let cot = trimmed.to_string();

    Ok(Record {
        cot,
        choice,
        score,
        prompt: None,
        expected: None,
        actual_output: None,
        references: vec![],
        duration_ms: 0.0,
        input_tokens: 0,
        output_tokens: 0,
    })
}
