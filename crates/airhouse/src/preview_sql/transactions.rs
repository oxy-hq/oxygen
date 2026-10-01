//! A preview batch may wrap its statements in one transaction, and nothing
//! else: a `BEGIN` left open, a `COMMIT` of someone else's transaction, a
//! savepoint or a chained transaction would carry state past the batch on a
//! pooled connection.

use sqlparser::ast::Statement;

use super::Refused;

/// Refuse unless every `BEGIN` is closed by one `COMMIT` or `ROLLBACK` in the
/// same batch, none nests, and none carries statements, savepoints or chains.
pub(super) fn check(statements: &[Statement]) -> Result<(), Refused> {
    let mut open = false;
    for statement in statements {
        match statement {
            Statement::StartTransaction {
                statements,
                exception,
                ..
            } => {
                if !statements.is_empty() || exception.is_some() {
                    return Err(unbalanced("a BEGIN … END block"));
                }
                if open {
                    return Err(unbalanced("a BEGIN inside a transaction"));
                }
                open = true;
            }
            Statement::Commit { chain, .. } | Statement::Rollback { chain, .. } if *chain => {
                return Err(unbalanced("AND CHAIN"));
            }
            Statement::Rollback {
                savepoint: Some(_), ..
            } => return Err(unbalanced("ROLLBACK TO SAVEPOINT")),
            Statement::Commit { .. } | Statement::Rollback { .. } => {
                if !open {
                    return Err(unbalanced("a COMMIT or ROLLBACK with no BEGIN before it"));
                }
                open = false;
            }
            _ => {}
        }
    }
    if open {
        return Err(unbalanced("a BEGIN with no COMMIT or ROLLBACK after it"));
    }
    Ok(())
}

fn unbalanced(what: &str) -> Refused {
    Refused(format!(
        "{what} is not allowed in a preview: a batch may wrap its statements in one BEGIN … \
         COMMIT, and a transaction may not outlast the batch"
    ))
}
