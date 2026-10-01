// Shared case orchestration used by both bespoke and Stagehand runtimes.
//
// Each runtime opens its own browser/page (Stagehand v2.5 manages its own
// browser, while bespoke uses vanilla Playwright). Once a Page exists, the
// per-case flow is the same: setup → steps → expectations.

import type { BrowserContext, Page } from "@playwright/test";
import { runSetup } from "../fixtures/reset";
import { applyPathPrefix } from "./backend";
import { type CostMeter, charge, ensurePriced, reserve, worstCaseUsd } from "./budget";
import { evaluateExpectations } from "./judge";
import { computeCost } from "./pricing";
import { runWaitFor } from "./tool-registry";
import {
  addTokens,
  type CaseRunResult,
  type ExpectResult,
  emptyJudgeUsage,
  emptyTokens,
  type FlowCase,
  type FlowSettings,
  type FlowStep,
  type HealingEvent,
  type RedriveReason,
  type SelectorDriftEvent,
  type StepDebug,
  type TokenUsage,
  type ToolCallDebug
} from "./types";

/**
 * Subset of `StepDebug` that the runtime fills in for an `act:` step. The
 * shared case-runner adds the wrapping fields (step_index, kind, text,
 * duration_ms, cost_usd).
 */
export interface RuntimeStepDebug {
  iterations: number;
  model?: string;
  tokens: TokenUsage;
  from_cache: boolean;
  tool_calls: ToolCallDebug[];
  snapshot_bytes: number;
  snapshot_calls: number;
  snapshot_cache_hits: number;
  escalated?: boolean;
  initial_model?: string;
  /**
   * Optional pre-computed cost. Use when the step ran on multiple models
   * (e.g. haiku pickup with sonnet escalation) and a single
   * computeCost(model, tokens) would incorrectly price the per-model
   * token splits.
   */
  cost_usd?: number;
  cache_hit_streak?: number;
  last_redrive_reason?: RedriveReason;
  selector_drift_events?: SelectorDriftEvent[];
  healed?: HealingEvent;
  error?: string;
}

// A judge call's input for `worstCaseUsd`, which reads two bytes as a token:
// judge.ts sends at most 8,000 chars of page text (at most ~8k tokens, even as
// non-ASCII), its prompt and the claim (~0.5k), and one viewport screenshot
// (~2k image tokens) — a ~10.5k-token ceiling, written here as its byte count.
export const JUDGE_INPUT_BYTES = 21_000;
export const JUDGE_MAX_TOKENS = 256;

interface ActStepInput {
  prompt: string;
  stepIndex: number;
  step: FlowStep;
}

export interface CaseRunInputs {
  page: Page;
  context?: BrowserContext;
  flow: { name: string; file: string; setup: string[]; settings: FlowSettings };
  testCase: FlowCase;
  apiKey: string;
  /**
   * Runtime-specific implementation of an `act:` natural-language step.
   * Returns runtime-side debug data; case-runner wraps it with timing and
   * cost. Throw to signal step failure.
   */
  runAct: (input: ActStepInput) => Promise<RuntimeStepDebug>;
  /** Spend limit; the judge is checked against it before it runs and charged after. */
  meter?: CostMeter;
}

