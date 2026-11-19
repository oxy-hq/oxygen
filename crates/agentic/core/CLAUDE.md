# Agentic-Core Crate

## Architecture

A generic FSM for LLM-powered problem solving, parameterized by domain.

```
  Intent
    │
    ▼
 Clarifying ──► Specifying ──► Solving ──► Executing ──► Interpreting ──► Done
    ▲               ▲             ▲             ▲               ▲
    └───────────────┴─────────────┴─────────────┴───────────────┘
                        Diagnosing (back-edges)
```

## States and What They Carry

Each variant carries the **input** to the current activity, not the output. The orchestrator holds accumulated outputs as local variables.

| State                        | Carries           | Meaning                                                  |
| ---------------------------- | ----------------- | -------------------------------------------------------- |
| `Clarifying(Intent)`         | Partial intent    | Forming a grounded question from user utterance          |
| `Specifying(Intent)`         | Formed intent     | Grounding intent against semantic model into a spec      |
| `Solving(Spec)`              | Grounded spec     | Producing SQL (skipped when semantic model compiled SQL) |
| `Executing(Solution)`        | Candidate SQL     | Running and validating results                           |
| `Interpreting(Result)`       | Validated results | Producing natural language answer                        |
| `Diagnosing { error, back }` | Error + target    | Routing failure to correct recovery state                |
| `Done(Answer)`               | Final answer      | Terminal                                                 |

## Rules

### Orchestrator owns context, workers receive arguments

The enum is thin. The orchestrator holds prior outputs as local variables and passes what each worker needs:

```rust
let intent = clarify(&raw_input, &catalog).await?;
let spec = specify(&intent, &catalog).await?;
let solution = solve(&spec).await?;
let result = execute(&solution).await?;
let answer = interpret(&intent, &spec, &result).await?;
```

No trait chains. No `HasIntent`. No `HasSpec`. Plain structs, plain arguments.

### LLM does not drive transitions

The orchestrator loop is deterministic. Validators decide success/failure. `Diagnosing` routes back-edges. The LLM is only called as a worker within a state.

```
WRONG:  Ask the LLM "what should we do next?"
RIGHT:  Validator fails → enter Diagnosing → diagnose() → BackTarget
```

### Validators are pure functions

No LLM calls inside validators. They take structured data, return `Result<(), Vec<Error>>`. They are the most important code in the system.

### Tools follow Unix philosophy

Each tool does one thing. 1-2 params in, structured data out. The LLM composes them.

```
WRONG:  resolve_and_validate_metric(name, context, options)
RIGHT:  list_metrics(query) -> Vec<MetricSummary>
RIGHT:  get_metric_definition(name) -> MetricDef
```

Tools per state:

```
clarifying:    list_metrics, list_dimensions, get_metric_definition
specifying:    get_valid_dimensions, get_column_range, get_join_path
solving:       explain_plan, dry_run
interpreting:  render_chart
```

### Semantic model hybrid routing

Decision happens in `Specifying`, not `Clarifying`:

```
Specifying tries catalog.try_compile(&intent):
  Ok(sql)          → spec.source = SemanticLayer, skip Solving
  Err(TooComplex)  → spec.source = LlmWithSemanticContext, enter Solving
```

### Dynamic state skipping

Two kinds:

- Static: `SKIP_STATES` — domain never uses this state
- Dynamic: `should_skip(state, data)` — this query doesn't need this state

For analytics, `Solving` is dynamically skipped when `spec.solution_source == SemanticLayer`.

### Back-edges are path-aware

- `Executing` fails on semantic model path → route to `Specifying` (not `Solving`, it was skipped)
- `Executing` fails on LLM path → route to `Solving`
- `Specifying` can retry itself by switching from semantic model to LLM path

### Retries: don't anchor on previous failures

Carry the error and the spec. Consider NOT carrying the failed SQL. Generate fresh from spec + error. See Olausson et al. 2023 in DESIGN.md references.

### Thinking is read-only

Encrypted blobs preserved WITHIN a tool loop only. Never cross state boundaries. Summaries stream as events for display. Never feed back into the system.

The react runtime deliberately persists provider thinking blocks verbatim in `Turn.native` so they survive resume (spec: `internal-docs/agent-engine-runtime.md`, Layer 2).

### Events: token-level streaming

```rust
CoreEvent::LlmStart    { state, prompt_tokens }   // once per HTTP round
CoreEvent::LlmToken    { token }                   // per output token
CoreEvent::LlmEnd      { state, output_tokens, duration_ms }  // once per HTTP round
CoreEvent::ThinkingStart { state }                 // per thinking block
CoreEvent::ThinkingToken { token }                 // per thinking token
CoreEvent::ThinkingEnd   { state }                 // per thinking block
```

`LlmStart`/`LlmEnd` appear once per HTTP round (i.e. per `provider.stream()`
call). When the tool loop runs N rounds, there will be N pairs, each with
per-round token counts and inference-only timing. `ThinkingStart`/`ThinkingEnd`
pairs may appear multiple times within a single round.

## Domain trait

```rust
trait Domain {
    type Intent;
    type Spec;
    type Solution;
    type Result;
    type Answer;
    type Catalog;
    type Error;
}
```

No trait bounds on associated types. Plain structs.

## Build and test

```bash
cargo build
cargo test
```
