// Bespoke runtime — drives the browser via vanilla Playwright and decides
// actions through @anthropic-ai/sdk.
//
// Per `act:` step:
//   1. Compute a cache key (flow file + case + step index + step text).
//   2. If cached, replay the recorded action sequence directly. Free.
//   3. On miss (or replay failure), enter an LLM loop:
//        - the model calls browser_snapshot lazily (no pre-capture)
//        - LLM picks a tool, we dispatch + record state-changing tools
//        - state-changing tool results carry a fresh post-action snapshot
//          inline so the model rarely needs a follow-up snapshot call
//        - in-turn snapshot cache: repeat browser_snapshot calls return
//          the last captured tree until a state-changing tool invalidates
//        - repeat until end_turn or max_steps
//      Then persist the recorded sequence so the next run is free.
//
// Prompt caching breakpoints (Anthropic supports up to 4):
//   1. system + tools (ephemeral) — full prefix cached across all steps
//      and all flows that share these tool definitions.
//   2. step prompt (ephemeral) — cached across iterations 2..N of the
//      same step, so a multi-iteration step replays the step prompt at
//      cache-read rate instead of paying full input on every turn.

import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import Anthropic from "@anthropic-ai/sdk";
import { type Browser, chromium, type Page } from "@playwright/test";
import { type ActionCache, createActionCache, type RecordedAction } from "../action-cache";
import { BudgetExceeded, bytesOf, charge, ensurePriced, reserve, worstCaseUsd } from "../budget";
import {
  captureContextOptions,
  closeAndCollectVideo,
  openStartPage,
  takeFinalScreenshot
} from "../capture-profile";
import { executeCase, type RuntimeStepDebug } from "../case-runner";
import { stageHealedActions, writeHealingArtifact } from "../healing";
import { computeCost } from "../pricing";
import { expandSecrets, redactArgs, redactStrategies } from "../secrets";
import {
  isNonDurableRecording,
  isSelectorTool,
  materializeStrategies,
  normalizeSelectorArgs
} from "../selectors";
import { signIn } from "../session";
import { findTool, getGenericTools, isStateChanging } from "../tool-registry";
import {
  addTokens,
  type CaseRunResult,
  emptyTokens,
  type HealingEvent,
  type TokenUsage,
  type ToolCallDebug,
  type ToolDefinition
} from "../types";
import type { Runtime, RuntimeContext } from "./interface";
import { ReplayFailure, replayCachedActions } from "./replay";

const SYSTEM_PROMPT = `You drive an agentic browser test for the Oxy web application.

You will be given a single step from a test flow. Use the available
tools to complete that step, then end your turn.

Workflow:
1. Call browser_snapshot first to see what's on the page. The snapshot
   shows elements by ARIA role, visible label, and any data-testid
   attributes the page exposes.
2. Identify the element you need. Selector preference order:
   a. \`[data-testid=...]\` — most stable, decoupled from copy/i18n/CSS.
      Use this whenever the snapshot shows a testid.
   b. \`role=...[name='...']\` — when no testid exists, ARIA role + name
      is more durable than visible text alone.
   c. \`text=...\` — last resort. Drifts on every label/copy edit.
   When the step text gives you an explicit selector, use it verbatim.
3. Act via browser_click, browser_type, etc. The runtime auto-records
   2–3 fallback selector strategies per state-changing tool, so a flow
   survives single-axis selector drift even if your primary selector
   isn't perfect — pick the single selector you're most confident in
   and the recording layer handles drift resilience.
4. If your action didn't produce the expected effect, snapshot again
   and adjust. Don't keep retrying the same selector.
5. End your turn when the step is done. Do not perform actions beyond
   what the step describes.

Constraints:
- Each step is independent. Don't carry assumptions across steps.
- Avoid speculative actions. If a step is ambiguous, do the most direct
  thing the description implies and stop.
- browser_screenshot is available for visual confirmation but it's
  expensive — prefer browser_snapshot.`;

