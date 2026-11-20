// The eval engine. The command that drives it (`oxy test`), its event handler
// and the reporters are `oxy-api-eval`'s, which reaches this through
// `crate::surface::eval` until the engine follows it (domain-boundaries.md,
// Pending). `pub(crate)` so that re-export can name the metric types.
pub(crate) mod builders;

pub use builders::EvalLauncher;
pub use builders::types::{EvalInput, EvalResult, MetricKind};
