import type { Page } from "@playwright/test";
import { applyPathPrefix } from "./backend";
import { resolveRepoFile } from "./files";
import { captureSnapshot } from "./snapshot";
import type { ToolDefinition } from "./types";

const PAGE_TEXT_LIMIT = 8_000;
const DEFAULT_WAIT_MS = 10_000;
// Click/type timeout is short on purpose — Playwright's default 30s means a
// single wrong selector burns 30s before the model can adjust. 5s is plenty
// for elements that are actually visible in the snapshot.
const ACTION_TIMEOUT_MS = 5_000;

const TOOLS: ToolDefinition[] = [
  {
    name: "browser_snapshot",
    description:
      'Return a compact accessibility-tree text snapshot of the current page. Call this at the start of each step to identify elements by their visible label and ARIA role. Each element is tagged with a ref like [ref=f1e14] — pass that exact string as the selector to browser_click/browser_type/etc. when the element has no stable text/role/testid (e.g. ambiguous icon buttons, list rows with duplicate labels). Truncated to ~12kB; use browser_get_page_text as a fallback if the snapshot is too noisy. Optional region: "main" scopes to the main content region; any other string is treated as a CSS selector for a subtree.',
    inputSchema: {
      type: "object",
      properties: {
        region: {
          type: "string",
          description:
            'Optional region to scope the snapshot. "main" → main content region; any other string → CSS selector.'
        }
      }
    },
    invoke: async (args, page) => {
      const region = typeof args.region === "string" ? args.region : undefined;
      return { snapshot: await captureSnapshot(page, { region }) };
    }
  },
  {
    name: "browser_click",
    description:
      "Click an element. Pass a Playwright selector (CSS, role=, text=, or [data-testid=...]), or a ref from browser_snapshot's [ref=f1e14] tags. Prefer text= or role-based selectors over a ref when the snapshot shows a stable visible label — a ref only identifies this exact render, so it won't survive a re-snapshot the way a label-based selector does.",
    inputSchema: {
      type: "object",
      properties: {
        selector: {
          type: "string",
          description:
            'Playwright selector — e.g. "text=Submit", "role=button[name=\'Save\']", "[data-testid=foo]", or a snapshot ref "f1e14" / "[ref=f1e14]".'
        }
      },
      required: ["selector"]
    },
    invoke: async (args, page) => {
      const selector = String(args.selector);
      await page.locator(selector).first().click({ timeout: ACTION_TIMEOUT_MS });
      return { ok: true };
    }
  },
  {
    name: "browser_type",
    description:
      "Type text into an input or textarea identified by a selector. Replaces existing content unless append=true.",
    inputSchema: {
      type: "object",
      properties: {
        selector: {
          type: "string",
          description:
            'Playwright selector for an editable element (CSS, role=, text=, [data-testid=...], or a browser_snapshot ref like "f1e14"/"[ref=f1e14]").'
        },
        text: { type: "string", description: "Text to enter." },
        append: { type: "boolean", description: "If true, type at the end without clearing." }
      },
      required: ["selector", "text"]
    },
    invoke: async (args, page) => {
      const selector = String(args.selector);
      const text = String(args.text);
      const locator = page.locator(selector).first();
      if (args.append) {
        await locator.focus({ timeout: ACTION_TIMEOUT_MS });
        await page.keyboard.type(text, { delay: 25 });
        await page.waitForTimeout(100);
      } else {
        await locator.fill(text, { timeout: ACTION_TIMEOUT_MS });
      }
      return { ok: true };
    }
  },
  {
    name: "browser_press_key",
    description:
      "Press a single key or chord on the focused element. Examples: 'Enter', 'Tab', 'ControlOrMeta+s', 'Control+Enter'. For an app keybinding bound to Ctrl/Cmd (Monaco's save, etc.), use 'ControlOrMeta+<key>' rather than a literal 'Meta+<key>' — Playwright resolves it to the platform's actual modifier (Control on Linux/Windows CI, Meta on macOS), whereas a literal 'Meta' sends the physical Meta/Super key, which a Monaco `KeyMod.CtrlCmd` keybinding doesn't match on Linux.",
    inputSchema: {
      type: "object",
      properties: {
        key: { type: "string", description: "Playwright key combo." }
      },
      required: ["key"]
    },
    invoke: async (args, page) => {
      await page.keyboard.press(String(args.key));
      return { ok: true };
    }
  },
  {
    name: "browser_keyboard_type",
    description:
      "Type text into the currently-focused element via the keyboard, character by character. Use this for editors like Monaco where browser_type's selector-based focus picks the wrong textarea — first focus the editor with browser_click on the editor surface (e.g. .monaco-editor), then call this.",
    inputSchema: {
      type: "object",
      properties: {
        text: { type: "string", description: "Text to type into the focused element." }
      },
      required: ["text"]
    },
    invoke: async (args, page) => {
      // 25ms between keystrokes so Monaco's React input handler keeps up.
      // With delay=0 (Playwright's default) HEADED Chromium drops chars,
      // and even headless can lose them if the editor is busy formatting.
      await page.keyboard.type(String(args.text), { delay: 25 });
      // Tiny pause so the next step (e.g. ControlOrMeta+s) doesn't race the
      // final input event into Monaco's debounced state update.
      await page.waitForTimeout(100);
      return { ok: true };
    }
  },
  {
    name: "browser_file_upload",
    description:
      'Attach files to an `<input type="file">` element via Playwright\'s setInputFiles. Use this for upload UIs (e.g. the DuckDB onboarding wizard\'s CSV/Parquet upload step). Selector must point at the actual `<input type="file">` (often hidden behind a styled drop zone — the field\'s `id` is `credential-<key>`, so `#credential-<key>` works for the onboarding wizard). The `paths` array entries are repo-relative; absolute paths and `..` traversal are refused.',
    inputSchema: {
      type: "object",
      properties: {
        selector: {
          type: "string",
          description:
            'Playwright selector for an `<input type="file">`. The element does not need to be visible — Playwright sets the input directly.'
        },
        paths: {
          type: "array",
          items: { type: "string" },
          minItems: 1,
          description:
            "Repo-relative file paths to upload (always an array, even for a single file). Example: ['demo_project/.db/oxymart.csv']."
        }
      },
      required: ["selector", "paths"]
    },
    invoke: async (args, page) => {
      const selector = String(args.selector);
      const raw = args.paths;
      // Accept a string for forgiveness even though the schema asks for
      // an array — the LLM occasionally passes a bare string and there's
      // no good reason to reject it.
      const list: string[] = Array.isArray(raw) ? raw.map((p) => String(p)) : [String(raw)];
      if (list.length === 0) throw new Error("browser_file_upload: paths is empty");
      const resolved = list.map((p) => resolveRepoFile(p));
      await page.locator(selector).first().setInputFiles(resolved, { timeout: ACTION_TIMEOUT_MS });
      return { ok: true, uploaded: resolved.length };
    }
  },
  {
    name: "browser_navigate",
    description:
      "Navigate the page to a URL. Absolute URL or a path relative to the current origin.",
    inputSchema: {
      type: "object",
      properties: {
        url: {
          type: "string",
          description: "URL or path, e.g. '/' or 'http://localhost:5173/ide'."
        }
      },
      required: ["url"]
    },
    invoke: async (args, page) => {
      // Same workspace scoping as a flow's `goto:` — an act that says
      // "navigate to /threads" means the surface, and where that surface lives
      // is a property of the deployment, not of the instruction. Without this
      // the two navigation doors disagree: setup lands scoped and a mid-flow
      // browser_navigate silently drops back to the org home.
      await page.goto(applyPathPrefix(String(args.url)));
      return { ok: true };
    }
  },
  {
    name: "browser_screenshot",
    description:
      "Take a PNG screenshot of the current viewport, returned as base64. Expensive — prefer browser_snapshot when you only need to identify elements.",
    inputSchema: { type: "object", properties: {} },
    invoke: async (_args, page) => {
      const buf = await page.screenshot({ fullPage: false });
      return { image_base64: buf.toString("base64") };
    }
  },
  {
    name: "browser_wait_for_selector",
    description: "Wait up to 10 seconds for a selector to become visible.",
    inputSchema: {
      type: "object",
      properties: {
        selector: {
          type: "string",
          description: 'Playwright selector, or a browser_snapshot ref like "f1e14"/"[ref=f1e14]".'
        }
      },
      required: ["selector"]
    },
    invoke: async (args, page) => {
      await page
        .locator(String(args.selector))
        .first()
        .waitFor({ state: "visible", timeout: DEFAULT_WAIT_MS });
      return { ok: true };
    }
  },
  {
    name: "browser_get_page_text",
    description:
      "Return the visible text of the page (truncated to ~8kB). Use as a fallback when browser_snapshot is too noisy or omits the content you need.",
    inputSchema: { type: "object", properties: {} },
    invoke: async (_args, page) => {
      const text = await page
        .locator("body")
        .innerText()
        .catch(() => "");
      return { text: text.slice(0, PAGE_TEXT_LIMIT) };
    }
  }
];

