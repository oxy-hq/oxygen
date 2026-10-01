//! How both SQL fences (`sql_rules`, `preview_sql`) — and the host's
//! read/write classifier for other dialects ([`with_parsed`]) — read SQL: one
//! tokenize, one depth check, and a parse on a stack that holds the tree.
//!
//! What overflows a stack is a statement's *depth*, not its size: a chain
//! (`a OR b OR …`) that walking, rendering and dropping recurse through once
//! per link, or a nested type the parser itself recurses through (both beyond
//! sqlparser's recursion limit; `sql_depth`). A 20,000-row `VALUES` list or a
//! 2 MiB migration is big and flat, and parses exactly as it would with no
//! guard at all.
//!
//! So SQL is tokenized once — a linear scan, safe on any stack — and
//! `sql_depth::stack_units` reads from the tokens an upper bound on the stack
//! the tree needs, in chain links. Over [`MAX_SQL_DEPTH`], the SQL is refused
//! before anything is parsed. Over [`SHALLOW_SQL_DEPTH`], parse, check, render
//! and drop run on one dedicated thread whose stack ([`SQL_STACK_BYTES`])
//! holds [`MAX_SQL_DEPTH`] many times over. At or under it — every
//! `ctx.airhouse.append` whatever its row count, and nearly every statement —
//! they run directly on whichever thread called, a Tokio worker in
//! production, with no spawn.
//!
//! Measured in a debug build (dependencies unoptimized — release frames are
//! smaller, so these hold there with more room), running parse, `sql_rules`'s
//! check, render and drop on a thread of a given stack: a 2 MiB stack (a
//! Tokio worker's, and Rust's default for a spawned thread) overflows past
//! 22,283 chain links, and 64 MiB past 700,390 — ~96 bytes a link either way.
//! Nested types measured in the same units: `sql_depth::NESTING_UNITS`. To
//! re-measure, add a throwaway `#[ignore]` test that spawns a thread with an
//! explicit `stack_size` running that work on one generated statement, and
//! bisect its size one process per run
//! (`cargo nextest run -p airhouse --lib --features preview-sql --run-ignored
//! only -E 'test(name)'`): an overflow aborts the whole process, not just the
//! test.

use std::fmt::Display;

use sqlparser::ast::{FromTable, ObjectName, ObjectNamePart, Statement, TableFactor};
use sqlparser::dialect::{Dialect, DuckDbDialect};
use sqlparser::parser::{Parser, ParserError};
use sqlparser::tokenizer::{Token, TokenWithSpan, Tokenizer};

use crate::sql_depth::{NESTING_UNITS, stack_units};
use crate::sql_rules::Refused;

/// The deepest SQL — in `stack_units`, chain links — any caller here parses;
/// deeper is refused unparsed. A 50,000-link chain, or ~120 levels of nested
/// type: where the old 100,000-token cap already put the deepest chain, so no
/// deep SQL that cap let through is newly refused, while size no longer
/// counts at all. At ~96 bytes a link it needs ~4.8 MB, a fourteenth of
/// [`SQL_STACK_BYTES`]'s measured 700,390.
pub const MAX_SQL_DEPTH: usize = 50_000;

/// The stack parse, check, render and drop run on when SQL is deeper than
/// [`SHALLOW_SQL_DEPTH`]: measured to hold 689,062 chain links (see the
/// module doc), ~14x [`MAX_SQL_DEPTH`]. Reserved, not committed: only the
/// pages a parse touches are ever backed.
const SQL_STACK_BYTES: usize = 64 << 20;

/// At or under this many `stack_units`, work runs on the caller's own
/// thread: ~380 KB of stack, a 5.5x margin under the 22,283 links a 2 MiB
/// stack was measured to hold — room left for whatever the caller's own
/// frames already use. Every bracket level counts
/// `sql_depth::NESTING_UNITS` (400), so this is also ~9 nested brackets.
const SHALLOW_SQL_DEPTH: usize = 4_000;

