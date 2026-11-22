# agentic-llm

Unified LLM provider abstraction with token-level streaming and extended thinking support.

## Providers

| Provider | Type | Config |
| ---------- | ------ | -------- |
| Anthropic | `AnthropicProvider` | API key + model name |
| OpenAI | `OpenAiProvider` | API key + model name + optional base URL |
| OpenAI-compatible | `OpenAiCompatProvider` | API key + model + base URL (Ollama, etc.) |

## Key Types

```rust
pub trait LlmProvider: Send + Sync {
    async fn stream(&self, system_prompt, messages, tools, thinking_config)
        -> Result<Stream<Chunk>>;
}

pub struct LlmClient {
    // Wraps a provider with tool-loop orchestration
    pub async fn run_with_tools(&self, ...) -> Result<LlmOutput>;
}

pub enum ThinkingConfig {
    Disabled,
    Adaptive,                      // native 4.6+
    Manual { budget_tokens: u32 }, // native 3.7-4.6
    Effort(ReasoningEffort),       // Low | Medium | High | XHigh(4.7+) | Max
}
```

The variant says what the caller wants; the Anthropic provider converts it to
whatever the target model accepts (`Manual`->`Effort` on 4.7+, `Adaptive`/`Effort`
->`Manual` below 4.6, `xhigh`->`high` below 4.7), warning once per provider. So
pick the variant that expresses intent, not the one the model happens to take.

## Thinking Support

Extended thinking (reasoning) uses opaque encrypted blobs:

- **Must** be passed back verbatim in subsequent tool-use loops
- **Must NOT** cross FSM state boundaries (discarded on state transition)
- The react runtime deliberately persists provider thinking blocks verbatim in `Turn.native` so they survive resume (spec: `internal-docs/agent-engine-runtime.md`, Layer 2)
- Controlled by `ThinkingConfig` from the agent YAML

## Advisor (Anthropic, react only)

`AnthropicProvider::with_advisor(AdvisorConfig)` declares Anthropic's advisor
tool (beta `advisor-tool-2026-03-01`): a stronger model the executor consults
mid-turn. `advisor.rs` carries the tool JSON and the usage parsing.

- Its `server_tool_use` / `advisor_tool_result` blocks come out as
  `ContentBlock::ServerTool` and **must** go back verbatim, in stream order:
  the react fold keeps stream order for any turn holding one.
- `StopReason::PauseTurn` means the advisor call is still pending. The react
  engine resends with the response appended and no user message. The FSM tool
  loop does not, so only react declares an advisor (`.agentic.yml`
  `llm.advisor`).
- Advisor tokens are only in `usage.iterations`; `Usage.advisor` carries them,
  priced at the advisor model's rates in `TurnUsage.cost_usd`, and reported on
  `LlmEnd.advisor_*`. An advisor model the price table does not know (or an
  empty one) is priced at the dearest Claude rate (`claude-opus-4-1`) with a
  warning, a deliberate break from `pricing.rs`'s "unknown gives `None`": the
  react cost cap must over-count, never read the turn as free.
- The advice of current advisor models is encrypted: replay it, never log it.

## Events

`LlmClient::run_with_tools` emits events through `EventStream`:

- `CoreEvent::LlmStart` / `LlmToken` / `LlmEnd` — per HTTP round (each provider.stream() call)
- `CoreEvent::ThinkingStart` / `ThinkingToken` / `ThinkingEnd` — per thinking block
- `CoreEvent::ToolCall` / `ToolResult` — per tool invocation

## Tracing: one span per HTTP round, two vocabularies

`genai.rs` builds the span around every `provider.stream()` call (the three
call sites in `client/`), and only it records the outcome. The span carries
the product store's `oxy.name = "llm.call"` / `oxy.span_type = "llm"` **and**
the OpenTelemetry GenAI conventions (`otel.name = "chat <model>"`,
`gen_ai.provider.name`, `gen_ai.request.model`, `gen_ai.usage.*`,
`gen_ai.response.finish_reasons`, `server.address`, `error.type`), pinned to
`semantic-conventions-genai@94f432d7126f` — a Development-status spec, so an
upstream rename is a deliberate edit here.

- Prompt/completion content (`gen_ai.input.messages`, `gen_ai.system_instructions`,
  `gen_ai.output.messages`) is **always** recorded, tool-call arguments and tool
  results included, capped at 64 KB per attribute. The true output size and any
  tool calls past the budget are separate fields (`oxy.gen_ai.output.bytes`,
  `oxy.gen_ai.output.tool_calls_dropped`) — never a marker inside the capped
  attribute, which the cap would cut away. A failed call also records the
  provider's message on `oxy.error.message`.
- Tenant / conversation / agent come from `LlmClient::with_genai_context`;
  the analytics `BuildContext.genai` and the builder client builder set what
  they know. Absent fields are not recorded.
- A provider says who it is through `LlmProvider::provider_name` / `endpoint`
  (defaults: `"custom"`, none); the three real providers map their host to the
  semconv value (`api.openai.com` → `openai`, `*.openai.azure.com` → `azure.ai.openai`).

## Rules

- Infrastructure crate — may be imported by any domain.
- Does NOT depend on runtime, pipeline, or HTTP.
- Provider selection is done in `agentic-pipeline::platform::ProjectContext::resolve_model()`, not here.