export async function executeCase(inputs: CaseRunInputs): Promise<CaseRunResult> {
  const { page, context, flow, testCase, apiKey } = inputs;
  const tokens: TokenUsage = emptyTokens();
  const cacheHits: boolean[] = [];
  const stepDebug: StepDebug[] = [];
  const start = Date.now();

  if (context && flow.settings.trace !== "never") {
    await context.tracing.start({ screenshots: true, snapshots: true }).catch(() => {});
  }

  let stepCount = 0;
  let stepError: string | undefined;
  let expectResults: ExpectResult[] = [];
  let judgeUsage = emptyJudgeUsage();

  try {
    await runSetup(flow.setup, {
      goto: async (url) => {
        await page.goto(applyPathPrefix(url));
      }
    });

    for (let i = 0; i < testCase.steps.length; i++) {
      const step = testCase.steps[i];
      stepCount++;
      const debug = await executeStep(step, i, page, inputs.runAct, flow.settings);
      stepDebug.push(debug);
      addTokens(tokens, debug.tokens);
      if (debug.kind === "act") cacheHits.push(debug.from_cache);
      if (debug.error) {
        stepError = debug.error;
        break;
      }
    }

    if (!stepError) {
      ensurePriced(inputs.meter, flow.settings.judge_model);
      const judgeCalls = testCase.expect.filter((e) => e.judge).length;
      reserve(
        inputs.meter,
        judgeCalls * worstCaseUsd(flow.settings.judge_model, JUDGE_INPUT_BYTES, JUDGE_MAX_TOKENS)
      );
      const judged = await evaluateExpectations(page, testCase.expect, {
        apiKey,
        model: flow.settings.judge_model
      });
      expectResults = judged.results;
      judgeUsage = judged.usage;
      charge(inputs.meter, judged.usage.cost_usd);
    }
  } catch (err) {
    stepError = err instanceof Error ? err.message : String(err);
  }

  const stepsPassed = !stepError;
  const expectsPassed = expectResults.every((e) => e.passed);
  const passed = stepsPassed && expectsPassed;

  let tracePath: string | undefined;
  if (context) {
    const shouldKeep =
      flow.settings.trace === "always" || (!passed && flow.settings.trace !== "never");
    if (shouldKeep) {
      tracePath = traceFile(flow.name, testCase.name);
      await context.tracing.stop({ path: tracePath }).catch(() => {});
    } else {
      await context.tracing.stop().catch(() => {});
    }
  }

  const cost_usd = stepDebug.reduce((s, d) => s + d.cost_usd, 0) + judgeUsage.cost_usd;

  return {
    passed,
    duration_ms: Date.now() - start,
    step_count: stepCount,
    tokens,
    cache_hits: cacheHits,
    expect_results: expectResults,
    step_debug: stepDebug,
    judge_usage: judgeUsage,
    cost_usd,
    trace_path: tracePath,
    error: stepError
  };
}

async function executeStep(
  step: FlowStep,
  stepIndex: number,
  page: Page,
  runAct: CaseRunInputs["runAct"],
  settings: FlowSettings
): Promise<StepDebug> {
  const start = Date.now();

  if (step.wait_for) {
    let error: string | undefined;
    try {
      await runWaitFor(page, step.wait_for);
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    }
    return {
      step_index: stepIndex,
      kind: "wait_for",
      text: step.wait_for,
      duration_ms: Date.now() - start,
      iterations: 0,
      tokens: emptyTokens(),
      cost_usd: 0,
      from_cache: false,
      tool_calls: [],
      snapshot_bytes: 0,
      snapshot_calls: 0,
      snapshot_cache_hits: 0,
      error
    };
  }

  if (step.act) {
    let runtime: RuntimeStepDebug;
    try {
      runtime = await runAct({ prompt: step.act, stepIndex, step });
    } catch (err) {
      runtime = {
        iterations: 0,
        tokens: emptyTokens(),
        from_cache: false,
        tool_calls: [],
        snapshot_bytes: 0,
        snapshot_calls: 0,
        snapshot_cache_hits: 0,
        error: err instanceof Error ? err.message : String(err)
      };
    }
    const model = runtime.model ?? settings.model;
    const cost_usd = runtime.cost_usd ?? computeCost(model, runtime.tokens);
    return {
      step_index: stepIndex,
      kind: "act",
      text: step.act,
      duration_ms: Date.now() - start,
      iterations: runtime.iterations,
      model: runtime.model,
      tokens: runtime.tokens,
      cost_usd,
      from_cache: runtime.from_cache,
      tool_calls: runtime.tool_calls,
      snapshot_bytes: runtime.snapshot_bytes,
      snapshot_calls: runtime.snapshot_calls,
      snapshot_cache_hits: runtime.snapshot_cache_hits,
      escalated: runtime.escalated,
      initial_model: runtime.initial_model,
      cache_hit_streak: runtime.cache_hit_streak,
      last_redrive_reason: runtime.last_redrive_reason,
      selector_drift_events: runtime.selector_drift_events,
      healed: runtime.healed,
      error: runtime.error
    };
  }

  throw new Error("step has no act/wait_for");
}

function traceFile(flowName: string, caseName: string): string {
  const safe = (s: string) => s.replace(/[^a-z0-9]+/gi, "-").toLowerCase();
  return `tests/agentic/.traces/${safe(flowName)}-${safe(caseName)}.zip`;
}
