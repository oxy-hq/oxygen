//! Keeps domain-boundaries.md F2 from regrowing: an entity module in this crate
//! must describe a table the central migrator (`crates/migration`) creates and
//! has not since dropped.
//!
//! ## Why a test
//!
//! The F2 rows — `checkpoints`, `runs`, `tasks`, `settings` — were entity files
//! whose table had lost every reader, or whose table a later migration had
//! dropped, and nothing said so: the file compiled, the prelude re-exported it,
//! and the schema and the entity crate drifted apart for months. A drop
//! migration that leaves its entity behind, or an entity added for a table no
//! migration creates, has the same "still compiles" failure mode as the typed
//! column rule next door, so the objection has to be mechanical.
//!
//! ## What it checks
//!
//! For every `pub mod <name>;` in `lib.rs` with a `table_name = "<table>"`:
//!
//! 1. **Named.** Some file in `crates/migration/src/` names the table — as the
//!    literal `"<table>"` (raw SQL, `Alias::new`), or as the `DeriveIden` enum
//!    that renders to it (`enum Tasks { Table, .. }` → `tasks`). An entity for a
//!    table no central migration has heard of is an orphan from day one.
//! 2. **Not dropped since.** The newest migration naming the table (migration
//!    files sort chronologically by name) does not drop it in its `up()`: not
//!    `Table::drop()…table(X::Table)`, not `Table::drop()…table(Alias::new(..))`,
//!    not raw `DROP TABLE`, and not a `&[&str]` list of names a drop loop walks.
//!    A `down()` that recreates it does not count, and neither does a later
//!    migration that only alters it.
//!
//! The reverse direction — a table the migrators leave in the database with no
//! entity — is not judged here. It needs a migrated database to be exact
//! (`crates/app/tests/platform/data_placement.rs` has one and already requires
//! every such table to be listed with a reason), and from source it would be an
//! allowlist of every raw-SQL table, which is the kind of list that only grows.
//!
//! Entities in other crates (`agentic-*`, `airhouse`, `cameras`, `oltp`) ride
//! their own migrators and are out of scope by construction: this crate's
//! tables are the central migrator's, and the test says so.

use std::path::{Path, PathBuf};

/// `crates/entity` -> workspace root.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/entity is two levels below the workspace root")
        .to_path_buf()
}

