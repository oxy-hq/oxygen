//! The engine that writes the answer: an Oxy agent, reading only what it was handed.
//!
//! # What this type is allowed to hold
//!
//! A platform context and a path to an agent config. Deliberately **not** a
//! database connection, an org id, or the caller's standing — the same
//! restraint [`DocumentSearch::answer`] states in its signature, kept here
//! where it would be easiest to break. Everything the model reads arrives as an
//! argument to one call, which is what makes "an engine cannot widen its own
//! candidate set" a fact about the code rather than a rule in a comment.
//!
//! Resolution needs those things, so [`DocumentAgent::resolve`] takes them and
//! keeps none of them.
//!
//! # Why the citations are numbers
//!
//! The prompt labels each document `[1]`, `[2]`, … and asks for those markers
//! back. Asking a model to emit document UUIDs instead invites it to invent
//! one that parses; a marker can only be right or out of range, and an out of
//! range marker is dropped rather than followed. The numbers double as what
//! the reader sees, so a footnote in the text and a card under it are the same
//! object.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use agentic_pipeline::platform::PlatformContext;
use entity::{documents, workspaces};
use oxy_shared::errors::OxyError;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use tracing::warn;
use uuid::Uuid;

use super::search::{Answer, DocumentSearch, MatchMode, PostgresSearch, Turn};
use super::visibility::ReadStanding;

/// Which agent answers document questions.
///
/// An env var with a default rather than a config key, because the fallback is
/// safe: a workspace without this file answers with sources and no prose, and
/// nothing errors. A config key would need a schema change to say the same
/// thing.
const AGENT_REF_ENV: &str = "OXY_DOCUMENT_AGENT";
const AGENT_REF_DEFAULT: &str = "document_librarian.agentic.yml";

/// How much of one document the model is shown.
///
/// A cap in characters, not tokens, because it is enforced against a `String`
/// and an approximate cap that is actually applied beats an exact one that is
/// computed somewhere else. The first pages of an SOP are the SOP; a chapter
/// long enough to be truncated here is one that should have been split.
const BODY_CHARS: usize = 8_000;

pub struct DocumentAgent {
    platform: Arc<dyn PlatformContext>,
    config_path: PathBuf,
    /// Retrieval is the shipped one, unchanged. This type replaces how the
    /// answer is written, never who the candidates are.
    retrieval: PostgresSearch,
}

impl DocumentAgent {
    /// Resolve the org's document agent, or `None` if this deployment has none.
    ///
    /// Every arm that gives up logs at `warn` and returns `None`: the caller's
    /// fallback is a working search with no written answer, and turning "no
    /// agent configured" into a `500` would take the sources away from a reader
    /// to punish an operator.
    pub async fn resolve(db: &DatabaseConnection, org_id: Uuid, caller: Uuid) -> Option<Self> {
        let workspace = org_workspace(db, org_id).await?;
        let project_id = workspace.id;

        let ctx = crate::server::api::custom_apps_gates::build_project_context(
            &workspace, caller, project_id,
        )
        .await
        .map_err(|resp| {
            warn!(
                %org_id, %project_id, status = %resp.status(),
                "could not build a project context to answer from"
            );
        })
        .ok()?;

        let agent_ref =
            std::env::var(AGENT_REF_ENV).unwrap_or_else(|_| AGENT_REF_DEFAULT.to_string());
        let ctx = Arc::new(ctx);
        // Resolved through the config manager so the path is right regardless
        // of CWD and of git-subdirectory workspace layouts — the same call the
        // workspace-health agent probe makes.
        let config_path = match ctx
            .workspace_manager()
            .config_manager
            .resolve_file(&agent_ref)
            .await
        {
            Ok(p) => PathBuf::from(p),
            Err(e) => {
                warn!(%org_id, agent_ref, error = %e, "no document agent in this workspace");
                return None;
            }
        };

        Some(Self {
            platform: ctx,
            config_path,
            retrieval: PostgresSearch,
        })
    }
}

/// The workspace an org's agent configs live in.
///
/// The oldest one. An org with several workspaces has them for its warehouse
/// work, and the library is not per-workspace — it is per-org, which is why
/// `documents` carries an `org_id` and no `workspace_id`. Picking the first
/// makes the choice stable across a workspace being added later; picking "the
/// most recent" would move the agent under an operator who only meant to open
/// a scratch project.
async fn org_workspace(db: &DatabaseConnection, org_id: Uuid) -> Option<workspaces::Model> {
    match workspaces::Entity::find()
        .filter(workspaces::Column::OrgId.eq(org_id))
        .order_by_asc(workspaces::Column::CreatedAt)
        .order_by_asc(workspaces::Column::Id)
        .limit(1)
        .one(db)
        .await
    {
        Ok(Some(w)) => Some(w),
        Ok(None) => {
            warn!(%org_id, "org has no workspace, so no agent can answer for it");
            None
        }
        Err(e) => {
            warn!(%org_id, error = %e, "looking up the org's workspace failed");
            None
        }
    }
}