const __dirname = dirname(fileURLToPath(import.meta.url));
const CACHE_PATH = resolve(__dirname, "..", "..", ".cache", "bespoke-actions.json");
const HEALING_STAGING_PATH = resolve(__dirname, "..", "..", ".cache", "healing-staging.json");
const HEALING_ARTIFACT_PATH = resolve(__dirname, "..", "..", ".results", "healing.json");
const TOOL_RESULT_LIMIT = 16_000;
const TURN_MAX_TOKENS = 1024;

// Haiku pickup → sonnet escalation shipped as infrastructure but is
// **off by default** on these flows: empirically haiku takes ~10× more
// iterations than sonnet for the same step (chat-ask haiku 12 iter vs
// sonnet 1-2; ide-save haiku 20+ iter vs sonnet 5-6), and the per-iter
// haiku savings don't make up for the extra iterations on the cumulative
// conversation context. We keep the escalation machinery in place so a
// future flow that *can* run cheaply on haiku can opt in via a per-flow
// `pickup_model` setting (not yet wired through). For now the loop runs
// only on settings.model.

export const bespokeRuntime: Runtime = {
  name: "bespoke",
  async runCase(ctx: RuntimeContext): Promise<CaseRunResult> {
    const browser = await chromium.launch({
      headless: ctx.headless,
      slowMo: ctx.capture?.slowMoMs
    });
    try {
      return await runWithBrowser(browser, ctx);
    } finally {
      await browser.close();
    }
  }
};

async function runWithBrowser(browser: Browser, ctx: RuntimeContext): Promise<CaseRunResult> {
  // Default to the Oxy backend's public port. The Vite dev server (5173)
  // is also a valid target during dev (it proxies API calls through to
  // 3000), but defaulting to it surprises devs who started oxy directly.
  // Override via OXY_BASE_URL when running against a different port (the
  // cloud-mode flows use 3001 for the auth-disabled internal port).
  const baseURL = process.env.OXY_BASE_URL ?? "http://localhost:3000";
  const context = await browser.newContext({
    baseURL,
    ...(ctx.capture ? captureContextOptions(ctx.capture) : {})
  });
  const recordingSince = Date.now();

  // See session.ts for why the public port needs both the cookie and localStorage.
  const sessionToken = ctx.session?.token ?? process.env.OXY_SESSION_TOKEN;
  const sessionUser = ctx.session?.user ?? process.env.OXY_SESSION_USER;
  if (sessionToken) await signIn(context, baseURL, sessionToken, sessionUser ?? "");

  const page = await context.newPage();

  const tools = getGenericTools();
  const sdkTools = buildSdkTools(tools);
  const client = new Anthropic({ apiKey: ctx.apiKey });
  const cache = createActionCache(ctx.cachePath ?? CACHE_PATH);
  let closed = false;

  try {
    const videoStartMs = ctx.capture ? await openStartPage(page, ctx.capture, recordingSince) : 0;
    const result = await executeCase({
      page,
      context,
      flow: ctx.flow,
      testCase: ctx.testCase,
      apiKey: ctx.apiKey,
      meter: ctx.meter,
      runAct: ({ prompt, stepIndex, step }) =>
        runActStep({
          client,
          step: prompt,
          stepIndex,
          cacheScope: step.cache_scope,
          ctx,
          page,
          cache,
          sdkTools,
          tools
        })
    });
    if (!ctx.capture) return result;
    const screenshot = await takeFinalScreenshot(page, ctx.capture);
    closed = true;
    const video = await closeAndCollectVideo(context, page, ctx.capture);
    return { ...result, capture: { screenshot, video, videoStartMs } };
  } finally {
    if (!closed) await context.close().catch(() => {});
  }
}

interface ActStepInputs {
  client: Anthropic;
  step: string;
  stepIndex: number;
  cacheScope?: "flow" | "shared";
  ctx: RuntimeContext;
  page: Page;
  cache: ActionCache;
  sdkTools: AnthropicToolWithCache[];
  tools: ToolDefinition[];
}

