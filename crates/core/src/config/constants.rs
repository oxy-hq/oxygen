use std::time::Duration;

pub const EXECUTE_SQL_SOURCE: &str = "execute_sql";
pub const VALIDATE_SQL_SOURCE: &str = "validate_sql";
pub const RETRIEVAL_SOURCE: &str = "retrieval";
pub const TOOL_SOURCE: &str = "tool";
pub const AGENT_SOURCE: &str = "agent";
pub const AGENT_SOURCE_TYPE: &str = "agent_type";
pub const AGENT_SOURCE_PROMPT: &str = "prompt";
pub const AGENT_SOURCE_CONTENT: &str = "content";
pub const AGENT_START_TRANSITION: &str = "start";
pub const AGENT_REVISE_PLAN_TRANSITION: &str = "update_plan";
pub const AGENT_CONTINUE_PLAN_TRANSITION: &str = "follow_plan";
pub const AGENT_FIX_ERROR_TRANSITION: &str = "fix_error";
pub const AGENT_END_TRANSITION: &str = "end";
pub const WORKFLOW_SOURCE: &str = "workflow";
pub const TASK_SOURCE: &str = "task";
pub const ARTIFACT_SOURCE: &str = "artifact";
pub const EVAL_SOURCE_ROOT: &str = "eval_root";
pub const EVAL_SOURCE: &str = "eval";
pub const EVAL_METRICS_POSTFIX: &str = "metrics";
pub const LOOP_VAR_NAME: &str = "value";
pub const DATABASE_SEMANTIC_PATH: &str = ".databases";
pub const SEMANTIC_MODEL_PATH: &str = "models";
pub const CONCURRENCY_SOURCE: &str = "concurrency";
pub const CONCURRENCY_ITEM_ID_PREFIX: &str = "concurrency_item_";
pub const CACHE_SOURCE: &str = "cache";
pub const CONSISTENCY_SOURCE: &str = "consistency";
pub const CONSISTENCY_THRESHOLD: f32 = 0.25;
pub const MARKDOWN_MAX_FENCES: usize = 10;
pub const AGENT_RETRY_MAX_ELAPSED_TIME: Duration = Duration::from_secs(60 * 2);

pub const ENUM_ROUTING_PATH: &str = "enum_routing";
pub const RETRIEVAL_EXCLUSION_BUFFER_MULTIPLIER: f32 = 0.9;
pub const RETRIEVAL_DEFAULT_INCLUSION_RADIUS: f32 = 0.2;
pub const RETRIEVAL_CHILD_INCLUSION_RADIUS: f32 = 0.1;
pub const RETRIEVAL_INCLUSIONS_TABLE: &str = "retrieval_items";
pub const RETRIEVAL_EMBEDDINGS_COLUMN: &str = "embedding";
pub const RETRIEVAL_EMBEDDINGS_BATCH_SIZE: usize = 128;
pub const RETRIEVAL_CACHE_PATH: &str = ".cache";
pub const VECTOR_INDEX_MIN_ROWS: usize = 1000;

/// Default environment variable names for LLM provider API keys.
/// These are the canonical names Oxy uses internally (e.g. routing agent, model defaults).
/// Update here and all callers pick up the change automatically.
pub const OPENAI_API_KEY_VAR: &str = "OPENAI_API_KEY";
pub const ANTHROPIC_API_KEY_VAR: &str = "ANTHROPIC_API_KEY";
pub const GEMINI_API_KEY_VAR: &str = "GEMINI_API_KEY";

