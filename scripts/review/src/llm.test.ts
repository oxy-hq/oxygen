import assert from "node:assert/strict";
import { test } from "node:test";
import type { SDKMessage } from "@anthropic-ai/claude-agent-sdk";
import { consume, usageOf } from "./llm.ts";

/** Just the fields consume() reads; the SDK's message types carry many more. */
const msg = (m: Record<string, unknown>): SDKMessage => m as unknown as SDKMessage;

async function* stream(messages: SDKMessage[]): AsyncIterable<SDKMessage> {
  for (const m of messages) yield m;
}

const assistant = (
  usage: Record<string, number>,
  content: unknown[],
  parent: string | null = null
) => msg({ type: "assistant", parent_tool_use_id: parent, message: { usage, content } });

test("consume keeps the first call's input usage, counts tools and signals the first response once", async () => {
  let signals = 0;
  const out = await consume(
    stream([
      msg({ type: "system", subtype: "init" }),
      msg({ type: "stream_event", event: { type: "message_start" } }),
      assistant(
        {
          input_tokens: 12,
          cache_creation_input_tokens: 31_000,
          cache_read_input_tokens: 0,
          output_tokens: 5
        },
        [{ type: "tool_use", name: "Read" }]
      ),
      assistant(
        {
          input_tokens: 3,
          cache_creation_input_tokens: 400,
          cache_read_input_tokens: 31_000,
          output_tokens: 9
        },
        [
          { type: "tool_use", name: "Read" },
          { type: "tool_use", name: "Grep" }
        ]
      ),
      msg({ type: "result", subtype: "success", modelUsage: {} })
    ]),
    () => signals++
  );
  assert.equal(signals, 1);
  assert.deepEqual(out.first, { input: 12, cacheWrite: 31_000, cacheRead: 0 });
  assert.deepEqual(out.tools, { Read: 2, Grep: 1 });
  assert.equal(out.result?.type, "result");
});

test("usage sums every model the pass called; no result means no usage", () => {
  const model = (input: number, write: number, read: number, output: number) => ({
    inputTokens: input,
    cacheCreationInputTokens: write,
    cacheReadInputTokens: read,
    outputTokens: output,
    webSearchRequests: 0,
    costUSD: 0,
    contextWindow: 0,
    maxOutputTokens: 0
  });
  assert.deepEqual(
    usageOf({
      modelUsage: {
        "claude-opus-5-5": model(10, 20, 30, 40),
        "claude-haiku-5-5": model(1, 2, 3, 4)
      }
    }),
    { input: 11, cacheWrite: 22, cacheRead: 33, output: 44 }
  );
  assert.equal(usageOf(null), undefined);
  assert.equal(usageOf({ modelUsage: {} }), undefined);
});