async function runActStep(inputs: ActStepInputs): Promise<RuntimeStepDebug> {
  const { client, step, stepIndex, cacheScope, ctx, page, cache, sdkTools, tools } = inputs;
  const settings = ctx.flow.settings;
  const key = cache.cacheKey(ctx.flow.file, ctx.testCase.name, stepIndex, step, cacheScope);

  const debug: RuntimeStepDebug = {
    iterations: 0,
    model: settings.model,
    tokens: emptyTokens(),
    from_cache: false,
    tool_calls: [],
    snapshot_bytes: 0,
    snapshot_calls: 0,
    snapshot_cache_hits: 0
  };

  if (ctx.cacheMode === "replay") return replayStrict(inputs, key, debug);

  let healingEvent: HealingEvent | undefined;

  if (ctx.cacheMode === "record") {
    debug.last_redrive_reason = "first_run";
  } else if (settings.cache_actions) {
    const cached = cache.get(key);
    if (cached) {
      try {
        const replay = await replayCachedActions({
          cache,
          cacheKey: key,
          actions: cached.actions,
          page,
          tools
        });
        debug.from_cache = true;
        debug.model = undefined;
        debug.tool_calls.push(...replay.tool_calls);
        debug.cache_hit_streak = cached.hit_streak + 1;
        if (replay.drift_events.length > 0) {
          debug.selector_drift_events = replay.drift_events;
        }
        return debug;
      } catch (err) {
        const failure = err instanceof ReplayFailure ? err : null;
        cache.invalidate(key);
        if (ctx.debug) {
          console.warn(`[bespoke] cache replay failed for step '${step}': ${formatErr(err)}`);
        }
        if (failure) {
          healingEvent = await prepareHealingEvent(
            cached.actions,
            failure,
            ctx.flow.name,
            ctx.testCase.name,
            stepIndex,
            step
          );
          debug.last_redrive_reason = "all_strategies_failed";
        } else {
          debug.last_redrive_reason = "selector_mismatch";
        }
      }
    } else {
      debug.last_redrive_reason = "first_run";
    }
  }

  const stepTokens = emptyTokens();
  let outcome: LoopOutcome;
  try {
    outcome = await runLLMLoop({
      client,
      step,
      ctx,
      page,
      sdkTools,
      tools,
      debug,
      model: settings.model,
      tokenSink: stepTokens
    });
  } catch (err) {
    // The meter stopped the step. Hand back what it spent first: thrown
    // onward, case-runner rebuilds the step from nothing and the stop reads as
    // a $0 step with no tool calls — the opposite of what it was. Nothing is
    // recorded either; a sequence cut off by the budget is not one to replay.
    if (!(err instanceof BudgetExceeded)) throw err;
    debug.error = err.message;
    debug.cost_usd = computeCost(settings.model, stepTokens);
    return debug;
  }

  // Persist recordings either to the main cache (normal cold redrive)
  // or to the healing staging file (Tier 2 — UI was redesigned, needs
  // human review before promotion). Skip entirely when nonDurable: an
  // aria-ref-only action (no testid/role/text alternative — typically an
  // unlabeled icon button) can never resolve on replay, since replay never
  // calls browser_snapshot to populate the ref. Caching it would guarantee
  // a ReplayFailure — and, if staged, a healing artifact — on every future
  // run instead of the rare genuine UI-drift case those paths exist for.
  if (ctx.cacheMode === "record") {
    if (outcome.nonDurable) {
      throw new Error(
        `step '${step}': the agent found no stable element to act on (no test id, role name or ` +
          "unique text) — the control may not exist as described, or is an unlabelled icon"
      );
    }
    cache.set(key, outcome.recorded);
    debug.cost_usd = computeCost(settings.model, stepTokens);
    return debug;
  }
  if (outcome.nonDurable && ctx.debug) {
    console.warn(
      `[bespoke] step '${step}' recorded an aria-ref-only action (no durable selector) — skipping cache/healing persistence`
    );
  }
  if (settings.cache_actions && outcome.recorded.length > 0 && !outcome.nonDurable) {
    if (healingEvent) {
      stageHealedActions(HEALING_STAGING_PATH, {
        flow: ctx.flow.name,
        case: ctx.testCase.name,
        step_index: stepIndex,
        cache_key: key,
        actions: outcome.recorded
      });
      const newPrimary = outcome.recorded.find(
        (a, i) => i === healingEvent?.action_index && a.selector_strategies?.[0]?.selector
      );
      const completed: HealingEvent = {
        ...healingEvent,
        new_primary: newPrimary?.selector_strategies?.[0]?.selector,
        new_kind: newPrimary?.selector_strategies?.[0]?.kind
      };
      writeHealingArtifact(HEALING_ARTIFACT_PATH, {
        flow: ctx.flow.name,
        case: ctx.testCase.name,
        step_index: stepIndex,
        action_index: completed.action_index,
        drift: {
          old_primary: completed.old_primary,
          new_primary: completed.new_primary,
          old_kind: completed.old_kind,
          new_kind: completed.new_kind,
          intent: completed.intent
        }
      });
      debug.healed = completed;
    } else {
      cache.set(key, outcome.recorded);
    }
  }

  debug.cost_usd = computeCost(settings.model, stepTokens);
  return debug;
}

