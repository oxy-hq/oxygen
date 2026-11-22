use super::*;

fn body_for(model: &str, thinking: &ThinkingConfig) -> Value {
    AnthropicProvider::new("key", model).build_request_body(
        "sys",
        "",
        &[],
        &[],
        thinking,
        None,
        None,
    )
}

#[test]
fn max_tokens_reaches_the_request() {
    let body = |cap| {
        AnthropicProvider::new("key", "claude-sonnet-4-6").build_request_body(
            "sys",
            "",
            &[],
            &[],
            &ThinkingConfig::Disabled,
            None,
            cap,
        )
    };
    assert_eq!(body(Some(1234))["max_tokens"], 1234);
    assert_eq!(body(None)["max_tokens"], DEFAULT_MAX_TOKENS);
}

#[test]
fn budget_tokens_classification_tracks_the_version_not_a_model_list() {
    // Still accepted.
    for m in [
        "claude-opus-4-6",
        "claude-sonnet-4-6",
        "claude-haiku-4-5-20251001",
        "claude-3-5-sonnet-20241022",
        // Claude 4.0: a major with NO minor segment, so the release date
        // sits where a minor would. Reading it as one classified these as
        // 4.20250514 -- i.e. "4.7 or later" -- and stripped a field they
        // accept in favour of adaptive thinking they do not support.
        "claude-sonnet-4-20250514",
        "claude-opus-4-20250514",
        "us.anthropic.claude-sonnet-4-20250514-v1:0",
        "claude-opus-4",
    ] {
        assert!(model_accepts_budget_tokens(m), "{m} should accept it");
    }
    // Removed on 4.7 and everything after, including vendor-prefixed and
    // suffixed spellings of the same model.
    for m in [
        "claude-opus-4-7",
        "claude-opus-4-8",
        "claude-sonnet-5",
        "claude-opus-5",
        "claude-fable-5",
        "claude-opus-5[1m]",
        "us.anthropic.claude-opus-4-7-v1:0",
        // A context-marker suffix on the MINOR token. No such id exists
        // yet, but a 1M variant of a 4.x model is the ordinary way this
        // function's "classifies a future model correctly" contract gets
        // tested -- and a whole-token length test would read this as 4.0
        // and hand it the budget_tokens shape 4.7 rejects.
        "claude-opus-4-7[1m]",
    ] {
        assert!(!model_accepts_budget_tokens(m), "{m} should reject it");
    }
    // Unknown ids keep the caller's config rather than guessing.
    assert!(model_accepts_budget_tokens("my-proxy-alias"));
}

#[test]
fn a_dated_claude_4_id_keeps_its_budget_tokens_untranslated() {
    // End to end, not just the classifier: the shape that would have
    // regressed is a working request becoming a 400.
    let body = body_for(
        "claude-sonnet-4-20250514",
        &ThinkingConfig::Manual {
            budget_tokens: 8192,
        },
    );
    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["thinking"]["budget_tokens"], 8192);
    assert!(
        body["output_config"].get("effort").is_none(),
        "a 4.0 model must not be handed adaptive thinking plus effort"
    );
}

#[test]
fn max_tokens_follows_the_thinking_config_actually_sent() {
    // A translated request carries no budget_tokens, so reserving
    // budget + DEFAULT for it would push max_tokens past the output cap
    // and 400 for a different reason than the one being fixed.
    let translated = body_for(
        "claude-sonnet-5",
        &ThinkingConfig::Manual {
            budget_tokens: 60_000,
        },
    );
    assert!(translated["thinking"].get("budget_tokens").is_none());
    assert_eq!(
        translated["max_tokens"], THINKING_MAX_TOKENS,
        "sized from the Effort it became, not the budget it no longer sends"
    );

    // On a model that keeps the budget, the headroom is still reserved.
    let kept = body_for(
        "claude-sonnet-4-20250514",
        &ThinkingConfig::Manual {
            budget_tokens: 60_000,
        },
    );
    assert_eq!(kept["max_tokens"], 60_000 + DEFAULT_MAX_TOKENS);
}

#[test]
fn the_top_two_effort_levels_clamp_at_their_own_boundaries() {
    // `max` shipped on 4.6, `xhigh` on 4.7 -- so 4.6 is the one model where
    // the two answers differ, and clamping both at 4.7 would downgrade a
    // `max` that model accepts.
    let (xhigh, max) = (
        ThinkingConfig::Effort(ReasoningEffort::XHigh),
        ThinkingConfig::Effort(ReasoningEffort::Max),
    );

    assert_eq!(
        body_for("claude-opus-4-6", &xhigh)["output_config"]["effort"],
        "high",
        "xhigh postdates 4.6"
    );
    assert_eq!(
        body_for("claude-opus-4-6", &max)["output_config"]["effort"],
        "max",
        "max ships on 4.6 and must survive"
    );

    // Below 4.6 there is no adaptive thinking at all, so an Effort config
    // becomes a budget rather than a clamped level. Asserting a clamped
    // `effort` here would have read as "4.5 + effort works" while the
    // `thinking` field beside it 400'd.
    let old_model = body_for("claude-opus-4-5", &max);
    assert_eq!(old_model["thinking"]["type"], "enabled");
    assert_eq!(old_model["thinking"]["budget_tokens"], 16_384);
    assert!(old_model["output_config"].get("effort").is_none());

    // Above both.
    assert_eq!(
        body_for("claude-opus-5", &xhigh)["output_config"]["effort"],
        "xhigh"
    );
    assert_eq!(
        body_for("claude-opus-5", &max)["output_config"]["effort"],
        "max"
    );
}

