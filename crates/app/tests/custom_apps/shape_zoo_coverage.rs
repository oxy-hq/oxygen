//! Coverage: every native type the connectors' type maps name has a shape-zoo case.
//!
//! The design's shape zoo section requires that a newly supported type cannot ship without a
//! zoo case. Source scans only, no database, following `tests/authz/app_scope_boundary.rs` and
//! `custom_app_functions_manual_run_guards.rs`: a comment naming a type must not count, so each
//! mapper is read with comments removed, string and char literals kept whole.
//!
//! **Names.** String literals for the three string maps (a trailing `(` dropped, as in
//! `"Nullable("` or `"DECIMAL("`); `Type::X` constants for `is_decodable`, lowercased to the
//! Postgres type name.
//!
//! **Coverage.** A case declares the name up to its first `(`: ClickHouse after peeling
//! `Nullable(…)` / `LowCardinality(…)` (the whole peeled type counts too, for
//! `Object('json')`), Postgres lowercased without quotes, DuckDB uppercased. The name plus
//! digits also covers it (`Enum8` covers `Enum`), unless that longer name is itself mapped
//! (`DateTime64` does not cover `DateTime`).

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::shape_zoo::{self, Engine, LoadedZoo};

#[derive(Clone, Copy)]
enum Names {
    StringLiterals,
    TypeConstants,
}

struct Mapper {
    engine: Engine,
    file: &'static str,
    function: &'static str,
    names: Names,
}

const MAPPERS: &[Mapper] = &[
    Mapper {
        engine: Engine::ClickHouse,
        file: "crates/agentic/connector/src/clickhouse_typed.rs",
        function: "ch_type_to_typed",
        names: Names::StringLiterals,
    },
    Mapper {
        engine: Engine::ClickHouse,
        file: "crates/agentic/connector/src/clickhouse_typed.rs",
        function: "strip_type_wrappers",
        names: Names::StringLiterals,
    },
    Mapper {
        engine: Engine::Postgres,
        file: "crates/agentic/connector/src/postgres_typed.rs",
        function: "pg_typname_to_typed",
        names: Names::StringLiterals,
    },
    Mapper {
        engine: Engine::Postgres,
        file: "crates/agentic/connector/src/postgres_tx/convert.rs",
        function: "is_decodable",
        names: Names::TypeConstants,
    },
    Mapper {
        engine: Engine::DuckDb,
        file: "crates/agentic/connector/src/duckdb/conversion.rs",
        function: "describe_type_to_typed",
        names: Names::StringLiterals,
    },
];

/// Mapped names no table column can have, each with the reason. A backlog, not a waiver:
/// `shape_zoo_exemptions_are_still_mapped_and_still_caseless` fails once one gets a case.
const EXEMPT: &[(Engine, &str, &str)] = &[
    (
        Engine::ClickHouse,
        "Nothing",
        "the type of a bare NULL; ClickHouse refuses it as a column type",
    ),
    (
        Engine::ClickHouse,
        "Object('json')",
        "the deprecated experimental Object type; CREATE TABLE refuses it without a setting the zoo's DDL cannot carry, and `JSON` is covered",
    ),
    (
        Engine::ClickHouse,
        "Object(Nullable('json'))",
        "the same deprecated Object type, nullable",
    ),
    (
        Engine::ClickHouse,
        "Nested",
        "with the default flatten_nested = 1 a Nested column is stored and returned as one Array column per field, so no result column is typed Nested(...)",
    ),
    (
        Engine::Postgres,
        "unknown",
        "the pseudo-type of an untyped literal in a SELECT list; no table column can be declared with it",
    ),
];

struct Mapped {
    engine: Engine,
    name: String,
    function: &'static str,
    file: &'static str,
}