/// Default per-vendor model ids used when a model is scaffolded without an
/// explicit `model_ref` — `oxy make`, `oxy init`, and the workspace models
/// settings builder (`oxy-project`). Consolidated here so the default can't
/// silently drift per call-site (which is how `oxy make` was left pinned to a
/// retiring `claude-3-7-sonnet` id while the platform default moved on).
///
/// The Anthropic default is kept in sync with
/// `agentic_llm::constants::DEFAULT_MODEL`; that crate is infrastructure and
/// must not be imported by the platform layer, so the value is mirrored here
/// rather than re-exported.
pub const DEFAULT_ANTHROPIC_MODEL: &str = "claude-opus-4-8";
pub const DEFAULT_OPENAI_MODEL: &str = "gpt-4o";
// `gemini-1.5-pro` is retired/retiring on Google's side; scaffolding it into
// every `oxy make`/`oxy init` Google model reproduced exactly the drift the
// Anthropic pin above warns about. Track a current Gemini 2.x id instead.
pub const DEFAULT_GOOGLE_MODEL: &str = "gemini-2.5-pro";

// Auth-related header constants live in `oxy-auth` so that crate has no `oxy`
// dependency. Re-exported here for source compatibility. There is no JWT
// secret among them any more: sessions are signed with a per-deployment key
// (`oxy_auth::session_key`).
pub use oxy_auth::constants::{AUTHENTICATION_HEADER_KEY, DEFAULT_API_KEY_HEADER};

