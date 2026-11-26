import { describe, expect, it } from "vitest";
import { parseJudgeResponse } from "./judge";

const NONCE = "a599599febf67966f";

describe("parseJudgeResponse", () => {
  it("reads a verdict wrapped in a json code fence", () => {
    const text = `\`\`\`json\n{"nonce": "${NONCE}", "passed": true, "rationale": "3.36 → 4.03 +20.0%"}\n\`\`\``;
    expect(parseJudgeResponse(text, NONCE)).toEqual({
      passed: true,
      rationale: "3.36 → 4.03 +20.0%"
    });
  });

  it("reads a verdict whose rationale quotes a brace", () => {
    const text = `{"nonce": "${NONCE}", "passed": true, "rationale": "the label reads {total}"}`;
    expect(parseJudgeResponse(text, NONCE).passed).toBe(true);
  });

  it("fails closed on a verdict carrying another nonce", () => {
    const text = `{"nonce": "not-it", "passed": true, "rationale": "x"}`;
    expect(parseJudgeResponse(text, NONCE).passed).toBe(false);
  });

  it("names truncation when the answer ran out of tokens", () => {
    const text = `\`\`\`json\n{"nonce": "${NONCE}", "passed": true, "rationale": "The UPSTREAM box shows`;
    const verdict = parseJudgeResponse(text, NONCE, "max_tokens");
    expect(verdict.passed).toBe(false);
    expect(verdict.rationale).toMatch(/^judge response truncated at max_tokens/);
  });

  it("still reports an unparsable answer as unparsable", () => {
    const verdict = parseJudgeResponse("I think it passes.", NONCE, "end_turn");
    expect(verdict.passed).toBe(false);
    expect(verdict.rationale).toMatch(/^unparsable judge response/);
  });
});