#[async_trait::async_trait]
impl DocumentSearch for DocumentAgent {
    fn name(&self) -> &'static str {
        "document-agent"
    }

    async fn query(
        &self,
        db: &DatabaseConnection,
        org_id: Uuid,
        caller: Uuid,
        standing: &ReadStanding,
        text: &str,
        mode: MatchMode,
        limit: u64,
    ) -> Result<Vec<documents::Model>, sea_orm::DbErr> {
        self.retrieval
            .query(db, org_id, caller, standing, text, mode, limit)
            .await
    }

    async fn query_folders(
        &self,
        db: &DatabaseConnection,
        org_id: Uuid,
        standing: &ReadStanding,
        text: &str,
        limit: u64,
    ) -> Result<Vec<entity::folders::Model>, sea_orm::DbErr> {
        self.retrieval
            .query_folders(db, org_id, standing, text, limit)
            .await
    }

    async fn answer(
        &self,
        question: &str,
        candidates: &[documents::Model],
        bodies: &HashMap<Uuid, String>,
        history: &[Turn],
    ) -> Result<Option<Answer>, OxyError> {
        let prompt = build_prompt(question, candidates, bodies, history);
        // `run_agentic_answer`, not `run_agentic_eval`. The latter drives the
        // analytics FSM, which refuses to build a solver without a warehouse
        // connector — `no databases configured`, raised before it reads a word
        // of the prompt. Satisfying it would mean handing a documents question
        // the ability to run SQL, which is the one thing this design is built
        // not to do. See the facade's own docs for the split.
        let text =
            agentic_pipeline::run_agentic_answer(self.platform.clone(), &self.config_path, prompt)
                .await
                .map_err(OxyError::AgentError)?;

        let text = text.trim().to_string();
        if text.is_empty() {
            return Ok(None);
        }
        let cited = cited_in_order(&text, candidates);
        Ok(Some(Answer { text, cited }))
    }
}

/// Everything the model is allowed to read, in one string.
///
/// The documents come before the question so the last thing in the prompt is
/// what is being asked, and the instruction to refuse comes before the
/// documents so it is not buried under them.
fn build_prompt(
    question: &str,
    candidates: &[documents::Model],
    bodies: &HashMap<Uuid, String>,
    history: &[Turn],
) -> String {
    let mut p = String::with_capacity(4096);
    p.push_str(
        "Answer the question using ONLY the numbered documents below. They are \
         this company's own operating documents; nothing outside them is in scope.\n\n\
         Rules:\n\
         - Cite the document you used with its marker, like [1], after the claim it supports.\n\
         - If the documents do not contain the answer, say exactly what is missing. \
         Do not fill the gap from general knowledge.\n\
         - Answer directly. Do not ask a clarifying question; answer the question as asked.\n\
         - Be brief. A few sentences.\n\
         - Write plain sentences. No markdown: no **bold**, no headings, no \
         bullet or numbered lists. The answer is rendered as plain text, so a \
         `**` reaches the reader as two asterisks. When the document is a list \
         of steps, write the steps as one sentence in order.\n\n",
    );

    if !history.is_empty() {
        p.push_str("Earlier in this conversation:\n");
        for turn in history {
            p.push_str("Q: ");
            p.push_str(turn.question.trim());
            p.push_str("\nA: ");
            p.push_str(turn.answer.trim());
            p.push_str("\n\n");
        }
        p.push_str(
            "Those earlier answers are context for what is being asked now, not \
             a source. Cite only the documents below.\n\n",
        );
    }

    p.push_str("Documents:\n\n");
    for (i, doc) in candidates.iter().enumerate() {
        p.push_str(&format!("[{}] {}\n", i + 1, doc.title));
        match bodies.get(&doc.id) {
            Some(body) => {
                let body = body.trim();
                match body.char_indices().nth(BODY_CHARS) {
                    Some((cut, _)) => {
                        p.push_str(&body[..cut]);
                        p.push_str("\n… (document continues)\n");
                    }
                    None => {
                        p.push_str(body);
                        p.push('\n');
                    }
                }
            }
            // A file upload's bytes never pass through this server, so its text
            // is genuinely absent. Saying so is better than an empty section
            // the model reads as an empty document.
            None => p.push_str("(no text available — this document is an uploaded file)\n"),
        }
        p.push('\n');
    }

    p.push_str("Question: ");
    p.push_str(question);
    p
}

