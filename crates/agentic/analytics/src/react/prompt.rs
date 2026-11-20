//! The system prompt for a react analytics run.
//!
//! No date line: the engine's `system_date_hint` carries today's date, so a
//! line here would say it twice.

use std::fmt::Write;

use super::files::FileKind;

/// Paths listed before the prompt says "use search" for the rest.
const MAX_FILES_LISTED: usize = 200;

const CITATION_RULE: &str = "Cite each result you use as {{result:<id>}}. \
    Mark the one result that answers the question as {{result:<id>|primary}}.";

const TOOL_RULES: &str = "How to work:\n\
    - Look before you query: use search and read to find the views, topics and SQL that cover \
    the question.\n\
    - Prefer the semantic model when a view covers the question; write raw SQL only for what \
    it cannot express.\n\
    - Answer from query results only. Never state a number no result returned.\n\
    - Ask the user only when the context cannot settle an ambiguity.";

pub struct PromptInput<'a> {
    pub instructions: &'a str,
    /// `(name, dialect)` per database.
    pub databases: &'a [(String, String)],
    pub files: &'a [(&'a str, FileKind)],
}

fn kind_label(k: FileKind) -> &'static str {
    match k {
        FileKind::View => "view",
        FileKind::Topic => "topic",
        FileKind::Sql => "sql",
        FileKind::Doc => "doc",
    }
}

fn databases_section(databases: &[(String, String)]) -> String {
    let mut s = String::from("Databases (read-only):\n");
    for (name, dialect) in databases {
        let _ = writeln!(s, "- {name} ({dialect})");
    }
    s
}

fn files_section(files: &[(&str, FileKind)]) -> String {
    let mut s = String::from("Workspace files:\n");
    for (path, kind) in files.iter().take(MAX_FILES_LISTED) {
        let _ = writeln!(s, "- {path} ({})", kind_label(*kind));
    }
    if files.len() > MAX_FILES_LISTED {
        let _ = writeln!(s, "… {} more, use search", files.len() - MAX_FILES_LISTED);
    }
    s
}

pub fn system_prompt(p: PromptInput<'_>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !p.instructions.trim().is_empty() {
        parts.push(p.instructions.trim().to_string());
    }
    parts.push(databases_section(p.databases));
    parts.push(files_section(p.files));
    parts.push(TOOL_RULES.to_string());
    parts.push(CITATION_RULE.to_string());
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(files: &'a [(&'a str, FileKind)], dbs: &'a [(String, String)]) -> PromptInput<'a> {
        PromptInput {
            instructions: "Be brief.",
            databases: dbs,
            files,
        }
    }

    #[test]
    fn prompt_lists_databases_files_and_citation_rule() {
        let dbs = vec![("bird_financial".to_string(), "SQLite".to_string())];
        let files = [("financial/account.view.yml", FileKind::View)];
        let p = system_prompt(input(&files, &dbs));
        assert!(p.starts_with("Be brief."));
        assert!(p.contains("bird_financial"));
        assert!(p.contains("financial/account.view.yml"));
        assert!(p.contains(
            "Cite each result you use as {{result:<id>}}. \
             Mark the one result that answers the question as {{result:<id>|primary}}."
        ));
    }

    #[test]
    fn prompt_caps_the_file_list_at_200() {
        let names: Vec<String> = (0..250).map(|i| format!("f{i:03}.sql")).collect();
        let files: Vec<(&str, FileKind)> =
            names.iter().map(|n| (n.as_str(), FileKind::Sql)).collect();
        let p = system_prompt(input(&files, &[]));
        assert!(p.contains("f199.sql"));
        assert!(!p.contains("f200.sql"));
        assert!(p.contains("… 50 more, use search"));
    }
}
