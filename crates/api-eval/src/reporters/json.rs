use std::io::Write;

use crate::builders::types::EvalResult;
use oxy_shared::errors::OxyError;

use super::Reporter;

pub struct JsonReporter;

/// The results document: what `oxy test --output-json` writes and
/// `--format json` prints, and what an eval run's row stores as its answer so
/// `oxyc evals run` writes the same bytes. Pretty-printed, with a trailing
/// newline. Pinned by `builders::results_pin_tests`; the skill reference
/// `oxy-evals/references/results-json.md` documents it.
pub fn results_json(results: &[EvalResult]) -> Result<String, OxyError> {
    let json = serde_json::to_string_pretty(results)
        .map_err(|e| OxyError::RuntimeError(format!("Failed to serialize JSON: {e}")))?;
    Ok(format!("{json}\n"))
}

impl Reporter for JsonReporter {
    fn report(&self, results: &[EvalResult], writer: &mut dyn Write) -> Result<(), OxyError> {
        writer.write_all(results_json(results)?.as_bytes())?;
        Ok(())
    }
}