/// The documents an answer actually cited, in the order it first cited them.
///
/// A marker outside the candidate range is dropped rather than clamped: the
/// model naming `[7]` when it was given four documents has cited nothing, and
/// silently rewriting that to `[4]` would put a real document's name under a
/// claim it does not support.
fn cited_in_order(text: &str, candidates: &[documents::Model]) -> Vec<Uuid> {
    let mut out: Vec<Uuid> = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(open) = bytes[i..].iter().position(|b| *b == b'[') {
        let start = i + open + 1;
        let Some(len) = bytes[start..].iter().position(|b| *b == b']') else {
            break;
        };
        let inner = &text[start..start + len];
        i = start + len + 1;
        let Ok(n) = inner.parse::<usize>() else {
            continue;
        };
        let Some(doc) = n.checked_sub(1).and_then(|k| candidates.get(k)) else {
            continue;
        };
        if !out.contains(&doc.id) {
            out.push(doc.id);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row that never sees a database.
    ///
    /// Written out in full rather than defaulted: `Model` has no `Default`
    /// (`created_at` is a required timestamp), and spelling the columns out
    /// means a schema change arrives here as a compile error rather than as a
    /// silently different fixture.
    fn doc(title: &str) -> documents::Model {
        let now = chrono::Utc::now().fixed_offset();
        documents::Model {
            id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            folder_id: None,
            title: title.into(),
            kind: "chapter".into(),
            status: "published".into(),
            visibility: "org".into(),
            location_id: None,
            pinned_at: None,
            pinned_by: None,
            category_id: None,
            review_status: None,
            reviewed_by: None,
            reviewed_at: None,
            review_note: None,
            expires_at: None,
            current_version_id: None,
            created_by: None,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        }
    }

    #[test]
    fn a_marker_out_of_range_cites_nothing() {
        let docs = vec![doc("a"), doc("b")];
        assert!(cited_in_order("as set out in [7].", &docs).is_empty());
        assert!(cited_in_order("as set out in [0].", &docs).is_empty());
    }

    #[test]
    fn citations_come_back_in_the_order_they_were_used() {
        let docs = vec![doc("a"), doc("b"), doc("c")];
        let cited = cited_in_order("Chill it [2], then log it [1]. Again [2].", &docs);
        assert_eq!(cited, vec![docs[1].id, docs[0].id]);
    }

    /// `Z` because the prompt's own prose is in the same string: the first
    /// version of this counted `x` and was off by one, from "say exactly what
    /// is missing" in the rules.
    #[test]
    fn a_body_longer_than_the_cap_is_cut_and_says_so() {
        let d = doc("long");
        let mut bodies = HashMap::new();
        bodies.insert(d.id, "Z".repeat(BODY_CHARS * 2));
        let p = build_prompt("q", std::slice::from_ref(&d), &bodies, &[]);
        assert!(p.contains("… (document continues)"));
        assert_eq!(p.matches('Z').count(), BODY_CHARS);
    }

    #[test]
    fn a_body_under_the_cap_arrives_whole_and_unmarked() {
        let d = doc("short");
        let mut bodies = HashMap::new();
        bodies.insert(d.id, "Z".repeat(BODY_CHARS - 1));
        let p = build_prompt("q", std::slice::from_ref(&d), &bodies, &[]);
        assert!(!p.contains("… (document continues)"));
        assert_eq!(p.matches('Z').count(), BODY_CHARS - 1);
    }

    /// A file upload has no text on this server, and the prompt has to say so.
    /// An empty section reads to a model as an empty document, which is a
    /// different and wrong claim about somebody's permit.
    #[test]
    fn a_document_with_no_text_says_why_rather_than_going_blank() {
        let d = doc("Health permit 2026");
        let p = build_prompt("q", std::slice::from_ref(&d), &HashMap::new(), &[]);
        assert!(p.contains("[1] Health permit 2026"));
        assert!(p.contains("no text available"));
    }

    /// History is context, never a source — and the prompt has to carry that
    /// distinction, because it is the only place the model can read it.
    #[test]
    fn a_session_arrives_as_context_and_says_it_is_not_a_source() {
        let d = doc("Sanitiser SOP");
        let history = vec![Turn {
            question: "how do I dilute it".into(),
            answer: "One capful to five litres [1].".into(),
            cited: vec![d.id],
        }];
        let p = build_prompt(
            "and how long",
            std::slice::from_ref(&d),
            &HashMap::new(),
            &history,
        );
        assert!(p.contains("how do I dilute it"));
        assert!(p.contains("not\n             a source") || p.contains("not a source"));
        assert!(
            p.rfind("Question: and how long").unwrap() > p.rfind("how do I dilute it").unwrap(),
            "the question being asked now must be the last thing in the prompt"
        );
    }
}
