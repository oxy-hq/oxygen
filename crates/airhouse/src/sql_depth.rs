//! How much stack a statement's tree can need, read from its tokens alone: a
//! linear scan with a heap stack of open brackets, safe on any thread for any
//! input.
//!
//! Size is not what overflows a stack; depth is. Two kinds of depth recurse
//! past sqlparser's recursion limit (which bounds bracket and prefix nesting,
//! and was measured to hold on a 2 MiB stack):
//!
//! - **Chains.** sqlparser builds a left-deep chain in a loop (`1 + 1 + …`,
//!   `a OR b OR …`, `x::t::t…`, `SELECT … UNION SELECT …`), so parsing one is
//!   cheap, but walking, rendering and dropping it recurse once per link.
//! - **Nested types.** `STRUCT(x STRUCT(…))`, `STRUCT<x STRUCT<…>>`,
//!   `INT[][]…` recurse in the type parser itself, which has no limit.
//!
//! A 20,000-row `VALUES` list or a migration of a thousand statements is big
//! and flat: every row, column and statement is a sibling in a list, never a
//! level of the tree.
//!
//! [`stack_units`] is an upper bound on the stack a statement's parse, walk,
//! render and drop need, in units of one chain link. It never under-counts:
//!
//! - Every link of a chain is spelled with an operator or a keyword (`+`,
//!   `OR`, `::`, `UNION`), so each of those counts one unit; operands
//!   (identifiers, numbers, strings, `NULL`) and the few keywords that never
//!   link (`CASE`'s `WHEN`/`THEN`/`ELSE`/`END`, `SELECT`, `ALL`, `DISTINCT`)
//!   never do.
//! - Every level of a nested type is a bracket, a bracket chained right onto
//!   one (`INT[][]`), or an angle bracket after `ARRAY`, `STRUCT` or `MAP`, so
//!   each counts [`NESTING_UNITS`] — measured to cover a type level. A
//!   bracket's own count is its contents' plus [`NESTING_UNITS`], and it adds
//!   to the level around it as that level's deepest bracket (siblings don't
//!   add up; nesting and chaining do).
//! - Commas never reset the count. A set-operation chain runs across them
//!   (`SELECT 1, 2 UNION SELECT 3, 4 UNION …` is as deep as it has `UNION`s),
//!   and a list of plain values adds nothing anyway.
//! - Only a top-level `;` ends a tree: statements are siblings.
//!
//! It over-counts — `a AND b OR c` counts two links for a tree one link deep,
//! and every bracket counts as if it held a type — which only ever moves SQL
//! to the bigger stack sooner or refuses it sooner. Under-counting is the
//! direction that aborts the process.

use sqlparser::keywords::Keyword;
use sqlparser::tokenizer::{Token, TokenWithSpan};

/// What one bracket or type-angle level counts, in chain links. Measured in
/// a debug build (dependencies unoptimized) on a 2 MiB stack running parse,
/// `sql_rules`'s check, render and drop: a chain overflows past 22,283 links
/// (~94 bytes a link); `STRUCT(x STRUCT(…))` past 63 levels (~33 KB a level),
/// `STRUCT<x STRUCT<…>>` past 64 (~32 KB) and `INT[]…[]` past 592 (~3.5 KB).
/// 400 links is ~37.6 KB: over every type level measured.
pub(crate) const NESTING_UNITS: usize = 400;

/// One open bracket (or the statement itself, at the bottom of the stack).
#[derive(Default)]
struct Level {
    /// Operators and keywords seen at this level.
    links: usize,
    /// The deepest bracket closed at this level, its own [`NESTING_UNITS`]
    /// included.
    deepest_bracket: usize,
    /// What the bracket closed last at this level counted: a bracket opened
    /// right after it (`x[1][2]`, `INT[][]`, `f(a)(b)`) chains onto it, so it
    /// counts on top of it rather than beside it.
    last_closed: usize,
    /// For a bracket: what the bracket it chains onto counted, if any.
    chained_onto: usize,
    /// Type angle brackets open at this level now, and the most ever open.
    angles: usize,
    deepest_angles: usize,
}

impl Level {
    fn units(&self) -> usize {
        self.links + self.deepest_bracket + self.deepest_angles * NESTING_UNITS
    }

    fn open_angle(&mut self) {
        self.angles += 1;
        self.deepest_angles = self.deepest_angles.max(self.angles);
    }

    fn close_angles(&mut self, n: usize) {
        self.angles = self.angles.saturating_sub(n);
    }
}

