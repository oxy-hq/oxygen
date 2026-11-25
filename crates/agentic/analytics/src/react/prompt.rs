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

const LOOK_RULE: &str = "- Look before you query: use search and read to find the views, \
    topics and SQL that cover the question.";

const SOURCE_RULE: &str = "- Prefer the semantic model when a view covers the question; write \
    raw SQL only for what it cannot express.";

/// For a `semantic_only` agent, whose `query` tool refuses raw `sql`.
const SEMANTIC_ONLY_RULE: &str = "- Query through the semantic model only: `query` refuses raw \
    `sql`. When the views cannot express the question, say so and answer what they can.";

const ANSWER_RULES: &str = "- Answer from query results only. Never state a number no result \
    returned.\n\
    - Compute final figures (averages, ratios, counts, rounding) in a query when it can \
    express them, not by hand in the answer.\n\
    - When the question limits what counts (only some months, records or rows), look at the \
    rows that limit applies to before answering.\n\
    - Ask the user only when the context cannot settle an ambiguity.\n\
    - Call `chart` only when the user asks for a chart, or when the answer is a trend over \
    time or a comparison across a few categories. Never chart a single number.";

/// For an agent with an `llm.advisor`. Sonnet-class models cannot be forced
/// to call it, so the prompt says when. Only when stuck: on BIRD a call on
/// every question bought nothing, because Sonnet alone answered ~90% correctly
/// and the advisor then only added cost or a wrong turn.
const ADVISOR_RULE: &str = "- Call the advisor only when one of these is true:\n  \
    - A query returned no rows, an error, or a number that conflicts with another result.\n  \
    - You ran 3 queries and you still cannot answer.\n  \
    - The question has two readings and the views do not tell you which one is correct.\n\
    - Do not call the advisor for a direct lookup.";

/// Added by the loop to a `query` result after the third query, or at the
/// first failed one, when the agent has an advisor (`ReactAnalytics::nudge`).
pub(crate) const ADVISOR_NUDGE: &str = "[Note from the runtime: you ran 3 queries, or a query \
    failed. Unless these results already answer the question, call the advisor before your \
    next query.]";

fn tool_rules(semantic_only: bool, advisor: bool) -> String {
    let source = if semantic_only {
        SEMANTIC_ONLY_RULE
    } else {
        SOURCE_RULE
    };
    let mut rules = format!("How to work:\n{LOOK_RULE}\n{source}\n{ANSWER_RULES}");
    if advisor {
        rules.push('\n');
        rules.push_str(ADVISOR_RULE);
    }
    rules
}

pub struct PromptInput<'a> {
    pub instructions: &'a str,
    /// `(name, dialect)` per database.
    pub databases: &'a [(String, String)],
    pub files: &'a [(&'a str, FileKind)],
    /// The agent's `semantic_only`: the rules say raw `sql` is refused.
    pub semantic_only: bool,
    /// The agent has an advisor: the rules say when to call it.
    pub advisor: bool,
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
    parts.push(tool_rules(p.semantic_only, p.advisor));
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
            semantic_only: false,
            advisor: false,
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

    #[test]
    fn semantic_only_prompt_says_sql_is_refused() {
        let plain = system_prompt(input(&[], &[]));
        assert!(plain.contains("write raw SQL only for what it cannot express"));
        assert!(!plain.contains("refuses raw `sql`"));
        let mut i = input(&[], &[]);
        i.semantic_only = true;
        let p = system_prompt(i);
        assert!(p.contains("`query` refuses raw `sql`"));
        assert!(!p.contains("write raw SQL"));
        assert!(p.contains("Never state a number no result returned."));
        assert!(p.contains("not by hand in the answer"), "{p}");
        assert!(p.contains("look at the rows that limit applies to"), "{p}");
    }

    #[test]
    fn the_advisor_rule_appears_only_with_an_advisor() {
        let plain = system_prompt(input(&[], &[]));
        assert!(!plain.contains("Call the advisor"));
        let mut i = input(&[], &[]);
        i.advisor = true;
        let p = system_prompt(i);
        assert!(p.contains("Call the advisor only when"), "{p}");
        assert!(
            p.contains("Do not call the advisor for a direct lookup"),
            "{p}"
        );
        assert!(!p.contains("before your first query"), "{p}");
    }
}
