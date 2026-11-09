//! The operating graph below the HTTP layer: a caller's reach, binding a semantic
//! query to it, the request and row types, the reads other contexts ask, and
//! the assignment writes two surfaces share (`oxy-app`'s assignment routes and
//! `oxy-api-frontline`'s enrolment). The handlers stay in `oxy-app`
//! (`server::api::operating_graph`).

pub mod assignments;
pub mod binding;
pub mod dto;
pub mod reach;
pub mod readers;

/// Is this the unique-constraint collision, rather than any other failure?
///
/// Asked through `sql_err()`, which classifies the driver's SQLSTATE. The
/// earlier spelling, `to_string().contains("23505")`, matched nothing: the
/// Display of a sea-orm query error carries Postgres's message ("duplicate
/// key value violates unique constraint …") and not its code, so every
/// collision read as a 500. The operating-graph tests pin this one.
pub fn is_unique_violation(e: &sea_orm::DbErr) -> bool {
    matches!(
        e.sql_err(),
        Some(sea_orm::SqlErr::UniqueConstraintViolation(_))
    )
}