#[test]
fn adaptive_becomes_a_budget_on_a_model_that_predates_it() {
    // The pairing this repo's own template and config example ship:
    // `thinking: adaptive` on a 4.5 triage model. Before the arm existed,
    // `effort` on that model worked and a bare `adaptive` 400'd.
    let old_model = body_for("claude-haiku-4-5", &ThinkingConfig::Adaptive);
    assert_eq!(old_model["thinking"]["type"], "enabled");
    assert_eq!(old_model["thinking"]["budget_tokens"], 8_192);

    // 4.6 is where adaptive arrives, so it must pass straight through.
    let supported = body_for("claude-opus-4-6", &ThinkingConfig::Adaptive);
    assert_eq!(supported["thinking"]["type"], "adaptive");
    assert!(supported["thinking"].get("budget_tokens").is_none());
}

#[test]
fn an_unrecognised_model_id_is_never_rewritten() {
    // Every probe answers "supported" for an id it cannot parse, so a proxy
    // alias or gateway keeps exactly the config the caller wrote. The
    // alternative -- assuming an unknown id is ancient -- silently
    // downgrades a request aimed at a model this code has not heard of.
    let alias = "my-gateway-alias";
    let xhigh = body_for(alias, &ThinkingConfig::Effort(ReasoningEffort::XHigh));
    assert_eq!(xhigh["output_config"]["effort"], "xhigh");
    let max = body_for(alias, &ThinkingConfig::Effort(ReasoningEffort::Max));
    assert_eq!(max["output_config"]["effort"], "max");
    let manual = body_for(
        alias,
        &ThinkingConfig::Manual {
            budget_tokens: 2048,
        },
    );
    assert_eq!(manual["thinking"]["type"], "enabled");
    assert_eq!(manual["thinking"]["budget_tokens"], 2048);

    // The fourth path: a bare `adaptive` must not be converted to a budget
    // just because the id is unparseable.
    let adaptive = body_for(alias, &ThinkingConfig::Adaptive);
    assert_eq!(adaptive["thinking"]["type"], "adaptive");
    assert!(adaptive["thinking"].get("budget_tokens").is_none());
}

#[test]
fn effort_sends_adaptive_thinking_plus_output_config_effort() {
    // The whole point: before this, Effort collapsed to bare Adaptive and
    // the level was discarded, so there was no way to ask for less.
    let body = body_for(
        "claude-sonnet-5",
        &ThinkingConfig::Effort(ReasoningEffort::Low),
    );
    assert_eq!(body["thinking"]["type"], "adaptive");
    assert_eq!(body["output_config"]["effort"], "low");
}

#[test]
fn effort_does_not_clobber_a_structured_output_format() {
    // `output_config` carries `format` for constrained decoding. Assigning
    // the effort object wholesale would silently drop it.
    let schema = ResponseSchema {
        name: "answer".to_string(),
        schema: json!({"type": "object"}),
    };
    let body = AnthropicProvider::new("key", "claude-sonnet-5").build_request_body(
        "sys",
        "",
        &[],
        &[],
        &ThinkingConfig::Effort(ReasoningEffort::Max),
        Some(&schema),
        None,
    );
    assert_eq!(body["output_config"]["effort"], "max");
    assert_eq!(body["output_config"]["format"]["type"], "json_schema");
}

#[test]
fn budget_tokens_is_translated_on_a_model_that_rejects_it() {
    // The reported failure, verbatim: a `budget_tokens` config on sonnet 5
    // returned 400 "thinking.type.enabled is not supported for this
    // model", killing every call the agent made.
    let body = body_for(
        "claude-sonnet-5",
        &ThinkingConfig::Manual {
            budget_tokens: 2048,
        },
    );
    assert_eq!(body["thinking"]["type"], "adaptive");
    assert!(
        body["thinking"].get("budget_tokens").is_none(),
        "the rejected field must not survive translation"
    );
    assert_eq!(body["output_config"]["effort"], "low");
}

#[test]
fn budget_tokens_is_left_alone_on_a_model_that_still_takes_it() {
    let body = body_for(
        "claude-haiku-4-5-20251001",
        &ThinkingConfig::Manual {
            budget_tokens: 8192,
        },
    );
    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["thinking"]["budget_tokens"], 8192);
    assert!(body["output_config"].get("effort").is_none());
}

