use std::io::Write;

use crate::builders::types::EvalResult;
use oxy_shared::errors::OxyError;

use super::Reporter;

pub struct JsonReporter;

impl Reporter for JsonReporter {
    fn report(&self, results: &[EvalResult], writer: &mut dyn Write) -> Result<(), OxyError> {
        let json = serde_json::to_string_pretty(results)
            .map_err(|e| OxyError::RuntimeError(format!("Failed to serialize JSON: {e}")))?;
        writeln!(writer, "{json}")?;
        Ok(())
    }
}