export function getGenericTools(): ToolDefinition[] {
  return TOOLS;
}

export function findTool(tools: ToolDefinition[], name: string): ToolDefinition | undefined {
  return tools.find((t) => t.name === name);
}

const STATE_CHANGING_TOOLS = new Set([
  "browser_click",
  "browser_type",
  "browser_keyboard_type",
  "browser_press_key",
  "browser_navigate",
  "browser_file_upload"
]);

export function isStateChanging(toolName: string): boolean {
  return STATE_CHANGING_TOOLS.has(toolName);
}

export async function runWaitFor(page: Page, primitive: string): Promise<void> {
  if (primitive === "streaming_complete") {
    await waitForStreamingComplete(page);
    return;
  }
  if (primitive === "network_idle") {
    await page.waitForLoadState("networkidle");
    return;
  }
  if (primitive.startsWith("selector:")) {
    // Optional `;timeout_ms=<n>` suffix lets a flow override Playwright's
    // 30s default for legitimately-long waits (build phases, etc.). The
    // suffix is parsed off the end so CSS selectors that contain `:` and
    // `=` keep working unchanged. `;` is not a valid CSS selector
    // character, so it's an unambiguous separator.
    const body = primitive.slice("selector:".length);
    const match = body.match(/^(.+);timeout_ms=(\d+)$/);
    const sel = match ? match[1] : body;
    const timeout = match ? Number.parseInt(match[2], 10) : undefined;
    await page.locator(sel).first().waitFor({ state: "visible", timeout });
    return;
  }
  if (primitive.startsWith("selector_hidden:")) {
    // Counterpart to `selector:` that waits for an element to disappear.
    // Useful for "wait until the loading spinner clears before screenshot"
    // patterns. Supports the same `;timeout_ms=<n>` override syntax.
    const body = primitive.slice("selector_hidden:".length);
    const match = body.match(/^(.+);timeout_ms=(\d+)$/);
    const sel = match ? match[1] : body;
    const timeout = match ? Number.parseInt(match[2], 10) : undefined;
    await page.locator(sel).first().waitFor({ state: "hidden", timeout });
    return;
  }
  throw new Error(`unknown wait_for primitive: '${primitive}'`);
}

