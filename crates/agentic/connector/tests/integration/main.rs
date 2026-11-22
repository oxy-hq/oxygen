//! Integration tests for `agentic-connector`, in ONE binary.
//!
//! Cargo compiles every `tests/*.rs` into its own executable, each statically
//! linking the whole dependency graph — so a test *file* costs a link, not just
//! a compile. Grouping keeps that cost proportional to crates rather than to
//! files. Add a case as a `mod` below, not as a new `tests/*.rs`.
//!
//! See `internal-docs/testing.md` for the cost model.

mod clickhouse_tagged_tests;
mod clickhouse_tests;
mod domo_tests;
mod duckdb_tests;
mod mysql_tests;
mod postgres_tests;
mod postgres_tx_tests;
mod read_only_backstop;

/// Built without `mysql`, `mysql_tests` compiles out entirely. Where
/// `OXY_TEST_REQUIRE_MYSQL=1` asks for those tests (CI), that is a failure,
/// not a silent pass: build agentic-connector with its `mysql` feature.
#[cfg(not(feature = "mysql"))]
#[test]
fn mysql_tests_are_compiled_in_when_required() {
    assert_ne!(
        std::env::var("OXY_TEST_REQUIRE_MYSQL").as_deref(),
        Ok("1"),
        "OXY_TEST_REQUIRE_MYSQL=1 but agentic-connector was built without its \
         `mysql` feature, so the MySQL integration tests did not run"
    );
}