/// An upper bound on the stack the tree `tokens` parse to needs, in chain
/// links (see the module doc). Whitespace and comments count for nothing.
pub(crate) fn stack_units(tokens: &[TokenWithSpan]) -> usize {
    let mut open = vec![Level::default()];
    let mut deepest_statement = 0;
    let mut previous: Option<&Token> = None;
    for TokenWithSpan { token, .. } in tokens {
        if matches!(token, Token::Whitespace(_)) {
            continue;
        }
        let top = open.len() - 1;
        let level = &mut open[top];
        if is_link(token) {
            level.links += 1;
        }
        match token {
            Token::LParen | Token::LBracket | Token::LBrace => {
                let chained_onto = if previous.is_some_and(is_closer) {
                    level.last_closed
                } else {
                    0
                };
                open.push(Level {
                    chained_onto,
                    ..Level::default()
                });
            }
            Token::RParen | Token::RBracket | Token::RBrace if top > 0 => close(&mut open),
            Token::Lt if previous.is_some_and(opens_a_type_angle) => level.open_angle(),
            Token::Gt => level.close_angles(1),
            Token::ShiftRight => level.close_angles(2),
            Token::SemiColon if top == 0 => {
                deepest_statement = deepest_statement.max(level.units());
                *level = Level::default();
            }
            _ => {}
        }
        previous = Some(token);
    }
    // SQL that ends inside a bracket does not parse, but the parser builds
    // what it can before it says so, and that is dropped like any other tree.
    while open.len() > 1 {
        close(&mut open);
    }
    deepest_statement.max(open[0].units())
}

fn close(open: &mut Vec<Level>) {
    if let Some(closed) = open.pop()
        && let Some(around) = open.last_mut()
    {
        let units = closed.chained_onto + closed.units() + NESTING_UNITS;
        around.deepest_bracket = around.deepest_bracket.max(units);
        around.last_closed = units;
    }
}

fn is_closer(token: &Token) -> bool {
    matches!(token, Token::RParen | Token::RBracket | Token::RBrace)
}

/// `ARRAY<`, `STRUCT<`, `MAP<`: the angle brackets sqlparser's type parser
/// recurses into.
fn opens_a_type_angle(token: &Token) -> bool {
    let Token::Word(word) = token else {
        return false;
    };
    matches!(
        word.keyword,
        Keyword::ARRAY | Keyword::STRUCT | Keyword::MAP
    )
}

/// Whether `token` can spell a chain link: anything but an operand (an
/// unquoted non-keyword or quoted identifier, a number, a string, `NULL`,
/// `TRUE`, `FALSE`), a comma, a bracket (counted as nesting instead), or a
/// keyword that never links two parts of a chain:
///
/// - `WHEN`, `THEN`, `ELSE` and `END` separate a `CASE`'s branches, which are
///   siblings; a `CASE` nested in one is counted by its own `CASE` (and is
///   within the parser's recursion limit besides).
/// - `SELECT`, `ALL` and `DISTINCT` open or qualify a query, which nests only
///   in brackets or within the recursion limit; in `… UNION ALL SELECT …` the
///   `UNION` is the link, and counts.
///
/// A token this does not recognize counts, so a new sqlparser token kind can
/// only over-count.
fn is_link(token: &Token) -> bool {
    match token {
        Token::Word(word) => !matches!(
            word.keyword,
            Keyword::NoKeyword
                | Keyword::NULL
                | Keyword::TRUE
                | Keyword::FALSE
                | Keyword::WHEN
                | Keyword::THEN
                | Keyword::ELSE
                | Keyword::END
                | Keyword::SELECT
                | Keyword::ALL
                | Keyword::DISTINCT
        ),
        Token::EOF
        | Token::Comma
        | Token::Number(..)
        | Token::LParen
        | Token::RParen
        | Token::LBracket
        | Token::RBracket
        | Token::LBrace
        | Token::RBrace => false,
        other => !is_string(other),
    }
}

fn is_string(token: &Token) -> bool {
    matches!(
        token,
        Token::SingleQuotedString(_)
            | Token::DoubleQuotedString(_)
            | Token::TripleSingleQuotedString(_)
            | Token::TripleDoubleQuotedString(_)
            | Token::DollarQuotedString(_)
            | Token::SingleQuotedByteStringLiteral(_)
            | Token::DoubleQuotedByteStringLiteral(_)
            | Token::TripleSingleQuotedByteStringLiteral(_)
            | Token::TripleDoubleQuotedByteStringLiteral(_)
            | Token::SingleQuotedRawStringLiteral(_)
            | Token::DoubleQuotedRawStringLiteral(_)
            | Token::TripleSingleQuotedRawStringLiteral(_)
            | Token::TripleDoubleQuotedRawStringLiteral(_)
            | Token::NationalStringLiteral(_)
            | Token::QuoteDelimitedStringLiteral(_)
            | Token::NationalQuoteDelimitedStringLiteral(_)
            | Token::EscapedStringLiteral(_)
            | Token::UnicodeStringLiteral(_)
            | Token::HexStringLiteral(_)
    )
}

#[cfg(test)]
#[path = "sql_depth_tests.rs"]
mod tests;