#[test]
fn shape_zoo_has_a_case_for_every_type_the_connectors_map() {
    let zoo = shape_zoo::load();
    let mapped = mapped();
    let mut missing = Vec::new();
    for engine in Engine::ALL {
        let names = names_for(&mapped, engine);
        let declared = declared(engine, &zoo);
        for m in mapped.iter().filter(|m| m.engine == engine) {
            let exempt = EXEMPT.iter().any(|(e, n, _)| *e == engine && *n == m.name);
            if !exempt && !covered(&m.name, &declared, &names) {
                missing.push(format!(
                    "  {} `{}` ({} in {})",
                    engine.name(),
                    m.name,
                    m.function,
                    m.file
                ));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "the shape zoo has no case for {} native type(s) a connector maps:\n{}\nAdd a case to {} \
         for each value class that applies, or, when no column can be declared with the type, \
         an EXEMPT entry in shape_zoo_coverage.rs saying why.",
        missing.len(),
        missing.join("\n"),
        shape_zoo::ZOO_PATH
    );
}

#[test]
fn shape_zoo_exemptions_are_still_mapped_and_still_caseless() {
    let zoo = shape_zoo::load();
    let mapped = mapped();
    for (engine, name, why) in EXEMPT {
        let names = names_for(&mapped, *engine);
        assert!(
            names.contains(*name),
            "EXEMPT names {} `{name}`, which no mapper maps any more; delete the entry",
            engine.name()
        );
        assert!(
            !covered(name, &declared(*engine, &zoo), &names),
            "{} `{name}` has a zoo case now; delete its EXEMPT entry ({why})",
            engine.name()
        );
    }
}

#[test]
fn shape_zoo_scanner_reads_code_not_comments() {
    let src = "// fn target() { \"Ghost\" }\n\
               fn target(x: &str) -> u8 {\n\
                   // \"Commented\"\n\
                   let _ = TypedDataType::Json;\n\
                   if matches!(*ty, Type::INT4 | Type::JSONB) { return 3; }\n\
                   match x { \"Real\" | \"Also(\" => 1, _ if x.ends_with('\"') => 2, _ => 0 } /* \"Blocked\" */\n\
               }\n\
               fn other() { \"Elsewhere\" }\n";
    let body = fn_body(&strip_comments(src), "target");
    assert_eq!(string_literals(&body), vec!["Real", "Also("]);
    assert_eq!(type_constants(&body), vec!["int4", "jsonb"]);
}

fn mapped() -> Vec<Mapped> {
    let mut out = Vec::new();
    for m in MAPPERS {
        let body = fn_body(&strip_comments(&read(m.file)), m.function);
        let raw = match m.names {
            Names::StringLiterals => string_literals(&body),
            Names::TypeConstants => type_constants(&body),
        };
        assert!(
            !raw.is_empty(),
            "no type names found in `{}` ({})",
            m.function,
            m.file
        );
        out.extend(raw.into_iter().map(|name| Mapped {
            engine: m.engine,
            name: normalize(m.engine, name.trim_end_matches('(')),
            function: m.function,
            file: m.file,
        }));
    }
    out
}

fn names_for(mapped: &[Mapped], engine: Engine) -> BTreeSet<String> {
    mapped
        .iter()
        .filter(|m| m.engine == engine)
        .map(|m| m.name.clone())
        .collect()
}

fn normalize(engine: Engine, name: &str) -> String {
    match engine {
        Engine::ClickHouse => name.to_string(),
        Engine::Postgres => name.replace('"', "").to_ascii_lowercase(),
        Engine::DuckDb => name.to_ascii_uppercase(),
    }
}

/// Every name the zoo's cases for `engine` declare.
fn declared(engine: Engine, zoo: &LoadedZoo) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for case in zoo.zoo.cases(engine) {
        let typ = normalize(engine, case.native_type.trim());
        out.insert(head(&typ));
        if engine == Engine::ClickHouse {
            let inner = unwrap_clickhouse(&typ);
            out.insert(head(inner));
            out.insert(inner.to_string());
        }
    }
    out
}

fn head(typ: &str) -> String {
    typ.split('(').next().unwrap_or(typ).trim().to_string()
}

fn unwrap_clickhouse(mut s: &str) -> &str {
    loop {
        s = s.trim();
        let peeled = ["Nullable(", "LowCardinality("]
            .iter()
            .find_map(|w| s.strip_prefix(w).and_then(|v| v.strip_suffix(')')));
        match peeled {
            Some(inner) => s = inner,
            None => return s,
        }
    }
}

fn covered(name: &str, declared: &BTreeSet<String>, mapped: &BTreeSet<String>) -> bool {
    declared.iter().any(|d| {
        d == name
            || d.strip_prefix(name).is_some_and(|rest| {
                !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()) && !mapped.contains(d)
            })
    })
}

