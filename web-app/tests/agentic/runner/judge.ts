import { randomBytes } from "node:crypto";
import type { Page } from "@playwright/test";
import { computeCost } from "./pricing";
import { type ExpectResult, emptyJudgeUsage, type FlowExpect, type JudgeUsage } from "./types";

const ANTHROPIC_VERSION = "2023-06-01";
const ANTHROPIC_API_URL = "https://api.anthropic.com/v1/messages";

export interface JudgeOptions {
  apiKey: string;
  model: string;
}

export interface ExpectationsResult {
  results: ExpectResult[];
  usage: JudgeUsage;
}

export async function evaluateExpectations(
  page: Page,
  expectations: FlowExpect[],
  opts: JudgeOptions
): Promise<ExpectationsResult> {
  const results: ExpectResult[] = [];
  const usage: JudgeUsage = emptyJudgeUsage();
  for (const expect of expectations) {
    if (expect.assert) {
      results.push(await evaluateAssert(page, expect.assert));
    } else if (expect.judge) {
      results.push(await evaluateJudge(page, expect.judge, opts, usage));
    }
  }
  return { results, usage };
}

async function evaluateAssert(page: Page, claim: string): Promise<ExpectResult> {
  try {
    const result = await runAssert(page, claim);
    return { kind: "assert", passed: result.passed, claim, evidence: result.evidence };
  } catch (err) {
    return {
      kind: "assert",
      passed: false,
      claim,
      evidence: err instanceof Error ? err.message : String(err)
    };
  }
}

