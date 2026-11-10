//! Automation discovery and search for the agentic analytics domain.
//!
//! `OxyAutomationRunner` implements the [`SubrunRunner`] trait, providing
//! subrun search via fuzzy matching against YAML metadata. Subrun
//! *execution* is handled by the coordinator-worker architecture (via
//! `AutomationStepOrchestrator`), not by this module.
//!
//! [`SubrunRunner`]: agentic_core::subrun::SubrunRunner

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tracing::warn;

use agentic_core::subrun::{SubrunRef, SubrunRunner};

use crate::workspace::WorkspaceContext;

/// Discovers and searches `.procedure.yml` files.
///
/// Wire this into `AnalyticsSolver::with_subrun_runner(Arc::new(runner))`.
///
/// Supply the automation file paths discovered from the agent config's `context`
/// globs via [`with_automation_files`](Self::with_automation_files). When set,
/// `search()` uses these paths directly instead of scanning the project
/// directory via `list_automation_files()`.
pub struct OxyAutomationRunner {
    workspace: Arc<dyn WorkspaceContext>,
    /// Automation file paths resolved from `context` globs at config load time.
    /// When non-empty, `search()` uses these instead of `list_automation_files()`.
    automation_files: Vec<PathBuf>,
}

impl OxyAutomationRunner {
    pub fn new(workspace: Arc<dyn WorkspaceContext>) -> Self {
        Self {
            workspace,
            automation_files: Vec::new(),
        }
    }

    /// Provide the pre-resolved automation file paths from the agent config's
    /// `context` globs. `search()` will use these paths directly.
    pub fn with_automation_files(mut self, files: Vec<PathBuf>) -> Self {
        self.automation_files = files;
        self
    }
}

#[async_trait::async_trait]
impl SubrunRunner for OxyAutomationRunner {
    async fn search(&self, query: &str) -> Vec<SubrunRef> {
        // Only `strip_prefix` arithmetic. A node with no files has nothing to
        // strip, and `make_workspace_relative` returns a relative path
        // unchanged, so the empty root is the identity here.
        let workspace_path = self
            .workspace
            .workspace_path()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();

        // Use context-resolved paths when available; fall back to full project scan.
        let paths = if !self.automation_files.is_empty() {
            self.automation_files.clone()
        } else {
            match self.workspace.list_automation_files().await {
                Ok(p) => p,
                Err(e) => {
                    warn!(error = %e, "OxyAutomationRunner: list_automation_files failed; returning empty search results");
                    return vec![];
                }
            }
        };

        filter_automation_paths(&paths, &workspace_path, query)
    }
}

/// Minimal YAML metadata read from each automation file.
#[derive(serde::Deserialize, Default)]
struct AutomationMeta {
    #[serde(default)]
    description: String,
    #[serde(default)]
    retrieval: RetrievalMeta,
    #[allow(dead_code)]
    #[serde(default)]
    tasks: Vec<AutomationTaskMeta>,
}

/// Minimal task metadata — only the name and type are needed for lifecycle events.
#[allow(dead_code)]
#[derive(serde::Deserialize, Default)]
struct AutomationTaskMeta {
    #[serde(default)]
    name: String,
    #[serde(default, rename = "type")]
    task_type: String,
}

#[derive(serde::Deserialize, Default)]
struct RetrievalMeta {
    /// Phrases that opt this automation *in*: if non-empty, the query must match
    /// at least one phrase, otherwise the automation is excluded.
    #[serde(default)]
    include: Vec<String>,
    /// Phrases that opt this automation *out*: if the query matches any phrase,
    /// the automation is always excluded regardless of other matches.
    #[serde(default)]
    exclude: Vec<String>,
}

/// Minimum Jaro-Winkler similarity for a query token to count as a "match"
/// against a corpus word, and for include-gate phrase tokens to match query
/// tokens.  0.85 tolerates common typos and word-form variants ("store" ↔
/// "stores", "performance" ↔ "performing") while rejecting unrelated words.
const FUZZY_THRESHOLD: f64 = 0.85;

