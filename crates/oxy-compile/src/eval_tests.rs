//! Eval suites (`*.test.yml`) as a compiled kind (`eval_test_definitions`),
//! read by `oxyc evals run`'s routes (`oxy-api-eval`).

/// The first `revisions.schema_version` whose revisions carry eval suites.
///
/// As for [`crate::context_documents::SINCE_SCHEMA_VERSION`]: an empty table
/// has two meanings. A revision compiled at or after this version with no rows
/// has no test files; one compiled before it was never asked, so a reader
/// answers "not compiled" (retryable) rather than "no such test file".
pub const SINCE_SCHEMA_VERSION: i32 = 3;

const _: () = assert!(crate::compile::CURRENT_SCHEMA_VERSION >= SINCE_SCHEMA_VERSION);