#[test]
fn effort_gets_thinking_headroom_in_max_tokens() {
    // Effort is adaptive thinking with a depth hint, so it must not fall
    // into the small no-thinking default.
    let effort = body_for(
        "claude-sonnet-5",
        &ThinkingConfig::Effort(ReasoningEffort::Low),
    );
    let adaptive = body_for("claude-sonnet-5", &ThinkingConfig::Adaptive);
    let disabled = body_for("claude-sonnet-5", &ThinkingConfig::Disabled);
    assert_eq!(effort["max_tokens"], adaptive["max_tokens"]);
    assert!(
        effort["max_tokens"].as_u64() > disabled["max_tokens"].as_u64(),
        "thinking needs more headroom than a no-thinking request"
    );
}

#[test]
fn disabled_sends_no_thinking_field() {
    let body = body_for("claude-sonnet-5", &ThinkingConfig::Disabled);
    assert!(body.get("thinking").is_none());
}

#[test]
fn defaults_to_the_public_messages_endpoint() {
    let p = AnthropicProvider::new("key", "claude-sonnet-5");
    assert_eq!(p.messages_url(), "https://api.anthropic.com/v1/messages");
    assert!(p.headers.is_empty());
}

/// Regression guard. `AnthropicModelConfig::api_url` is
/// `#[serde(default)]`-ed to the **root** `https://api.anthropic.com/v1`,
/// so an omitted `api_url` reaches `build_llm_client` as `Some(root)`, not
/// `None` — it always takes the `with_base_url` arm. While this provider
/// POSTed `base_url` verbatim that sent every default Anthropic agent to
/// `/v1` (404). Constructing from the config default must therefore land on
/// exactly the same URL as constructing with no override at all.
#[test]
fn config_default_root_matches_the_no_override_url() {
    let from_config = AnthropicProvider::with_base_url("key", "m", "https://api.anthropic.com/v1");
    let from_default = AnthropicProvider::new("key", "m");
    assert_eq!(from_config.messages_url(), from_default.messages_url());
    assert_eq!(
        from_config.messages_url(),
        "https://api.anthropic.com/v1/messages"
    );
}

#[test]
fn custom_root_gets_messages_appended() {
    let p = AnthropicProvider::with_base_url("key", "m", "https://proxy.internal/v1//");
    assert_eq!(p.messages_url(), "https://proxy.internal/v1/messages");
}

#[test]
fn custom_headers_are_stored() {
    let p = AnthropicProvider::new("key", "m")
        .with_headers(HashMap::from([("x-gw".to_string(), "1".to_string())]));
    assert_eq!(p.headers["x-gw"], "1");
}

/// A tool schema with `minItems: 3` (the legacy specify schema's
/// `join_path`) was sent as-is on a strict tool, and Anthropic answered
/// 400 "For 'array' type, 'minItems' values other than 0 or 1 are not
/// supported" -- for every request carrying it.
fn join_path_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "join_path": {
                "type": "array",
                "items": {"type": "string"},
                "minItems": 3,
                "maxItems": 3
            }
        },
        "required": ["join_path"],
        "additionalProperties": false
    })
}

fn tool(name: &'static str, strict: bool) -> ToolDef {
    ToolDef {
        name,
        description: "d",
        parameters: join_path_schema(),
        strict,
        ..Default::default()
    }
}

fn body_with(tools: &[ToolDef], schema: Option<&ResponseSchema>) -> Value {
    AnthropicProvider::new("key", "claude-opus-4-8").build_request_body(
        "sys",
        "",
        &[],
        tools,
        &ThinkingConfig::Disabled,
        schema,
        None,
    )
}

#[test]
fn strict_tool_schemas_are_reduced_to_the_strict_subset() {
    let body = body_with(&[tool("strict_one", true)], None);
    let jp = &body["tools"][0]["input_schema"]["properties"]["join_path"];
    assert_eq!(jp["minItems"], 1);
    assert!(jp.get("maxItems").is_none());
    assert_eq!(body["tools"][0]["strict"], true);
}

#[test]
fn non_strict_tool_schemas_are_sent_unchanged() {
    // Outside strict mode the API accepts full JSON Schema, so the
    // constraint stays and the model sees it as written.
    let body = body_with(&[tool("loose", false)], None);
    assert_eq!(body["tools"][0]["input_schema"], join_path_schema());
}

#[test]
fn structured_output_schemas_are_reduced_on_both_paths() {
    let schema = ResponseSchema {
        name: "specify_response".to_string(),
        schema: join_path_schema(),
    };
    // No real tools: sent as output_config.format.
    let body = body_with(&[], Some(&schema));
    let jp = &body["output_config"]["format"]["schema"]["properties"]["join_path"];
    assert_eq!(jp["minItems"], 1);
    // With real tools: sent as a synthetic strict tool.
    let body = body_with(&[tool("loose", false)], Some(&schema));
    let synthetic = &body["tools"][1];
    assert_eq!(synthetic["name"], "specify_response");
    assert_eq!(
        synthetic["input_schema"]["properties"]["join_path"]["minItems"],
        1
    );
}