/// Returns `true` when at least one token of `phrase_lower` has a
/// Jaro-Winkler similarity ≥ [`FUZZY_THRESHOLD`] against at least one token
/// in `query_tokens`.  This makes the include gate robust to word-form
/// variation (e.g. "store analysis" matches a query containing "stores").
fn phrase_fuzzy_matches(phrase_lower: &str, query_tokens: &[&str]) -> bool {
    if query_tokens.is_empty() {
        return false;
    }
    phrase_lower.split_whitespace().any(|pt| {
        query_tokens
            .iter()
            .any(|qt| strsim::jaro_winkler(pt, qt) >= FUZZY_THRESHOLD)
    })
}

/// Determines whether `path` is a searchable context file (procedure/workflow/automation or SQL).
fn is_context_file(path: &Path) -> bool {
    let name = path.to_string_lossy();
    name.contains(".procedure.")
        || name.contains(".workflow.")
        || name.contains(".automation.")
        || name.ends_with(".sql")
}

/// Extract the human-readable name and description for a context file.
///
/// For YAML files (automation): parses the `name` and `description` fields.
/// For SQL files: uses the file stem as name and the `oxy:` comment block description
/// (if any) as description.
fn extract_file_meta(path: &Path) -> (String, AutomationMeta) {
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

    if path.extension().is_some_and(|e| e == "sql") {
        // For SQL files, use the file stem (without extension) as the name.
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(file_name)
            .to_string();
        // Extract description from the oxy: comment block if present.
        let description = std::fs::read_to_string(path)
            .ok()
            .and_then(|content| extract_sql_description(&content))
            .unwrap_or_default();
        let meta = AutomationMeta {
            description,
            ..Default::default()
        };
        return (stem, meta);
    }

    // For .procedure.yml / .automation.yml: strip the double extension.
    // (`.workflow.yml` is not a recognised kind, so it never gets here.)
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let name = stem
        .strip_suffix(".procedure")
        .or_else(|| stem.strip_suffix(".automation"))
        .unwrap_or(stem)
        .to_string();
    let meta: AutomationMeta = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_yaml::from_str(&s).ok())
        .unwrap_or_default();
    (name, meta)
}

/// Extract a description string from the `oxy:` comment block in a SQL file.
///
/// Delegates to [`agentic_core::subrun::parse_oxy_comment_block`] so the
/// `oxy:` comment block format has a single source of truth.
fn extract_sql_description(content: &str) -> Option<String> {
    agentic_core::subrun::parse_oxy_comment_block(content).and_then(|b| b.description)
}

/// Convert `path` to a `workspace_path`-relative form. If `path` is
/// already relative, returns it unchanged. If absolute, strips the
/// `workspace_path` prefix when possible; falls back to the original
/// path if the prefix doesn't match (e.g., a symlink resolved
/// differently). The fallback is conservative — downstream validation
/// will reject the still-absolute path with a clear error rather than
/// silently constructing an out-of-workspace reference.
fn make_workspace_relative(path: &Path, workspace_path: &Path) -> PathBuf {
    if path.is_relative() {
        return path.to_path_buf();
    }
    path.strip_prefix(workspace_path)
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|_| path.to_path_buf())
}