/**
 * `cacheMode: "replay"` — the recording is the whole step. Nothing falls back
 * to the model and nothing is invalidated, so a miss is a failure the caller
 * sees rather than a silently re-driven step.
 */
async function replayStrict(
  inputs: ActStepInputs,
  key: string,
  debug: RuntimeStepDebug
): Promise<RuntimeStepDebug> {
  const cached = inputs.cache.get(key);
  if (!cached) throw new Error(`no recording for step '${inputs.step}'`);
  const replay = await replayCachedActions({
    cache: inputs.cache,
    cacheKey: key,
    actions: cached.actions,
    page: inputs.page,
    tools: inputs.tools
  });
  debug.from_cache = true;
  debug.model = undefined;
  debug.tool_calls.push(...replay.tool_calls);
  if (replay.drift_events.length > 0) debug.selector_drift_events = replay.drift_events;
  return debug;
}

async function prepareHealingEvent(
  actions: RecordedAction[],
  failure: ReplayFailure,
  _flow: string,
  _caseName: string,
  _stepIndex: number,
  step: string
): Promise<HealingEvent> {
  const failed = actions[failure.action_index];
  const oldPrimary = failed?.selector_strategies?.[0];
  return {
    action_index: failure.action_index,
    old_primary: oldPrimary?.selector,
    old_kind: oldPrimary?.kind,
    intent: failed?.intent ?? `replay action ${failure.action_index} of step "${step}"`
  };
}

interface LLMLoopInputs {
  client: Anthropic;
  step: string;
  ctx: RuntimeContext;
  page: Page;
  sdkTools: AnthropicToolWithCache[];
  tools: ToolDefinition[];
  debug: RuntimeStepDebug;
  /** Model to drive this loop (PICKUP_MODEL or settings.model). */
  model: string;
  /** Token bucket to accumulate this loop's per-call usage into. The runtime
   * sums per-model buckets to compute accurate USD cost when we escalate.
   * TODO: today only one model runs per step, so `tokenSink` and `debug.tokens`
   * always agree. Drop one or the other once the haiku→sonnet escalation
   * machinery (A.3) actually fires for at least one flow. */
  tokenSink: TokenUsage;
  /** Optional early-abort thresholds. When both maxIter is reached or maxErrors
   * is exceeded AND no state-changing tool has been invoked yet, the loop
   * exits early so the caller can escalate to a stronger model. */
  earlyAbort?: { maxIter: number; maxErrors: number };
}

interface LoopOutcome {
  aborted: boolean;
  stateChanged: boolean;
  recorded: RecordedAction[];
  /** True if any recorded action's only resolvable selector was a raw
   * aria-ref (see `isNonDurableRecording`) — the whole recording is
   * unreplayable and must not be persisted. */
  nonDurable: boolean;
}

interface TurnState {
  /** Sequence of state-changing tool calls observed this turn, in order, for
   * action-cache replay. */
  recorded: RecordedAction[];
  /** See `LoopOutcome.nonDurable`. */
  nonDurable: boolean;
}