/// `tasks` -> `Tasks`, `api_token_grants` -> `ApiTokenGrants`: the enum name
/// `DeriveIden` snake-cases back to the table.
fn pascal(table: &str) -> String {
    table
        .split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect()
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `needle` appears in `haystack` with no identifier character on either side,
/// so `tasks` is not found inside `a2a_tasks` or `tasks_ready`.
fn contains_word(haystack: &str, needle: &str) -> bool {
    let mut from = 0;
    while let Some(offset) = haystack[from..].find(needle) {
        let start = from + offset;
        let end = start + needle.len();
        let before_ok = haystack[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !is_ident_char(c));
        let after_ok = haystack[end..]
            .chars()
            .next()
            .is_none_or(|c| !is_ident_char(c));
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

/// The `up()` body: from `fn up(` to `fn down(`, or to the end when there is no
/// `down`. Consts declared above `up` are not in it, which is why the list
/// shape below looks at the whole file for the names and at `up` for the loop.
fn up_body(source: &str) -> &str {
    let Some(start) = source.find("fn up(") else {
        return "";
    };
    match source[start..].find("fn down(") {
        Some(end) => &source[start..start + end],
        None => &source[start..],
    }
}

/// Does this migration's `up()` drop `table`? Covers every spelling in the tree.
fn drops_table(source: &str, table: &str) -> bool {
    let up = up_body(source);
    let flat: String = up.split_whitespace().collect();
    let enum_target = format!(".table({}::Table)", pascal(table));
    let alias_target = format!(".table(Alias::new(\"{table}\"))");
    // Builder chain, possibly `.table(A).table(B)`: judge the chain from
    // `Table::drop()` to its `.to_owned()` or `.await`.
    let mut from = 0;
    while let Some(offset) = flat[from..].find("Table::drop()") {
        let start = from + offset;
        let rest = &flat[start..];
        let end = rest
            .find(".to_owned()")
            .or_else(|| rest.find(".await"))
            .unwrap_or(rest.len());
        let chain = &rest[..end];
        if chain.contains(&enum_target) || chain.contains(&alias_target) {
            return true;
        }
        from = start + 1;
    }
    // Raw SQL: `DROP TABLE [IF EXISTS] "table"` / `DROP TABLE table`.
    let lowered = up.to_ascii_lowercase();
    let words: Vec<&str> = lowered
        .split(|c: char| !is_ident_char(c))
        .filter(|w| !w.is_empty())
        .collect();
    let mut i = 0;
    while i + 2 < words.len() {
        if words[i] == "drop" && words[i + 1] == "table" {
            let mut j = i + 2;
            if words.get(j) == Some(&"if") && words.get(j + 1) == Some(&"exists") {
                j += 2;
            }
            if words.get(j) == Some(&table) {
                return true;
            }
        }
        i += 1;
    }
    // A `&[&str]` list of names that a loop in `up` drops, whichever way it
    // spells the drop (`m20260915_000001_drop_dead_tables`,
    // `m20260421_000001_drop_legacy_slack_tables`).
    let has_drop_loop = flat.contains("Table::drop()") || lowered.contains("drop table");
    let has_name_list = source
        .split_whitespace()
        .collect::<String>()
        .contains("&[&str]=&[");
    has_drop_loop && has_name_list && source.contains(&format!("\"{table}\""))
}

/// Every `pub mod <name>;` in `lib.rs` with its `table_name`, by module.
fn entity_tables(root: &Path) -> Vec<(String, String)> {
    let src = root.join("crates/entity/src");
    let lib = std::fs::read_to_string(src.join("lib.rs")).expect("read crates/entity/src/lib.rs");
    let mut out = Vec::new();
    for line in lib.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("pub mod ") else {
            continue;
        };
        let Some(module) = rest.strip_suffix(';') else {
            continue;
        };
        if module == "prelude" {
            continue;
        }
        let path = src.join(format!("{module}.rs"));
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(table) = table_name(&body) {
            out.push((module.to_string(), table));
        }
    }
    out
}

/// The `table_name = "<name>"` an entity file declares, if any.
fn table_name(body: &str) -> Option<String> {
    let idx = body.find("table_name")?;
    let after = &body[idx + "table_name".len()..];
    let after = after.trim_start().strip_prefix('=')?.trim_start();
    let after = after.strip_prefix('"')?;
    let end = after.find('"')?;
    Some(after[..end].to_string())
}

/// `crates/migration/src/m*.rs`, sorted: the name carries the date, so lexical
/// order is chronological order.
fn migrations(root: &Path) -> Vec<(String, String)> {
    let dir = root.join("crates/migration/src");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("read crates/migration/src")
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "rs")
                && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                    n.starts_with('m') && n.as_bytes().get(1).is_some_and(u8::is_ascii_digit)
                })
        })
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|p| {
            let name = p.file_stem().unwrap().to_string_lossy().into_owned();
            let body = std::fs::read_to_string(&p).expect("read migration");
            (name, body)
        })
        .collect()
}

/// Why an entity module is out of step with the central migrator.
#[derive(Debug, PartialEq, Eq)]
enum Fault {
    /// No central migration names the table.
    Unnamed { module: String, table: String },
    /// The newest migration naming the table drops it.
    Dropped {
        module: String,
        table: String,
        migration: String,
    },
}

fn faults(entities: &[(String, String)], migrations: &[(String, String)]) -> Vec<Fault> {
    let mut out = Vec::new();
    for (module, table) in entities {
        let enum_name = pascal(table);
        let newest = migrations
            .iter()
            .rfind(|(_, body)| contains_word(body, table) || contains_word(body, &enum_name));
        match newest {
            None => out.push(Fault::Unnamed {
                module: module.clone(),
                table: table.clone(),
            }),
            Some((name, body)) if drops_table(body, table) => out.push(Fault::Dropped {
                module: module.clone(),
                table: table.clone(),
                migration: name.clone(),
            }),
            Some(_) => {}
        }
    }
    out
}

#[test]
fn every_entity_describes_a_table_the_central_migrator_still_creates() {
    let root = workspace_root();
    let entities = entity_tables(&root);
    assert!(
        entities.len() > 100,
        "expected to find this crate's entity modules through lib.rs; found {}",
        entities.len()
    );
    let migrations = migrations(&root);
    assert!(
        migrations.len() > 200,
        "expected the central migration chain; found {}",
        migrations.len()
    );

    let found = faults(&entities, &migrations);
    let mut report = String::new();
    for fault in &found {
        match fault {
            Fault::Unnamed { module, table } => report.push_str(&format!(
                "\n  crates/entity/src/{module}.rs: no migration in crates/migration/src names \
                 table `{table}` (as \"{table}\" or `enum {}`). Either the entity is for a table \
                 another migrator owns, which belongs in that crate, or it is an orphan — delete it.",
                pascal(table)
            )),
            Fault::Dropped {
                module,
                table,
                migration,
            } => report.push_str(&format!(
                "\n  crates/entity/src/{module}.rs: `{migration}` drops table `{table}` in its up(). \
                 Delete the entity file, its `pub mod` line and its prelude re-export in the same PR \
                 as the drop (domain-boundaries.md F2).",
            )),
        }
    }
    assert!(
        found.is_empty(),
        "{} entity module(s) out of step with the central migrator:{report}",
        found.len()
    );
}

