import { afterEach, describe, expect, it } from "vitest";
import type { ActionCache } from "../action-cache";
import { redactStrategies } from "../secrets";
import type { ToolDefinition } from "../types";
import { replayCachedActions } from "./replay";

// A recorded selector strategy crosses the same `${VAR}` boundary as the
// action's args: the plaintext a page showed is recorded as the placeholder,
// and a replay dispatches the placeholder expanded to its own value.
const RUN = "$" + "{SHOWCASE_RUN}";

function clickTool(seen: string[]): ToolDefinition[] {
  return [
    {
      name: "browser_click",
      description: "",
      inputSchema: { type: "object", properties: {} },
      invoke: async (args: Record<string, unknown>) => {
        seen.push(String(args.selector));
        return {};
      }
    }
  ] as unknown as ToolDefinition[];
}

const noCache = { recordReplay() {}, updateActionStrategies() {} } as unknown as ActionCache;

describe("selector strategies and the placeholder boundary", () => {
  afterEach(() => {
    delete process.env.SHOWCASE_RUN;
  });

  it("records the placeholder, not the value the record pass typed", () => {
    process.env.SHOWCASE_RUN = "3f9a2c1b";
    expect(
      redactStrategies([
        { kind: "text" as const, selector: "text=Front counter 3f9a2c1b", rank: 0 }
      ])
    ).toEqual([{ kind: "text", selector: `text=Front counter ${RUN}`, rank: 0 }]);
  });

  it("replays each strategy with the placeholder expanded to this pass's value", async () => {
    process.env.SHOWCASE_RUN = "77aa88bb";
    const seen: string[] = [];
    await replayCachedActions({
      cache: noCache,
      cacheKey: "k",
      page: {} as never,
      tools: clickTool(seen),
      actions: [
        {
          tool: "browser_click",
          args: { selector: `text=Front counter ${RUN}` },
          selector_strategies: [{ kind: "text", selector: `text=Front counter ${RUN}`, rank: 0 }]
        }
      ]
    });
    expect(seen).toEqual(["text=Front counter 77aa88bb"]);
  });
});