async function runLLMLoop(inputs: LLMLoopInputs): Promise<LoopOutcome> {
  const { client, step, ctx, page, sdkTools, tools, debug, model, tokenSink, earlyAbort } = inputs;
  const settings = ctx.flow.settings;

  // Egress boundary #1: expand `${VAR}` placeholders into real secret
  // values just before the step prompt is sent to Anthropic. The
  // unexpanded `step` stays in case-runner.ts:StepDebug.text, which is
  // what we serialize to the result artifact.
  const expandedStep = expandSecrets(step);

  const messages: Anthropic.MessageParam[] = [
    {
      role: "user",
      content: [
        // A.4: cache_control on the step prompt creates a 2nd breakpoint so
        // iterations 2..N of the same step replay [system + tools + step]
        // from cache instead of re-billing input on every turn.
        { type: "text", text: expandedStep, cache_control: { type: "ephemeral" } }
      ]
    }
  ];
  const turn: TurnState = { recorded: [], nonDurable: false };
  const errBaseline = debug.tool_calls.filter((c) => c.error).length;
  let aborted = false;

  ensurePriced(ctx.meter, model);
  const fixedBytes = bytesOf(SYSTEM_PROMPT, JSON.stringify(sdkTools));
  for (let iter = 0; iter < settings.max_steps; iter++) {
    if (ctx.meter) {
      const inputBytes = fixedBytes + bytesOf(JSON.stringify(messages));
      reserve(ctx.meter, worstCaseUsd(model, inputBytes, TURN_MAX_TOKENS));
    }
    const res = await client.messages.create({
      model,
      max_tokens: TURN_MAX_TOKENS,
      system: [{ type: "text", text: SYSTEM_PROMPT, cache_control: { type: "ephemeral" } }],
      tools: sdkTools as unknown as Anthropic.Tool[],
      messages
    });

    debug.iterations++;
    const turnTokens: TokenUsage = {
      input: res.usage.input_tokens,
      cached_input: res.usage.cache_read_input_tokens ?? 0,
      cache_creation: res.usage.cache_creation_input_tokens ?? 0,
      output: res.usage.output_tokens
    };
    addTokens(debug.tokens, turnTokens);
    addTokens(tokenSink, turnTokens);
    charge(ctx.meter, computeCost(model, turnTokens));

    if (ctx.debug) logTurn(res, iter, model);

    if (res.stop_reason === "end_turn") break;

    const toolUses = res.content.filter((b): b is Anthropic.ToolUseBlock => b.type === "tool_use");
    if (toolUses.length === 0) break;

    messages.push({ role: "assistant", content: res.content });

    const results: Anthropic.ToolResultBlockParam[] = [];
    for (const use of toolUses) {
      const args = (use.input ?? {}) as Record<string, unknown>;
      const resultJson = await dispatchInLoop(use.name, args, page, tools, turn, debug);
      results.push({ type: "tool_result", tool_use_id: use.id, content: resultJson });
    }
    messages.push({ role: "user", content: results });

    // A.3: only abort if state hasn't changed yet — otherwise the page is
    // already partially mutated and a restart would re-do parts of the work
    // (or worse, do them twice).
    if (earlyAbort && turn.recorded.length === 0) {
      const errs = debug.tool_calls.filter((c) => c.error).length - errBaseline;
      const overIter = iter + 1 >= earlyAbort.maxIter;
      const overErr = errs > earlyAbort.maxErrors;
      if (overIter || overErr) {
        aborted = true;
        break;
      }
    }
  }

  return {
    aborted,
    stateChanged: turn.recorded.length > 0,
    recorded: turn.recorded,
    nonDurable: turn.nonDurable
  };
}

/**
 * Dispatch a single tool call inside the LLM loop and capture per-tool debug
 * data. State-changing tool calls are recorded for action-cache replay.
 */