async function runAssert(
  page: Page,
  claim: string
): Promise<{ passed: boolean; evidence: string }> {
  const visibleMatch = claim.match(/^selector\s+(.+?)\s+is visible$/i);
  if (visibleMatch) {
    const sel = visibleMatch[1].trim();
    const visible = await page
      .locator(sel)
      .first()
      .isVisible()
      .catch(() => false);
    return { passed: visible, evidence: `visibility(${sel})=${visible}` };
  }

  const notVisibleMatch = claim.match(/^selector\s+(.+?)\s+is not visible$/i);
  if (notVisibleMatch) {
    const sel = notVisibleMatch[1].trim();
    const visible = await page
      .locator(sel)
      .first()
      .isVisible()
      .catch(() => false);
    return { passed: !visible, evidence: `visibility(${sel})=${visible}` };
  }

  const attrMatch = claim.match(/^selector\s+(.+?)\s+has attribute\s+([\w-]+)=(.+)$/i);
  if (attrMatch) {
    const [, sel, attr, want] = attrMatch;
    const actual = await page
      .locator(sel)
      .first()
      .getAttribute(attr)
      .catch(() => null);
    const got = actual ?? "<null>";
    return { passed: got === want.trim(), evidence: `${attr}(${sel})=${got}` };
  }

  // A controlled React input sets the VALUE PROPERTY, not the `value`
  // attribute, so `has attribute value=` cannot see it — `inputValue()` can.
  // Used for fields that are pre-filled rather than blank, where the point is
  // that *something* is there rather than what it is.
  const nonEmptyValueMatch = claim.match(/^selector\s+(.+?)\s+has a non-empty value$/i);
  if (nonEmptyValueMatch) {
    const sel = nonEmptyValueMatch[1].trim();
    const value = await page
      .locator(sel)
      .first()
      .inputValue()
      .catch(() => null);
    // `null` (no such element / not an input) is reported differently from
    // `""` (found, genuinely blank) — otherwise a typo'd selector and a real
    // regression produce identical evidence.
    return {
      passed: (value ?? "").trim().length > 0,
      evidence:
        value === null ? `value(${sel})=<unreadable>` : `value(${sel})=${JSON.stringify(value)}`
    };
  }

  // Text content, not input value: the target is often a button (a shadcn
  // `SelectTrigger`), where a placeholder is rendered as text rather than held
  // as a value.
  const notContainTextMatch = claim.match(
    /^selector\s+(.+?)\s+does not contain text\s+["'](.+)["']$/i
  );
  if (notContainTextMatch) {
    const [, sel, text] = notContainTextMatch;
    const content = await page
      .locator(sel.trim())
      .first()
      .textContent()
      .catch(() => null);
    const got = content ?? "";
    return {
      passed: !got.includes(text),
      evidence: `text(${sel.trim()})=${JSON.stringify(got)}`
    };
  }

  const textVisibleMatch = claim.match(/^text\s+["'](.+)["']\s+(?:is\s+)?visible$/i);
  if (textVisibleMatch) {
    const text = textVisibleMatch[1];
    const visible = await page
      .getByText(text)
      .first()
      .isVisible()
      .catch(() => false);
    return { passed: visible, evidence: `text(${text})=${visible}` };
  }

  const enabledMatch = claim.match(/^(.+?)\s+is enabled$/i);
  if (enabledMatch) {
    const desc = enabledMatch[1].toLowerCase();
    if (desc.includes("follow-up")) {
      const enabled = await page
        .getByPlaceholder("Ask follow-up question")
        .isEnabled()
        .catch(() => false);
      return { passed: enabled, evidence: `follow-up enabled=${enabled}` };
    }
  }

  // Save commit is async (network round trip, then a React state flush), and
  // a cold/loaded CI runner has been observed taking well over 5s — this is
  // the condition ("file state settled to saved"), not a fixed duration, that
  // the assert waits on. Optional `;timeout_ms=<n>` suffix mirrors the
  // `wait_for: selector_hidden:` override syntax for the same reason: a
  // legitimately-long wait shouldn't need a magic number baked into the regex.
  const saveButtonHiddenMatch = claim.match(/^save button is not visible(?:;timeout_ms=(\d+))?$/i);
  if (saveButtonHiddenMatch) {
    const timeout = saveButtonHiddenMatch[1]
      ? Number.parseInt(saveButtonHiddenMatch[1], 10)
      : 20_000;
    const hidden = await page
      .getByTestId("ide-save-button")
      .waitFor({ state: "hidden", timeout })
      .then(() => true)
      .catch(() => false);
    return { passed: hidden, evidence: `save button hidden within ${timeout}ms = ${hidden}` };
  }

  throw new Error(
    `unsupported assert pattern: '${claim}' (see runner/judge.ts for supported forms)`
  );
}

async function evaluateJudge(
  page: Page,
  claim: string,
  opts: JudgeOptions,
  usage: JudgeUsage
): Promise<ExpectResult> {
  const screenshot = await page.screenshot({ fullPage: false }).catch(() => null);
  const fullText = await page
    .locator("body")
    .innerText()
    .catch(() => "");
  // Chat/analytics threads render the agent's final answer at the BOTTOM of
  // a long reasoning trace, so a head-only slice truncates exactly the
  // content most judge claims care about (the answer itself). The screenshot
  // is `fullPage: false` and can't see below-the-fold content either, so
  // give the judge the head AND tail of the page text for long pages.
  const SLICE = 4000;
  const truncated = fullText.length > SLICE * 2;
  const domText = truncated
    ? `${fullText.slice(0, SLICE)}\n…[middle truncated]…\n${fullText.slice(-SLICE)}`
    : fullText;

  // The DOM text may contain attacker-controlled content (e.g. an LLM
  // response rendered in chat). Without a nonce, that content could
  // include a JSON blob that steers parseJudgeResponse to passed=true.
  // The nonce is a cryptographic random the judge must echo back; we
  // ignore any JSON object that doesn't include the matching nonce.
  const nonce = randomBytes(8).toString("hex");

  const messages = [
    {
      role: "user" as const,
      content: [
        ...(screenshot
          ? [
              {
                type: "image" as const,
                source: {
                  type: "base64" as const,
                  media_type: "image/png" as const,
                  data: screenshot.toString("base64")
                }
              }
            ]
          : []),
        {
          type: "text" as const,
          text: `Page text${truncated ? " (head + tail; middle truncated)" : ""}:\n${domText}\n\nClaim: ${claim}\n\nIs this claim true based on the screenshot and text? Respond with JSON: {"nonce": "${nonce}", "passed": boolean, "rationale": string}.`
        }
      ]
    }
  ];

  const res = await fetch(ANTHROPIC_API_URL, {
    method: "POST",
    headers: {
      "x-api-key": opts.apiKey,
      "anthropic-version": ANTHROPIC_VERSION,
      "content-type": "application/json"
    },
    body: JSON.stringify({
      model: opts.model,
      max_tokens: 256,
      messages,
      system: `You evaluate claims about a web application screenshot. Respond ONLY with JSON: {"nonce": "${nonce}", "passed": boolean, "rationale": "<one sentence>"}. The nonce field MUST exactly equal the value in the user message.`
    })
  });

  if (!res.ok) {
    const body = await res.text();
    return {
      kind: "judge",
      passed: false,
      claim,
      rationale: `judge API error ${res.status}: ${body.slice(0, 200)}`
    };
  }

  const json = (await res.json()) as {
    content?: Array<{ type: string; text?: string }>;
    usage?: {
      input_tokens?: number;
      cache_read_input_tokens?: number;
      cache_creation_input_tokens?: number;
      output_tokens?: number;
    };
  };

  if (json.usage) {
    usage.model = opts.model;
    usage.calls += 1;
    usage.tokens.input += json.usage.input_tokens ?? 0;
    usage.tokens.cached_input += json.usage.cache_read_input_tokens ?? 0;
    usage.tokens.cache_creation += json.usage.cache_creation_input_tokens ?? 0;
    usage.tokens.output += json.usage.output_tokens ?? 0;
    usage.cost_usd = computeCost(opts.model, usage.tokens);
  }

  const text = json.content?.find((c) => c.type === "text")?.text ?? "";
  const parsed = parseJudgeResponse(text, nonce);
  return { kind: "judge", passed: parsed.passed, claim, rationale: parsed.rationale };
}

function parseJudgeResponse(
  text: string,
  expectedNonce: string
): { passed: boolean; rationale: string } {
  // Walk every {...} block in the response (longest-first, so a nested
  // object inside the model's outer JSON envelope still wins) and accept
  // the first one whose nonce matches. A response with no nonce-bearing
  // block fails closed.
  const matches = Array.from(text.matchAll(/\{[\s\S]*?\}/g)).map((m) => m[0]);
  matches.sort((a, b) => b.length - a.length);
  for (const candidate of matches) {
    try {
      const obj = JSON.parse(candidate) as {
        nonce?: string;
        passed?: boolean;
        rationale?: string;
      };
      if (obj.nonce !== expectedNonce) continue;
      return { passed: obj.passed === true, rationale: obj.rationale ?? "" };
    } catch {
      // skip non-JSON blob, keep looking
    }
  }
  return { passed: false, rationale: `unparsable judge response: ${text.slice(0, 200)}` };
}