fn filter_automation_paths(
    paths: &[PathBuf],
    workspace_path: &Path,
    query: &str,
) -> Vec<SubrunRef> {
    let query_lower = query.to_lowercase();

    // Split the query into lowercase tokens; empty tokens are dropped.
    let tokens: Vec<&str> = query_lower.split_whitespace().collect();

    // Collect all automation/SQL candidates, reading metadata upfront
    // so that description and retrieval config participate in filtering/scoring.
    let mut scored: Vec<(f64, SubrunRef)> = paths
        .iter()
        .filter(|p| is_context_file(p))
        .filter_map(|path| {
            let (name, meta) = extract_file_meta(path);

            // ── retrieval config: hard include / exclude gates ──────────────
            // These mirror the same semantics used across the rest of the Oxy
            // retrieval system (vector store, agents, SQL files, topics).
            if !query_lower.is_empty() {
                // Exclude gate: exact substring match (conservative — only
                // block when the phrase is literally present in the query).
                let excluded = meta
                    .retrieval
                    .exclude
                    .iter()
                    .any(|phrase| query_lower.contains(phrase.to_lowercase().as_str()));
                if excluded {
                    return None;
                }

                // Include gate: fuzzy phrase-token matching (liberal — at
                // least one token of any include phrase must fuzzy-match a
                // query token).  This handles word-form variation such as
                // "store analysis" matching a query that says "stores".
                let includes = &meta.retrieval.include;
                if !includes.is_empty() {
                    let included = includes
                        .iter()
                        .any(|phrase| phrase_fuzzy_matches(&phrase.to_lowercase(), &tokens));
                    if !included {
                        return None;
                    }
                }
            }

            // ── fuzzy token scoring over name + description ─────────────────
            // Build corpus words by splitting on whitespace, underscores, and
            // hyphens so that e.g. "monthly_revenue" → ["monthly", "revenue"].
            // Score = sum of max Jaro-Winkler similarity per query token across
            // all corpus words.  An empty query matches everything with score 0.
            let corpus_raw = format!("{} {}", name, meta.description).to_lowercase();
            let corpus_words: Vec<&str> = corpus_raw
                .split(|c: char| c.is_whitespace() || c == '_' || c == '-')
                .filter(|s| !s.is_empty())
                .collect();

            let token_scores: Vec<f64> = if tokens.is_empty() {
                vec![]
            } else {
                tokens
                    .iter()
                    .map(|t| {
                        corpus_words
                            .iter()
                            .map(|w| strsim::jaro_winkler(t, w))
                            .fold(0.0_f64, f64::max)
                    })
                    .collect()
            };

            let score: f64 = token_scores.iter().sum();

            // Require at least one query token to have a strong fuzzy match
            // (≥ FUZZY_THRESHOLD) so that entirely unrelated automations are
            // excluded even if they accumulate a non-zero sum from many weak
            // partial similarities.
            if !tokens.is_empty() && !token_scores.iter().any(|&s| s >= FUZZY_THRESHOLD) {
                return None;
            }

            // SubrunRef::path is the *workspace-relative* path. The
            // downstream contract — DelegationTarget::Automation's
            // `workflow_ref` + `WorkspaceContext::resolve_automation_yaml` —
            // rejects absolute paths as a containment guard against
            // `..`-traversal, so the LLM-visible identifier we surface
            // here must already be relative. `list_automation_files()`
            // returns absolute paths today, but downstream callers
            // (automation-runner, search tool result) treat the path
            // string as a stable key, not as a filesystem location;
            // making it relative is the correct fix.
            let rel_path = make_workspace_relative(path, workspace_path);

            Some((
                score,
                SubrunRef {
                    name,
                    path: rel_path,
                    description: meta.description,
                },
            ))
        })
        .collect();

    // Sort by descending score so the most relevant automations come first.
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().map(|(_, r)| r).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn make_path(name: &str) -> PathBuf {
        PathBuf::from(format!("/project/{name}"))
    }

    #[test]
    fn non_context_files_are_excluded() {
        let paths = vec![
            make_path("monthly_revenue.automation.yml"),
            make_path("monthly_revenue.agentic.yml"),
            make_path("monthly_revenue.view.yml"),
        ];
        let refs = filter_automation_paths(&paths, Path::new("/project"), "");
        // Only automation files are returned; agent/view files are excluded.
        assert_eq!(refs.len(), 1);
        assert!(refs.iter().all(|r| r.name == "monthly_revenue"));
    }

    #[test]
    fn automation_files_are_included() {
        let paths = vec![make_path("monthly_revenue.automation.yml")];
        let refs = filter_automation_paths(&paths, Path::new("/project"), "");
        assert_eq!(refs.len(), 1);
    }

    #[test]
    fn empty_query_returns_all_automations() {
        let paths = vec![
            make_path("revenue.automation.yml"),
            make_path("churn.automation.yml"),
        ];
        let refs = filter_automation_paths(&paths, Path::new("/project"), "");
        assert_eq!(refs.len(), 2);
    }

    #[test]
    fn query_filters_by_name_token() {
        let paths = vec![
            make_path("monthly_revenue.automation.yml"),
            make_path("churn_rate.automation.yml"),
        ];
        let refs = filter_automation_paths(&paths, Path::new("/project"), "revenue");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, "monthly_revenue");
    }

    #[test]
    fn query_is_case_insensitive() {
        let paths = vec![make_path("Monthly_Revenue.automation.yml")];
        let refs = filter_automation_paths(&paths, Path::new("/project"), "REVENUE");
        assert_eq!(refs.len(), 1);
    }

    #[test]
    fn no_match_returns_empty_vec() {
        let paths = vec![make_path("churn_rate.automation.yml")];
        let refs = filter_automation_paths(&paths, Path::new("/project"), "revenue");
        assert!(refs.is_empty());
    }

    #[test]
    fn query_matches_description_not_name() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("orders.automation.yml");
        let mut f = std::fs::File::create(&file_path).unwrap();
        writeln!(f, "name: orders\ndescription: Monthly revenue breakdown").unwrap();

        // "revenue" is only in the description, not the name
        let refs = filter_automation_paths(&[file_path], dir.path(), "revenue");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].description, "Monthly revenue breakdown");
    }

    #[test]
    fn multi_token_query_scores_and_orders_results() {
        let dir = tempfile::tempdir().unwrap();

        // "sales_summary" matches both tokens: "sales" (name) + "monthly" (description)
        let p1 = dir.path().join("sales_summary.automation.yml");
        let mut f = std::fs::File::create(&p1).unwrap();
        writeln!(f, "name: sales_summary\ndescription: Monthly sales report").unwrap();

        // "churn_rate" matches one token: "monthly" (description only)
        let p2 = dir.path().join("churn_rate.automation.yml");
        let mut f = std::fs::File::create(&p2).unwrap();
        writeln!(f, "name: churn_rate\ndescription: Monthly churn metrics").unwrap();

        let refs = filter_automation_paths(&[p1, p2], dir.path(), "sales monthly");
        assert_eq!(refs.len(), 2);
        // Higher-scoring result (2 tokens matched) comes first
        assert_eq!(refs[0].name, "sales_summary");
        assert_eq!(refs[1].name, "churn_rate");
    }

    #[test]
    fn description_read_from_valid_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("orders.automation.yml");
        let mut f = std::fs::File::create(&file_path).unwrap();
        writeln!(f, "name: orders\ndescription: Order analysis").unwrap();

        let refs = filter_automation_paths(&[file_path], dir.path(), "orders");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].description, "Order analysis");
    }

    // ── retrieval config: include / exclude gates ─────────────────────────────

    #[test]
    fn exclude_phrase_removes_automation() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("revenue.automation.yml");
        let mut f = std::fs::File::create(&file_path).unwrap();
        writeln!(
            f,
            "name: revenue\ndescription: Revenue report\nretrieval:\n  exclude:\n    - apple revenue"
        )
        .unwrap();

        // Query contains the exclude phrase → automation must not appear.
        let refs = filter_automation_paths(&[file_path], dir.path(), "apple revenue data");
        assert!(refs.is_empty());
    }

    #[test]
    fn exclude_phrase_does_not_affect_unrelated_query() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("revenue.automation.yml");
        let mut f = std::fs::File::create(&file_path).unwrap();
        writeln!(
            f,
            "name: revenue\ndescription: Revenue report\nretrieval:\n  exclude:\n    - apple revenue"
        )
        .unwrap();

        // Query does NOT contain the exclude phrase → automation appears normally.
        let refs = filter_automation_paths(&[file_path], dir.path(), "revenue");
        assert_eq!(refs.len(), 1);
    }

    #[test]
    fn include_whitelist_allows_matching_query() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("fruit_sales.automation.yml");
        let mut f = std::fs::File::create(&file_path).unwrap();
        writeln!(
            f,
            "name: fruit_sales\ndescription: Fruit sales data\nretrieval:\n  include:\n    - fruit sales"
        )
        .unwrap();

        let refs = filter_automation_paths(&[file_path], dir.path(), "get fruit sales data");
        assert_eq!(refs.len(), 1);
    }

    #[test]
    fn include_whitelist_blocks_non_matching_query() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("fruit_sales.automation.yml");
        let mut f = std::fs::File::create(&file_path).unwrap();
        writeln!(
            f,
            "name: fruit_sales\ndescription: Fruit sales data\nretrieval:\n  include:\n    - fruit sales"
        )
        .unwrap();

        // Query does not contain any include phrase → excluded.
        let refs = filter_automation_paths(&[file_path], dir.path(), "revenue report");
        assert!(refs.is_empty());
    }

    #[test]
    fn empty_query_bypasses_retrieval_gates() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("fruit_sales.automation.yml");
        let mut f = std::fs::File::create(&file_path).unwrap();
        writeln!(
            f,
            "name: fruit_sales\ndescription: x\nretrieval:\n  include:\n    - fruit sales\n  exclude:\n    - apple"
        )
        .unwrap();

        // Empty query bypasses both gates and returns all automations.
        let refs = filter_automation_paths(&[file_path], dir.path(), "");
        assert_eq!(refs.len(), 1);
    }

    #[test]
    fn include_phrase_matched_as_substring_of_query() {
        // "external factors" must match as a sub-phrase of a longer query,
        // e.g. "sales correlation external factors CPI unemployment".
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir
            .path()
            .join("external-factors-correlation.automation.yml");
        let mut f = std::fs::File::create(&file_path).unwrap();
        writeln!(
            f,
            "name: external_factors_correlation\n\
             description: Correlation analysis\n\
             retrieval:\n  include:\n    - \"external factors\""
        )
        .unwrap();

        let refs = filter_automation_paths(
            &[file_path],
            dir.path(),
            "sales correlation external factors CPI unemployment",
        );
        assert_eq!(refs.len(), 1, "include phrase substring should pass");
    }

    #[test]
    fn real_world_automation_matches_external_factors_query() {
        // Mirrors the demo project's external-factors-correlation.automation.yml
        // and the LLM query that was failing to return results.
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir
            .path()
            .join("external-factors-correlation.automation.yml");
        let mut f = std::fs::File::create(&file_path).unwrap();
        writeln!(
            f,
            "name: external_factors_correlation\n\
             description: |\n  \
               Advanced correlation analysis between external factors (temperature, fuel prices,\n  \
               unemployment, CPI) and sales performance.\n\
             retrieval:\n  include:\n    - \"external factors\"\n    - \"unemployment analysis\""
        )
        .unwrap();

        let refs = filter_automation_paths(
            &[file_path],
            dir.path(),
            "sales correlation external factors CPI unemployment",
        );
        assert_eq!(refs.len(), 1, "automation should be found for this query");
        assert_eq!(refs[0].name, "external-factors-correlation");
    }

    #[test]
    fn missing_description_defaults_to_empty_string() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("orders.automation.yml");
        let mut f = std::fs::File::create(&file_path).unwrap();
        writeln!(f, "name: orders\ntasks: []").unwrap();

        let refs = filter_automation_paths(&[file_path], dir.path(), "");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].description, "");
    }

    #[test]
    fn absolute_input_paths_become_workspace_relative() {
        // `list_automation_files()` returns absolute paths today; the
        // search tool must strip the workspace prefix so the
        // LLM-visible `path` and downstream `DelegationTarget::Automation`
        // are workspace-relative. The containment validator on the
        // worker side rejects absolute `workflow_ref`s.
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("workflows").join("sales.automation.yml");
        std::fs::create_dir_all(file_path.parent().unwrap()).unwrap();
        std::fs::File::create(&file_path).unwrap();

        let refs = filter_automation_paths(&[file_path.clone()], dir.path(), "");
        assert_eq!(refs.len(), 1);
        assert_eq!(
            refs[0].path,
            PathBuf::from("workflows/sales.automation.yml"),
            "absolute input should be stripped to workspace-relative; \
             got {:?}",
            refs[0].path
        );
        assert!(
            refs[0].path.is_relative(),
            "SubrunRef::path must be relative"
        );
    }

    #[test]
    fn relative_input_paths_pass_through_unchanged() {
        // Callers can also feed relative paths directly (e.g. tests
        // using `make_path` with a bare filename) — those stay as-is.
        let refs = filter_automation_paths(
            &[PathBuf::from("workflows/sales.automation.yml")],
            Path::new("/project"),
            "",
        );
        assert_eq!(refs.len(), 1);
        assert_eq!(
            refs[0].path,
            PathBuf::from("workflows/sales.automation.yml")
        );
    }

    #[test]
    fn unrelated_absolute_input_falls_back_to_original() {
        // If the input path doesn't share the workspace prefix
        // (symlink, misconfig), we conservatively keep the original
        // absolute path so the downstream validator rejects it with
        // a clear error rather than silently constructing an
        // out-of-workspace ref.
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("sales.automation.yml");
        std::fs::File::create(&file_path).unwrap();

        let refs = filter_automation_paths(&[file_path.clone()], Path::new("/nowhere/else"), "");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].path, file_path, "fallback should preserve input");
    }

    // ── fuzzy scoring ─────────────────────────────────────────────────────────

    #[test]
    fn fuzzy_query_matches_store_deep_dive_automation() {
        // Mirrors demo_project/workflows/store-deep-dive-analysis.automation.yml.
        // The query uses related terms ("top stores performance") that don't
        // appear verbatim in the include phrases ("store analysis", etc.) or
        // the description, but should still match via Jaro-Winkler fuzzy scoring
        // and the fuzzy include gate.
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("store-deep-dive-analysis.automation.yml");
        let mut f = std::fs::File::create(&file_path).unwrap();
        writeln!(
            f,
            "name: store_deep_dive_analysis\n\
             description: |\n  \
               Comprehensive store-by-store analysis using loop_sequential to iterate through\n  \
               top performing stores and generate detailed individual reports for each.\n  \
               This workflow demonstrates advanced looping capabilities and nested task execution.\n\
             retrieval:\n\
               include:\n\
                 - \"store analysis\"\n\
                 - \"individual store performance\"\n\
                 - \"store deep dive\""
        )
        .unwrap();

        let refs = filter_automation_paths(
            &[file_path],
            dir.path(),
            "top stores performance revenue sales",
        );
        assert_eq!(refs.len(), 1, "automation should be found for this query");
        assert_eq!(refs[0].name, "store-deep-dive-analysis");
    }

    // ── SQL file search ───────────────────────────────────────────────────────

    #[test]
    fn sql_file_is_included_in_search() {
        let paths = vec![
            make_path("monthly_revenue.sql"),
            make_path("churn_rate.automation.yml"),
        ];
        let refs = filter_automation_paths(&paths, Path::new("/project"), "");
        assert_eq!(refs.len(), 2);
    }

    #[test]
    fn sql_file_uses_stem_as_name() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("monthly_revenue.sql");
        std::fs::File::create(&file_path).unwrap();

        let refs = filter_automation_paths(&[file_path], dir.path(), "");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, "monthly_revenue");
    }

    #[test]
    fn sql_file_extracts_description_from_oxy_comment() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("revenue_by_region.sql");
        let mut f = std::fs::File::create(&file_path).unwrap();
        writeln!(
            f,
            "/*\noxy:\n  description: \"Monthly revenue broken down by region\"\n*/\nSELECT 1"
        )
        .unwrap();

        let refs = filter_automation_paths(&[file_path], dir.path(), "");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].description, "Monthly revenue broken down by region");
    }

    #[test]
    fn sql_file_matches_by_name_token() {
        let paths = vec![
            make_path("monthly_revenue.sql"),
            make_path("churn_rate.sql"),
        ];
        let refs = filter_automation_paths(&paths, Path::new("/project"), "revenue");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, "monthly_revenue");
    }
}

// ---------------------------------------------------------------------------
// Tests for extract_steps / step_to_result / text_fallback
