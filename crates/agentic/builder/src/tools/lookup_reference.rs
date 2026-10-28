//! `lookup_reference` — fetch a domain reference card on demand.
//!
//! The cached system prefix carries only an index of card names plus a
//! short rules summary (see [`crate::prompts`]).  When the agent is
//! about to author or modify a YAML file whose schema it has not yet
//! seen, it calls this tool to load the relevant card body.

use agentic_core::tools::{ToolDef, ToolError};
use serde_json::{Value, json};

use crate::prompts::KnowledgeCard;

pub fn lookup_reference_def() -> ToolDef {
    ToolDef {
        name: "lookup_reference",
        description: "Load a domain reference card for a given Oxygen YAML file type. \
             Returns the full reference body (rules, file shape, examples) plus any \
             associated verbatim templates. Call this before authoring or modifying \
             a `.view.yml`, `.topic.yml`, or `.agentic.yml` \
             file when the relevant card has not already been loaded in this conversation.",
        parameters: json!({
            "type": "object",
            "properties": {
                "card_name": {
                    "type": "string",
                    "enum": [
                        "semantic-layer",
                        "agentic-builder"
                    ],
                    "description": "Which reference card to load. \
                        `semantic-layer` covers .view.yml + .topic.yml; \
                        `agentic-builder` covers .agentic.yml."
                }
            },
            "required": ["card_name"],
            "additionalProperties": false
        }),
        ..Default::default()
    }
}

pub fn execute_lookup_reference(params: &Value) -> Result<Value, ToolError> {
    let card_name = params["card_name"]
        .as_str()
        .ok_or_else(|| ToolError::BadParams("missing 'card_name'".into()))?;

    let card = KnowledgeCard::from_slug(card_name).ok_or_else(|| {
        let supported = KnowledgeCard::all()
            .iter()
            .map(|c| c.slug())
            .collect::<Vec<_>>()
            .join(", ");
        ToolError::BadParams(format!(
            "unknown card_name '{card_name}'. Supported: {supported}"
        ))
    })?;

    Ok(json!({
        "card_name": card.slug(),
        "content": card.content_with_templates(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_reference_def_lists_all_card_slugs() {
        let def = lookup_reference_def();
        let enum_arr = def.parameters["properties"]["card_name"]["enum"]
            .as_array()
            .expect("card_name enum array missing");
        let slugs: Vec<&str> = enum_arr.iter().filter_map(|v| v.as_str()).collect();
        for card in KnowledgeCard::all() {
            assert!(
                slugs.contains(&card.slug()),
                "JSON schema enum missing slug {}",
                card.slug()
            );
        }
        assert_eq!(slugs.len(), KnowledgeCard::all().len());
    }

    #[test]
    fn lookup_reference_returns_known_card_content() {
        // semantic-layer ships with view + topic templates; sentinels
        // pin both the card body and the inlined templates.
        let out = execute_lookup_reference(&json!({"card_name": "semantic-layer"})).unwrap();
        assert_eq!(out["card_name"], "semantic-layer");
        let content = out["content"].as_str().unwrap();
        assert!(content.contains("Semantic model reference"));
        assert!(content.contains("entities:"));
        assert!(content.contains("base_view:"));

        // agentic-builder ships with the agentic-template inlined.
        let out = execute_lookup_reference(&json!({"card_name": "agentic-builder"})).unwrap();
        let content = out["content"].as_str().unwrap();
        assert!(content.contains("Agentic agent reference"));
        assert!(content.contains("states:"));

        // Data Apps are retired: there is no card for authoring one.
        let err = execute_lookup_reference(&json!({"card_name": "app-builder"})).unwrap_err();
        assert!(matches!(err, ToolError::BadParams(_)));
    }

    #[test]
    fn lookup_reference_rejects_unknown_card() {
        let err = execute_lookup_reference(&json!({"card_name": "bogus"})).unwrap_err();
        match err {
            ToolError::BadParams(msg) => assert!(msg.contains("unknown card_name")),
            other => panic!("expected BadParams, got {other:?}"),
        }
    }

    #[test]
    fn lookup_reference_rejects_missing_card_name() {
        let err = execute_lookup_reference(&json!({})).unwrap_err();
        assert!(matches!(err, ToolError::BadParams(_)));
    }

    #[test]
    fn lookup_reference_does_not_leak_provenance_headers() {
        for card in KnowledgeCard::all() {
            let out = execute_lookup_reference(&json!({"card_name": card.slug()})).unwrap();
            let content = out["content"].as_str().unwrap();
            assert!(
                !content.contains("# Vendored from"),
                "vendoring header leaked for card {}",
                card.slug()
            );
            assert!(
                !content.contains("reconciled-at:"),
                "frontmatter leaked for card {}",
                card.slug()
            );
        }
    }
}