// ── the detector itself, pinned without touching the tree ─────────────────

const DROP_ENUM: &str = r#"
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Tasks::Table).if_exists().to_owned())
            .await
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.create_table(Table::create().table(Tasks::Table).to_owned()).await
    }
}
enum Tasks { Table, Id }
"#;

#[test]
fn detects_a_builder_drop_by_enum() {
    assert!(drops_table(DROP_ENUM, "tasks"));
    assert!(!drops_table(DROP_ENUM, "a2a_tasks"));
}

#[test]
fn a_recreate_in_down_is_not_a_drop() {
    let create_only = DROP_ENUM.replace(
        "manager\n            .drop_table(Table::drop().table(Tasks::Table).if_exists().to_owned())\n            .await",
        "Ok(())",
    );
    assert!(!drops_table(&create_only, "tasks"));
}

#[test]
fn detects_a_chained_alias_drop() {
    let src = r#"
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("app_preflight_blocks"))
                    .table(Alias::new("app_preflight_refusals"))
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
    "#;
    assert!(drops_table(src, "app_preflight_blocks"));
    assert!(drops_table(src, "app_preflight_refusals"));
    assert!(!drops_table(src, "app_preflight"));
}

#[test]
fn detects_raw_sql_and_name_list_drops() {
    let raw = r#"
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(r"DROP TABLE IF EXISTS "settings"").await?;
        manager.get_connection().execute_unprepared("drop table legacy_two cascade").await
    }
    "#;
    assert!(drops_table(raw, "settings"));
    assert!(drops_table(raw, "legacy_two"));
    assert!(!drops_table(raw, "cascade"));

    let listed = r#"
    const DEAD_TABLES: &[&str] = &["a2a_tasks", "apalis_jobs"];
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let mut drop = Table::drop();
        for table in DEAD_TABLES { drop.table(Alias::new(*table)); }
        manager.drop_table(drop.if_exists().to_owned()).await
    }
    "#;
    assert!(drops_table(listed, "apalis_jobs"));
    assert!(!drops_table(listed, "tasks"));
}

#[test]
fn an_alter_after_the_create_keeps_the_entity_live() {
    let entities = vec![("tasks".to_string(), "tasks".to_string())];
    let migrations = vec![
        (
            "m20250501_000001_add_tasks".to_string(),
            "enum Tasks { Table }".to_string(),
        ),
        (
            "m20250601_000001_widen_tasks".to_string(),
            "fn up() { alter_table(Table::alter().table(Tasks::Table)) }".to_string(),
        ),
    ];
    assert!(faults(&entities, &migrations).is_empty());
}

#[test]
fn a_drop_after_the_create_orphans_the_entity() {
    let entities = vec![("tasks".to_string(), "tasks".to_string())];
    let migrations = vec![
        (
            "m20250501_000001_add_tasks".to_string(),
            "enum Tasks { Table }".to_string(),
        ),
        (
            "m20261108_000300_drop_dead".to_string(),
            DROP_ENUM.to_string(),
        ),
    ];
    assert_eq!(
        faults(&entities, &migrations),
        vec![Fault::Dropped {
            module: "tasks".into(),
            table: "tasks".into(),
            migration: "m20261108_000300_drop_dead".into(),
        }]
    );
}

#[test]
fn an_entity_no_migration_names_is_unnamed() {
    let entities = vec![("ghosts".to_string(), "ghosts".to_string())];
    let migrations = vec![(
        "m20250501_000001_add_tasks".to_string(),
        "enum Tasks { Table }".to_string(),
    )];
    assert_eq!(
        faults(&entities, &migrations),
        vec![Fault::Unnamed {
            module: "ghosts".into(),
            table: "ghosts".into(),
        }]
    );
}

#[test]
fn word_matching_respects_identifier_boundaries() {
    assert!(contains_word("CREATE TABLE tasks (", "tasks"));
    assert!(contains_word("\"tasks\"", "tasks"));
    assert!(!contains_word("a2a_tasks", "tasks"));
    assert!(!contains_word("tasks_ready", "tasks"));
    assert_eq!(pascal("api_token_grants"), "ApiTokenGrants");
}
