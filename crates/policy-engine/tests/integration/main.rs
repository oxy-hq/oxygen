//! Integration tests against a per-org OLTP tenant (a container standing in for
//! Neon): the policy read through the op boundary, and the DuckDB sync to
//! Parquet. One binary, per the workspace's grouped-tests rule.

mod common;
mod live_oltp;
mod pg_policy;
mod sync_parquet;