const _: () = assert!(SHALLOW_SQL_DEPTH < MAX_SQL_DEPTH);

/// Parse `sql` as `dialect` speaks it and hand the result to `work`, which
/// must do everything it does with the tree — walk, render, drop — and return
/// only what it derived. Both run on a stack that holds the tree (see the
/// module doc). SQL that does not tokenize reaches `work` as the parse error
/// [`Parser::parse_sql`] would give.
///
/// `Err` — and `work` never runs — when `sql` is deeper than
/// [`MAX_SQL_DEPTH`] or the big-stack thread cannot start. A caller that
/// decides whether SQL may run treats that as "cannot tell": held, never
/// sent.
pub fn with_parsed<T: Send>(
    dialect: &(dyn Dialect + Sync),
    sql: &str,
    work: impl FnOnce(Result<Vec<Statement>, ParserError>) -> T + Send,
) -> Result<T, Refused> {
    match Tokenizer::new(dialect, sql).tokenize_with_location() {
        Ok(tokens) => parse_on_sql_stack(dialect, tokens, work),
        Err(e) => Ok(work(Err(e.into()))),
    }
}

/// The fences' parse: [`with_parsed`] as DuckDB, failing closed. SQL that
/// does not parse, backtick quoting (not DuckDB's, so sqlparser and DuckDB
/// would disagree on the names), an empty batch and SQL deeper than
/// [`MAX_SQL_DEPTH`] are refused; `work` gets the statements otherwise.
/// `sender` names who sent the SQL, for the message.
pub(crate) fn on_sql_stack<T: Send>(
    sql: &str,
    sender: &str,
    work: impl FnOnce(Vec<Statement>) -> Result<T, Refused> + Send,
) -> Result<T, Refused> {
    let dialect = DuckDbDialect {};
    let tokens = Tokenizer::new(&dialect, sql)
        .tokenize_with_location()
        .map_err(|e| unparsed(sender, &e))?;
    if tokens
        .iter()
        .any(|t| matches!(&t.token, Token::Word(w) if w.quote_style == Some('`')))
    {
        return Err(Refused(
            "backtick-quoted identifiers are not DuckDB syntax; quote names with double quotes"
                .into(),
        ));
    }
    parse_on_sql_stack(&dialect, tokens, |parsed| {
        let statements = parsed.map_err(|e| unparsed(sender, &e))?;
        if statements.is_empty() {
            return Err(Refused("there is no statement to run".into()));
        }
        work(statements)
    })?
}

/// Parse `tokens` and run `work` on the result: here when they are shallow,
/// on the big-stack thread when not, and not at all when they are too deep.
fn parse_on_sql_stack<T: Send>(
    dialect: &(dyn Dialect + Sync),
    tokens: Vec<TokenWithSpan>,
    work: impl FnOnce(Result<Vec<Statement>, ParserError>) -> T + Send,
) -> Result<T, Refused> {
    let depth = stack_units(&tokens);
    if depth > MAX_SQL_DEPTH {
        return Err(Refused(format!(
            "this SQL nests too deep to check: it counts {depth} against a limit of \
             {MAX_SQL_DEPTH} (each operator, AND/OR or UNION in a chain counts 1, each level of \
             brackets or nested type {NESTING_UNITS}) — split it into several statements, or \
             use an IN list instead of a long OR chain"
        )));
    }
    let run = move || {
        work(
            Parser::new(dialect)
                .with_tokens_with_locations(tokens)
                .parse_statements(),
        )
    };
    if depth <= SHALLOW_SQL_DEPTH {
        return Ok(run());
    }
    std::thread::scope(|scope| {
        let thread = std::thread::Builder::new()
            .name("airhouse-sql".into())
            .stack_size(SQL_STACK_BYTES)
            .spawn_scoped(scope, run)
            .map_err(|e| Refused(format!("could not start the SQL checker: {e}")))?;
        Ok(thread
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic)))
    })
}

