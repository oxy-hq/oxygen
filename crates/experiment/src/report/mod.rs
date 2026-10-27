//! The sealed report: fixed rules over the estimator's results.
//! See `internal-docs/experiments.md` § "Results and the surface".

mod build;
mod design;
mod prior;
mod spillover;
mod types;
mod validity;

#[cfg(test)]
mod design_tests;
#[cfg(test)]
mod observed_tests;
#[cfg(test)]
mod prior_tests;
#[cfg(test)]
mod tests;

pub use build::build_report;
pub use types::*;
