// Multi-strategy cache replay. Walks each `RecordedAction`'s
// `selector_strategies` by rank; first that resolves wins. Tier-1
// silent re-rank: if a non-zero-rank strategy wins, the cache entry's
// strategy ranks for that action are updated in place.
//
// On total failure (every strategy fails for any one action), throws a
// `ReplayFailure` carrying the failure index + the partial drift events
// observed before the failure. The caller decides whether to invalidate
// and trigger Tier 2 healing redrive.

import type { Page } from "@playwright/test";
import type { ActionCache, RecordedAction, SelectorStrategy } from "../action-cache";
import { expandArgs, expandStrategies, redactSecrets } from "../secrets";
import { dispatchWithFallbacks, isSelectorTool } from "../selectors";
import { findTool } from "../tool-registry";
import type { SelectorDriftEvent, ToolCallDebug, ToolDefinition } from "../types";

export class ReplayFailure extends Error {
  readonly action_index: number;
  readonly drift_events: SelectorDriftEvent[];
  constructor(message: string, action_index: number, drift_events: SelectorDriftEvent[]) {
    super(message);
    this.action_index = action_index;
    this.drift_events = drift_events;
  }
}

export interface ReplayOutcome {
  drift_events: SelectorDriftEvent[];
  tool_calls: ToolCallDebug[];
}

export async function replayCachedActions(args: {
  cache: ActionCache;
  cacheKey: string;
  actions: RecordedAction[];
  page: Page;
  tools: ToolDefinition[];
}): Promise<ReplayOutcome> {
  const { cache, cacheKey, actions, page, tools } = args;
  const drift_events: SelectorDriftEvent[] = [];
  const tool_calls: ToolCallDebug[] = [];

  for (let i = 0; i < actions.length; i++) {
    const action = actions[i];
    const start = Date.now();
    const tool = findTool(tools, action.tool);
    if (!tool) throw new ReplayFailure(`unknown tool: ${action.tool}`, i, drift_events);

    if (!isSelectorTool(action.tool) || !action.selector_strategies) {
      // No strategies recorded — fall back to v2 behavior: dispatch
      // args verbatim with secret expansion. If this throws we treat
      // it as a total failure for the action.
      try {
        await tool.invoke(expandArgs(action.args), page);
      } catch (err) {
        throw new ReplayFailure(err instanceof Error ? err.message : String(err), i, drift_events);
      }
      tool_calls.push({ name: action.tool, ms: Date.now() - start });
      continue;
    }

    let outcome: Awaited<ReturnType<typeof dispatchWithFallbacks<unknown>>>;
    try {
      outcome = await dispatchWithFallbacks(
        expandArgs(action.args),
        expandStrategies(action.selector_strategies),
        async (sw) => tool.invoke(sw, page)
      );
    } catch (err) {
      throw new ReplayFailure(err instanceof Error ? err.message : String(err), i, drift_events);
    }

    tool_calls.push({ name: action.tool, ms: Date.now() - start });

    if (outcome.strategyIndex !== 0) {
      // Tier 1 silent re-rank: bump the winning strategy to rank 0 and
      // shift the others down. Persist immediately so the next replay
      // hits the new primary on its first try.
      const winning = action.selector_strategies[outcome.strategyIndex];
      const reordered: SelectorStrategy[] = [
        { ...winning, rank: 0 },
        ...action.selector_strategies
          .filter((_, idx) => idx !== outcome.strategyIndex)
          .map((s, idx) => ({ ...s, rank: idx + 1 }))
      ];
      cache.updateActionStrategies(cacheKey, i, reordered);
      drift_events.push({
        action_index: i,
        primary_selector: action.selector_strategies[0]?.selector ?? "",
        used_selector: redactSecrets(outcome.usedSelector),
        used_kind: outcome.usedKind
      });
    }
  }

  cache.recordReplay(cacheKey);
  return { drift_events, tool_calls };
}