// `agent-loading-state` is a flicker (only rendered while isStreaming &&
// !showAnswer), so we accept any of three early-state signals before gating
// on the stop button disappearing. waitFor({state: "hidden"}) on an
// unattached element returns immediately, which is why we wait for SOMETHING
// to appear first. Promise.any (no .catch fallthrough) ensures we actually
// fail when the previous step never started a stream — otherwise an LLM
// that bails out early is masked by a no-op "wait".
async function waitForStreamingComplete(page: Page): Promise<void> {
  try {
    await Promise.any([
      page.getByTestId("agent-loading-state").waitFor({ state: "visible", timeout: 20_000 }),
      page
        .getByTestId("agent-message-container")
        .first()
        .waitFor({ state: "visible", timeout: 20_000 }),
      page.getByTestId("message-input-stop-button").waitFor({ state: "visible", timeout: 20_000 })
    ]);
  } catch {
    throw new Error(
      "wait_for streaming_complete: no streaming signal (loading state, message container, or stop button) appeared within 20s — the previous act step probably never submitted"
    );
  }

  await page
    .getByTestId("message-input-stop-button")
    .waitFor({ state: "hidden", timeout: 240_000 });

  // Either the home submit button or the thread follow-up send button is fine.
  await Promise.any([
    page.getByTestId("chat-panel-submit-button").waitFor({ state: "visible", timeout: 15_000 }),
    page.getByTestId("message-input-send-button").waitFor({ state: "visible", timeout: 15_000 })
  ]).catch(() => {});
}