async function dispatchInLoop(
  name: string,
  rawArgs: Record<string, unknown>,
  page: Page,
  tools: ToolDefinition[],
  turn: TurnState,
  debug: RuntimeStepDebug
): Promise<string> {
  const start = Date.now();
  // Rewrite a snapshot ref (`ref=f1e14` / `[ref=f1e14]`) to Playwright's
  // real `aria-ref=` engine before anything downstream sees it — both the
  // live Playwright dispatch and the action-cache recording below need the
  // selector that actually resolves. See selectors.ts for why the model
  // emits these in the first place.
  const args = normalizeSelectorArgs(rawArgs);
  // Egress boundary #2: Playwright sees the literal `args` (with secret
  // values inlined by the LLM); the redacted copy is what we stash on
  // disk + in the result artifact. State-changing tools are recorded
  // redacted so the action cache stays free of plaintext secrets.
  const redactedArgs = redactArgs(args);
  const call: ToolCallDebug = { name, ms: 0, args: redactedArgs };
  let result: unknown;

  try {
    const tool = findTool(tools, name);
    if (!tool) throw new Error(`unknown tool: ${name}`);
    // Read the target's strategies BEFORE acting too. After a click that
    // closes its dialog — every form's submit button — the element is gone
    // and a snapshot ref has gone stale, so the post-action read below finds
    // nothing and the recording was dropped as unreplayable.
    // Read with the real args (a redacted selector names nothing on the page)
    // and redacted on the way out, the same boundary args cross.
    const before =
      isStateChanging(name) && isSelectorTool(name)
        ? redactStrategies(await materializeStrategies(page, name, args).catch(() => []))
        : [];
    result = await tool.invoke(args, page);

    if (name === "browser_snapshot") {
      const snap = (result as { snapshot?: string }).snapshot ?? "";
      debug.snapshot_calls++;
      debug.snapshot_bytes += snap.length;
    }

    if (isStateChanging(name)) {
      const recorded: RecordedAction = { tool: name, args: redactedArgs };
      // Materialize 2–3 ranked fallback selectors for every selector
      // tool. Done post-success so the resolved DOM reflects the
      // tool's effect — testid attributes the LLM didn't write but
      // the page exposes are visible at this point. Best-effort: if
      // anything throws here, we keep the single-selector recording.
      if (isSelectorTool(name)) {
        const primary = (redactedArgs.selector ?? redactedArgs.element) as string | undefined;
        try {
          const strategies = redactStrategies(await materializeStrategies(page, name, args));
          if (strategies.length > 0) recorded.selector_strategies = strategies;
        } catch {
          // ignore — single-selector recording is still useful
        }
        // Post-action wins when it found something durable; the pre-action
        // read is the fallback for a target the action itself removed.
        if (
          isNonDurableRecording(primary, recorded.selector_strategies) &&
          !isNonDurableRecording(primary, before)
        ) {
          recorded.selector_strategies = before;
        }
        if (isNonDurableRecording(primary, recorded.selector_strategies)) {
          turn.nonDurable = true;
        }
      }
      turn.recorded.push(recorded);
    }
  } catch (err) {
    call.error = formatErr(err);
    result = { error: call.error };
  }

  call.ms = Date.now() - start;
  debug.tool_calls.push(call);
  return JSON.stringify(result).slice(0, TOOL_RESULT_LIMIT);
}

interface AnthropicToolWithCache {
  name: string;
  description: string;
  input_schema: ToolDefinition["inputSchema"];
  cache_control?: { type: "ephemeral" };
}

function buildSdkTools(tools: ToolDefinition[]): AnthropicToolWithCache[] {
  return tools.map((t, idx) => ({
    name: t.name,
    description: t.description,
    input_schema: t.inputSchema,
    ...(idx === tools.length - 1 ? { cache_control: { type: "ephemeral" as const } } : {})
  }));
}

function logTurn(res: Anthropic.Message, iter: number, model: string): void {
  const tools = res.content
    .filter((b) => b.type === "tool_use")
    .map((b) => (b as Anthropic.ToolUseBlock).name)
    .join(", ");
  const text = res.content
    .filter((b) => b.type === "text")
    .map((b) => (b as Anthropic.TextBlock).text)
    .join(" ")
    .slice(0, 200);
  console.log(
    `[bespoke turn ${iter} model=${model}] tools=[${tools}] stop=${res.stop_reason} text=${JSON.stringify(text)}`
  );
}

function formatErr(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
