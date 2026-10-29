//! The sealed report: fixed rules over the estimator's results.
//! See `internal-docs/experiments.md` § "Results and the surface".

mod build;
mod design;
mod prior;
mod spillover;
mod synthetic;
mod types;
mod validity;
mod waves;

#[cfg(test)]
mod design_tests;
#[cfg(test)]
mod observed_tests;
#[cfg(test)]
mod prior_tests;
#[cfg(test)]
mod synthetic_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod waves_tests;

pub use build::build_report;
pub use synthetic::SYNTHETIC_LABEL;
pub use types::*;
pub use waves::PROPORTIONAL_ESTIMAND;