fn read(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

/// `src` without `//` and `/* */` comments; string and char literals copied whole.
fn strip_comments(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < chars.len() {
        let end = match (chars[i], chars.get(i + 1)) {
            ('"', _) => string_end(&chars, i),
            ('\'', _) => char_end(&chars, i),
            ('/', Some('/')) => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            ('/', Some('*')) => {
                let mut j = i + 2;
                while j + 1 < chars.len() && !(chars[j] == '*' && chars[j + 1] == '/') {
                    j += 1;
                }
                i = j + 2;
                continue;
            }
            _ => i + 1,
        };
        out.extend(&chars[i..end]);
        i = end;
    }
    out
}

/// Index just past the string literal that opens at `start`.
fn string_end(chars: &[char], start: usize) -> usize {
    let mut i = start + 1;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 2,
            '"' => return i + 1,
            _ => i += 1,
        }
    }
    chars.len()
}

/// Index just past a char literal at `start` (`'x'`, `'\n'`, `'\''`), or `start + 1` for a
/// lifetime.
fn char_end(chars: &[char], start: usize) -> usize {
    match (chars.get(start + 1), chars.get(start + 2)) {
        (Some('\\'), _) => chars
            .get(start + 3..)
            .and_then(|rest| rest.iter().position(|c| *c == '\''))
            .map_or(chars.len(), |p| start + 4 + p),
        (Some(_), Some('\'')) => start + 3,
        _ => start + 1,
    }
}

/// The body of the first `fn <name>(` in comment-free `src`, braces matched outside literals.
fn fn_body(src: &str, name: &str) -> String {
    let at = src
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("`fn {name}` not found"));
    let chars: Vec<char> = src[at..].chars().collect();
    let open = chars
        .iter()
        .position(|c| *c == '{')
        .expect("a function body");
    let (mut depth, mut i) = (0usize, open);
    while i < chars.len() {
        match chars[i] {
            '"' => i = string_end(&chars, i) - 1,
            '\'' => i = char_end(&chars, i) - 1,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return chars[open..=i].iter().collect();
                }
            }
            _ => {}
        }
        i += 1;
    }
    panic!("unbalanced braces in `fn {name}`")
}

fn string_literals(body: &str) -> Vec<String> {
    let chars: Vec<char> = body.chars().collect();
    let (mut out, mut i) = (Vec::new(), 0);
    while i < chars.len() {
        match chars[i] {
            '"' => {
                let end = string_end(&chars, i);
                let text: String = chars[i + 1..end - 1].iter().collect();
                out.push(text.replace("\\\"", "\""));
                i = end;
            }
            '\'' => i = char_end(&chars, i),
            _ => i += 1,
        }
    }
    out
}

/// `Type::X` constants, not the tail of `TypedDataType::X`.
fn type_constants(body: &str) -> Vec<String> {
    body.match_indices("Type::")
        .filter(|(at, _)| !body[..*at].ends_with(|c: char| c.is_alphanumeric() || c == '_'))
        .map(|(at, m)| {
            body[at + m.len()..]
                .chars()
                .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
                .collect::<String>()
                .to_ascii_lowercase()
        })
        .filter(|name| !name.is_empty())
        .collect()
}