fn unparsed(sender: &str, e: &dyn Display) -> Refused {
    Refused(format!(
        "could not parse this as DuckDB SQL ({e}); SQL {sender} sends to Airhouse must parse"
    ))
}

/// File name endings DuckDB's replacement scans read as a file when no table
/// has the name: `FROM x.parquet` and `FROM "s3://b/x.csv"` read files.
/// Compression suffixes are here because `x.csv.gz` ends in them.
const FILE_ENDINGS: [&str; 18] = [
    "parquet", "csv", "tsv", "tbl", "json", "jsonl", "ndjson", "xlsx", "avro", "arrow", "db",
    "duckdb", "sqlite", "gz", "gzip", "zst", "zstd", "bz2",
];
/// Characters no table name needs and file paths and globs do.
const PATH_CHARS: [char; 8] = ['.', '/', '\\', ':', '?', '*', '[', '{'];

/// Refuse a table name *read* where DuckDB could read a file instead: a
/// quoted string (`FROM 's3://…/x.parquet'`), a part holding a path or glob
/// character, or a name ending in a file type (`FROM x.parquet`, `DESCRIBE
/// a.b.csv`). When no table has such a name, DuckDB's replacement scans read
/// the file. Only read positions: a FROM, JOIN or USING table other than a
/// DML target ([`write_target_addresses`]), and [`described_table`]. A write
/// target named `app_x.json` is a table name and nothing else.
pub(crate) fn check_relation(name: &ObjectName) -> Result<(), Refused> {
    let file_like = name.0.iter().any(|part| match part {
        ObjectNamePart::Identifier(ident) => {
            ident.quote_style == Some('\'') || ident.value.contains(PATH_CHARS)
        }
        _ => false,
    }) || (name.0.len() > 1
        && matches!(name.0.last(), Some(ObjectNamePart::Identifier(last))
            if FILE_ENDINGS.iter().any(|e| last.value.eq_ignore_ascii_case(e))));
    if file_like {
        return Err(Refused(format!(
            "{name} looks like a file, not a table: DuckDB reads a table name like this as a file \
             when no table has it. Name tables schema.table, without quotes around the whole \
             name, dots inside a part, or a file type at the end"
        )));
    }
    Ok(())
}

/// The table `DESCRIBE t` or `SHOW COLUMNS FROM t` reads, outside any FROM.
pub(crate) fn described_table(statement: &Statement) -> Option<&ObjectName> {
    match statement {
        Statement::ExplainTable { table_name, .. } => Some(table_name),
        Statement::ShowColumns { show_options, .. } => show_options
            .show_in
            .as_ref()
            .and_then(|show_in| show_in.parent_name.as_ref()),
        _ => None,
    }
}

/// Where in the tree the names of the tables a DML statement writes live:
/// UPDATE's table, DELETE's FROM tables, MERGE's INTO table. They are table
/// factors like any read, but DuckDB binds them as tables and never scans a
/// file for them, so [`check_relation`] skips them by address. (INSERT and DDL
/// targets are not table factors, and are never checked.)
pub(crate) fn write_target_addresses(statement: &Statement) -> Vec<usize> {
    let factors: Vec<&TableFactor> = match statement {
        Statement::Update(update) => vec![&update.table.relation],
        Statement::Delete(delete) => {
            let (FromTable::WithFromKeyword(from) | FromTable::WithoutKeyword(from)) = &delete.from;
            from.iter().map(|t| &t.relation).collect()
        }
        Statement::Merge(merge) => vec![&merge.table],
        _ => Vec::new(),
    };
    factors
        .into_iter()
        .filter_map(|factor| match factor {
            TableFactor::Table { name, .. } => Some(address(name)),
            _ => None,
        })
        .collect()
}

/// A table name's place in its tree, to match [`write_target_addresses`].
pub(crate) fn address(name: &ObjectName) -> usize {
    std::ptr::from_ref(name) as usize
}

#[cfg(test)]
#[path = "sql_parse_tests.rs"]
mod tests;
