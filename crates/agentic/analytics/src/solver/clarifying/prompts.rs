//! Prompt builders for the Clarifying stage.

use agentic_core::orchestrator::CompletedTurn;

use crate::{AnalyticsDomain, AnalyticsIntent};

use super::super::prompts::{format_history_section, format_session_turns_section};

pub fn build_triage_user_prompt(
    intent: &AnalyticsIntent,
    session_turns: &[CompletedTurn<AnalyticsDomain>],
    topics_section: &str,
) -> String {
    let session_section = format_session_turns_section(session_turns);
    let history_section = format_history_section(&intent.history);
    let topics_hint = if topics_section.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nThe following topics are defined in the semantic model. Use them to guide \
your search_catalog queries — search for terms related to the matching topic's views:\n{topics_section}\n"
        )
    };
    format!(
        "{session_section}{history_section}Question: {raw_question}{topics_hint}\n\n\
Call search_automations first. If no automation matches, call search_catalog to discover \
available measures and dimensions. If search_catalog confirms that ALL needed members \
exist, call propose_semantic_query with the exact view.member paths and a confidence \
score (>= 0.85 for full coverage, lower for partial).\n\n\
If the question requires a measure or dimension that search_catalog could NOT find, \
populate missing_members with one entry per missing concept — include a suggested \
snake_case name, whether it is a \"measure\" or \"dimension\", and a short description \
of what it should represent. Leave missing_members as an empty array when all needed \
members exist in the catalog.",
        raw_question = intent.raw_question,
    )
}