pub const CONSISTENCY_PROMPT: &str = indoc::indoc! {"
    You are evaluating if two submissions are FACTUALLY CONSISTENT for data analysis purposes.

    **MANDATORY OVERRIDE RULES - READ THIS FIRST:**

    If you see ANY of these, you MUST answer A immediately - DO NOT continue reasoning:
    ✓ Rounding difference < $1 (like $0.33, $0.10, $0.38) → IMMEDIATELY Answer: A
    ✓ One submission includes additional details the other lacks (like 989 weeks, sample sizes, extra context) → IMMEDIATELY Answer: A
    ✓ Grammar/style/formatting differences only → IMMEDIATELY Answer: A

    FORBIDDEN: You must NOT use these phrases or reasoning patterns:
    ✗ however the differing precision
    ✗ not fully consistent
    ✗ minor inconsistency
    ✗ divergence in factual content
    ✗ do not fit into a superset relationship
    ✗ supplemental detail indicates divergence

    CRITICAL: If one submission simply omits information that the other includes, this is NOT a conflict. Answer A.

    [BEGIN DATA]
    ************
    [Question]: {{ task_description }}
    ************
    [Submission 1]: {{ submission_1 }}
    ************
    [Submission 2]: {{ submission_2 }}
    ************
    [END DATA]

    ## EVALUATION RULES

    ### ALWAYS CONSISTENT (Answer: A)

    1. **Rounding differences < $1 or < 0.1%** ← MOST COMMON
       Examples that MUST be marked A:
       * $1,081,396 vs $1,081,395.67 → A (33 cent difference)
       * $1,065,619 vs $1,065,618.90 → A (10 cent difference)
       * $1,005,917 vs $1,005,917.38 → A (38 cent difference)
       * 42.67% vs 42.7% → A (precision difference)
       * ANY difference under $1 → A

    2. **Grammar & Style**
       * \"amounts to\" vs \"amount to\" → A
       * \"There are\" vs \"There're\" → A
       * Synonyms like \"decreased\" vs \"fell\" → A

    3. **Formatting**
       * \"1000\" vs \"1,000\" → A
       * Different date formats → A
       * Whitespace/line breaks → A

    4. **Additional Details (no contradiction)** ← VERY IMPORTANT
       * \"42 users\" vs \"42 users, 18 active\" → A
       * \"$1,081,396\" vs \"$1,081,396 over 989 weeks\" → A
       * \"Sales are $1.08M\" vs \"Sales are $1.08M (average of 989 weeks)\" → A
       * One has more context/statistics than the other → A
       * **CRITICAL: \"Doesn't mention X\" is NOT the same as \"Contradicts X\"**
       * If Submission 2 adds details that Submission 1 lacks → A (CONSISTENT)

    ### ONLY INCONSISTENT (Answer: B) when:

    * Different categorical facts (\"Paris\" vs \"London\" for same location)
    * Material numerical difference (> $10 AND > 10% relative)
    * Contradictory statements (\"increased\" vs \"decreased\")
    * Incompatible conclusions
    * **NOT** inconsistent when one submission simply has MORE details

    ## EVALUATION PROCESS

    Step 1: List differences:

    (1) <category>
    +++ <value_from_submission_1>
    --- <value_from_submission_2>

    Step 2: For EACH difference, check IN ORDER:

    Difference: [describe]
    → Is this rounding (< $1 or < 0.1%)? If YES → STOP, this is CONSISTENT
    → Is this grammar/style? If YES → STOP, this is CONSISTENT
    → Is this formatting? If YES → STOP, this is CONSISTENT
    → Is one submission adding context/statistics the other lacks?
      (Examples: adding week counts, sample sizes, additional breakdowns)
      If YES → STOP, this is ADDITIONAL DETAIL → CONSISTENT
    → Does this CONTRADICT (not just differ in detail level)? If NO → CONSISTENT
    → Is this a material conflict? If YES → FLAG

    Step 3: Final Answer:
    - If ALL differences are rounding/grammar/format → Answer: A
    - If ANY material conflict → Answer: B

    ## EXAMPLES

    [EXAMPLE 1: Multiple Rounding Differences - Answer: A]
    Submission 1: \"Cold: $1,081,396, Moderate: $1,065,619, Hot: $1,005,917\"
    Submission 2: \"Cold: $1,081,395.67, Moderate: $1,065,618.90, Hot: $1,005,917.38\"

    Reasoning:
    (1) Cold temperature: $1,081,396 vs $1,081,395.67
    → Rounding (33¢ < $1)? YES → CONSISTENT

    (2) Moderate temperature: $1,065,619 vs $1,065,618.90
    → Rounding (10¢ < $1)? YES → CONSISTENT

    (3) Hot temperature: $1,005,917 vs $1,005,917.38
    → Rounding (38¢ < $1)? YES → CONSISTENT

    All differences are rounding (< $1). This is CONSISTENT.
    Answer: A

    [EXAMPLE 2: Grammar - Answer: A]
    Submission 1: \"The discount amounts to 15%\"
    Submission 2: \"The discount amount to 15%\"
    → Grammar difference? YES → CONSISTENT
    Answer: A

    [EXAMPLE 3: Additional Details with Week Counts - Answer: A]
    Submission 1: \"Cold temperatures average $1,081,396. Moderate temperatures average $1,065,619.\"
    Submission 2: \"Cold temperatures average $1,081,396 over 989 weeks. Moderate temperatures average $1,065,619 over 3,174 weeks.\"

    Reasoning:
    (1) Cold temperature description:
    Submission 1: \"$1,081,396\"
    Submission 2: \"$1,081,396 over 989 weeks\"
    → Does Submission 2 CONTRADICT Submission 1? NO
    → Does Submission 2 add details? YES → ADDITIONAL DETAIL
    → Additional detail? YES → CONSISTENT

    (2) Moderate temperature description:
    Submission 1: \"$1,065,619\"
    Submission 2: \"$1,065,619 over 3,174 weeks\"
    → Additional detail? YES → CONSISTENT

    Both submissions state the SAME sales figures. Submission 2 simply adds the sample size (week counts). \"Doesn't mention week counts\" ≠ \"Contradicts week counts\". This is ADDITIONAL DETAIL.
    Answer: A

    [EXAMPLE 4: Material Disagreement - Answer: B]
    Submission 1: \"Q4 revenue was $500,000\"
    Submission 2: \"Q4 revenue was $450,000\"
    → Rounding (< $1)? NO ($50,000 difference)
    → Material conflict (10%)? YES → INCONSISTENT
    Answer: B

    Now evaluate the provided submissions. Remember the MANDATORY OVERRIDE RULES at the top.

    Reasoning:
"};
