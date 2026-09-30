// Multi-strategy selector materialization + try-with-fallbacks dispatch.
//
// At record time, after a state-changing tool succeeds, we resolve the
// element the LLM clicked/typed and read three durability-graded
// alternatives off the live DOM:
//   testid    — `data-testid` attribute (most stable)
//   role+name — ARIA role + accessible name
//   text      — visible text (≤ 40 chars)
//   css       — the literal selector the LLM emitted (fallback safety net)
//
// At replay time, we walk strategies by rank. The first that resolves +
// dispatches without throwing wins. The runtime's cache layer is
// responsible for re-ranking on a fallback hit (Tier 1 silent heal).
//
// This module is deliberately framework-agnostic on the dispatch side —
// the caller passes a `dispatch` callback so the same machinery works
// for both record-time materialization (Playwright tools) and replay-
// time dispatch (cache replay tools).

import type { Page } from "@playwright/test";
import type { SelectorKind, SelectorStrategy } from "./action-cache";

/**
 * Selector-bearing tools — the ones whose `args.selector` (or
 * `args.element`) names a single page element. We only materialize
 * strategies for these. Other state-changing tools (browser_press_key,
 * browser_navigate) don't have a selector to drift.
 */
const SELECTOR_TOOLS = new Set([
  "browser_click",
  "browser_type",
  "browser_fill",
  "browser_hover",
  "browser_select_option",
  "browser_press_key", // sometimes carries a selector when targeted
  "browser_file_upload"
]);

export function isSelectorTool(name: string): boolean {
  return SELECTOR_TOOLS.has(name);
}

/**
 * `browser_snapshot` returns Playwright's `ariaSnapshot({mode: "ai"})`
 * output, which labels every element with a ref like `e14` — or `f1e14`
 * when the element lives inside a frame with a nonzero sequence number
 * (playwright-core `ariaSnapshotWithRefs`: `refPrefix = frameSeq ? "f" +
 * frameSeq : ""`; a plain top-level page after at least one navigation
 * already carries a nonzero frameSeq, so both shapes show up in practice).
 * This is the same convention other snapshot+ref browser-automation
 * tooling uses for ref-based clicking, and Playwright's own snapshot-linked
 * tools resolve those refs via `page.locator("aria-ref=" + ref)`. The model
 * reaches for these refs naturally (that's what a "ref" in a snapshot is
 * for) in any of the shapes the snapshot text itself uses — bare, bracketed,
 * with or without the `ref=` prefix — but our tools only documented plain
 * Playwright selectors: `ref=f1e14` throws "Unknown engine" and
 * `[ref=f1e14]` is parsed as a literal, nonexistent DOM attribute and just
 * times out — both silently derail the tool call instead of failing
 * clearly, and in practice burn the step's whole iteration budget before
 * `wait_for` times out downstream. Rewrite to the real engine instead.
 */
const ARIA_REF_RE = /^\[?(?:ref=)?((?:f\d+)?e\d+)\]?$/;

export function resolveAriaRefSelector(selector: string): string {
  const match = selector.match(ARIA_REF_RE);
  return match ? `aria-ref=${match[1]}` : selector;
}

/**
 * Normalize `args.selector`/`args.element` for a tool call, rewriting a
 * snapshot ref to Playwright's `aria-ref=` engine. Applied unconditionally
 * to any string `selector`/`element` field — not gated on `isSelectorTool`,
 * which exists for the strategy/cache machinery below and would otherwise
 * silently exclude a selector-bearing tool (e.g. `browser_wait_for_selector`,
 * which is read-only and never recorded) from the rewrite. Returns the same
 * object when there's nothing to rewrite — only ever narrows toward a
 * working selector.
 */
export function normalizeSelectorArgs(args: Record<string, unknown>): Record<string, unknown> {
  if (typeof args.selector !== "string" && typeof args.element !== "string") return args;
  const out = { ...args };
  if (typeof out.selector === "string") out.selector = resolveAriaRefSelector(out.selector);
  if (typeof out.element === "string") out.element = resolveAriaRefSelector(out.element);
  return out;
}

/**
 * True when `primary` is an `aria-ref=` selector and no durable alternative
 * (testid / role+name / text) was materialized for it. An aria-ref only
 * identifies an element within the page context that took the snapshot —
 * cache replay never calls `browser_snapshot`, so a ref-only recording can
 * never resolve there and would hard-fail every replay (a guaranteed
 * `ReplayFailure` → cache invalidation → Tier-2 healing redrive, not a rare
 * drift event). Callers use this to skip persisting such a recording rather
 * than caching something that can only ever miss.
 */
export function isNonDurableRecording(
  primary: string | undefined,
  strategies: SelectorStrategy[] | undefined
): boolean {
  if (!primary?.startsWith("aria-ref=")) return false;
  return !(strategies ?? []).some((s) => s.kind !== "css");
}

/**
 * Read durability-graded alternative selectors for the element the
 * given args targets. Returns an empty list if the args don't reference
 * a selector or the element can't be resolved on the page (which is
 * normal for non-selector tools like browser_press_key without a
 * `selector` field).
 *
 * The strategy whose `selector` text matches the LLM's primary is bumped
 * to rank 0. If multiple kinds resolve equivalently to the primary
 * (e.g. the LLM already used `[data-testid=foo]`), it's still the rank-0
 * entry — duplicates are de-duped by selector text.
 */
export async function materializeStrategies(
  page: Page,
  toolName: string,
  args: Record<string, unknown>
): Promise<SelectorStrategy[]> {
  if (!isSelectorTool(toolName)) return [];
  const primary = (args.selector ?? args.element) as string | undefined;
  if (!primary || typeof primary !== "string") return [];

  const candidates: Array<{ kind: SelectorKind; selector: string }> = [];
  // Always include the primary so we never lose what the LLM emitted.
  candidates.push({ kind: classifySelector(primary), selector: primary });

  try {
    const locator = page.locator(primary).first();
    // Resolve to a fresh ElementHandle (not memoized — the page may have
    // re-rendered between record and this call). 200ms is long enough
    // for any post-click DOM settle but short enough not to balloon
    // record-time wall clock if the element is gone.
    const handle = await locator.elementHandle({ timeout: 200 }).catch(() => null);
    if (!handle) return uniqueByRank(candidates);

    const testid = await handle.evaluate((el) => el.getAttribute("data-testid")).catch(() => null);
    if (testid) {
      candidates.unshift({ kind: "testid", selector: `[data-testid="${testid}"]` });
    }

    const role = await handle
      .evaluate((el) => {
        const explicit = el.getAttribute("role");
        if (explicit) return explicit;
        const tag = el.tagName.toLowerCase();
        const map: Record<string, string> = {
          a: "link",
          button: "button",
          input: "textbox",
          textarea: "textbox",
          select: "combobox",
          nav: "navigation",
          main: "main",
          header: "banner",
          footer: "contentinfo",
          aside: "complementary"
        };
        return map[tag] ?? null;
      })
      .catch(() => null);
    const name = await handle.evaluate(accessibleNameOf).catch(() => null);
    if (role && name) {
      const escaped = name.slice(0, 40).replace(/'/g, "\\'");
      candidates.push({ kind: "role_name", selector: `role=${role}[name='${escaped}']` });
    }

    const text = await handle.evaluate((el) => el.innerText?.trim() ?? null).catch(() => null);
    if (text) {
      const trimmed = text.slice(0, 40);
      candidates.push({ kind: "text", selector: `text=${trimmed}` });
    }

    await handle.dispose();
    return uniqueByRank(await keepUnambiguous(page, candidates));
  } catch {
    // Best effort. Materialization failure should never break a successful
    // recording — the primary selector still gets cached as a single
    // strategy, just without fallbacks.
  }

  return uniqueByRank(candidates);
}

/**
 * The name a `role=` selector matches, read in the page: `aria-label`, then
 * `aria-labelledby`, then an input's `<label>`, then its placeholder, then
 * its text. The label steps are what let typing into a form field record a
 * durable selector — an `<input>` has no `aria-label` and no `innerText`, so
 * it used to record only a snapshot ref, which no replay can resolve.
 *
 * Read from the DOM, NOT with `locator.ariaSnapshot()`: any aria snapshot
 * replaces the page's `aria-ref=` map, so taking one here would invalidate
 * every ref the model is about to act on. `keepUnambiguous` then checks the
 * guess with Playwright's own role engine, so a name it computes differently
 * is dropped rather than recorded.
 */
function accessibleNameOf(el: {
  getAttribute(name: string): string | null;
  innerText?: string;
  ownerDocument: { getElementById(id: string): { textContent: string | null } | null } | null;
}): string | null {
  const aria = el.getAttribute("aria-label");
  if (aria?.trim()) return aria.trim();
  const labelledBy = el.getAttribute("aria-labelledby");
  if (labelledBy && el.ownerDocument) {
    const doc = el.ownerDocument;
    const text = labelledBy
      .split(/\s+/)
      .map((id) => doc.getElementById(id)?.textContent?.trim() ?? "")
      .filter(Boolean)
      .join(" ");
    if (text) return text;
  }
  const labels = (el as { labels?: ArrayLike<{ innerText?: string }> | null }).labels;
  const label = labels && labels.length > 0 ? labels[0].innerText?.trim() : undefined;
  if (label) return label;
  const placeholder = el.getAttribute("placeholder");
  if (placeholder?.trim()) return placeholder.trim();
  return el.innerText?.trim() || null;
}

/**
 * Drop derived strategies that do not identify the element on their own.
 *
 * The derived alternates are guesses read off the recorded element, and
 * `uniqueByRank` promotes them above the primary by kind — text beats css,
 * because a label usually outlives a class name. That reasoning holds for a
 * label and fails for content: clicking Monaco records the editor's `innerText`,
 * which for an empty file is the line-number gutter, so the top-ranked strategy
 * became `text=1`. Replaying it clicked the gutter, the keystrokes went nowhere,
 * nothing threw, and the step reported a cache hit while doing nothing — the
 * failure surfaced three steps later as a save button that never appeared.
 *
 * Ambiguity is the signal that separates the two cases: `text=1` matched two
 * elements. A strategy that cannot say WHICH element it means has no business
 * outranking the one the model used and the run proved. The primary is always
 * kept — it is the selector that actually worked.
 */
async function keepUnambiguous(
  page: Page,
  candidates: Array<{ kind: SelectorKind; selector: string }>
): Promise<Array<{ kind: SelectorKind; selector: string }>> {
  const [primary, ...derived] = candidates;
  const kept = [primary];
  for (const c of derived) {
    const count = await page
      .locator(c.selector)
      .count()
      .catch(() => 0);
    if (count === 1) kept.push(c);
  }
  return kept;
}

function uniqueByRank(
  candidates: Array<{ kind: SelectorKind; selector: string }>
): SelectorStrategy[] {
  // Preference order independent of the LLM's primary: testid > role_name
  // > text > css. The primary stays at rank 0 if it doesn't already have
  // a more-stable version (e.g. the LLM used `text=Save` and the element
  // also has a testid → testid is rank 0, text= is rank 1, primary is
  // not duplicated).
  const order: Record<SelectorKind, number> = { testid: 0, role_name: 1, text: 2, css: 3 };
  const seen = new Set<string>();
  const unique = candidates.filter((c) => {
    if (seen.has(c.selector)) return false;
    seen.add(c.selector);
    return true;
  });
  unique.sort((a, b) => order[a.kind] - order[b.kind]);
  return unique.map((c, i) => ({ kind: c.kind, selector: c.selector, rank: i }));
}

export function classifySelector(selector: string): SelectorKind {
  if (/\[data-testid[=~]/.test(selector)) return "testid";
  if (/^role=/.test(selector)) return "role_name";
  if (/^text=/.test(selector)) return "text";
  return "css";
}

/**
 * Try `args.selector`, then walk fallback strategies by rank. Returns
 * the strategy index that succeeded (0 = primary). Throws when every
 * strategy fails — caller decides whether to invalidate the cache and
 * trigger a Tier 2 healing redrive.
 */
export async function dispatchWithFallbacks<R>(
  args: Record<string, unknown>,
  strategies: SelectorStrategy[] | undefined,
  dispatch: (args: Record<string, unknown>) => Promise<R>
): Promise<{ result: R; strategyIndex: number; usedSelector: string; usedKind: SelectorKind }> {
  // No strategies → behave like the v2 single-selector replay.
  if (!strategies || strategies.length === 0) {
    const result = await dispatch(args);
    const sel = (args.selector ?? args.element) as string | undefined;
    return {
      result,
      strategyIndex: 0,
      usedSelector: sel ?? "",
      usedKind: sel ? classifySelector(sel) : "css"
    };
  }

  const sorted = [...strategies].sort((a, b) => a.rank - b.rank);
  let lastErr: unknown;
  for (let i = 0; i < sorted.length; i++) {
    const strategy = sorted[i];
    try {
      const swapped: Record<string, unknown> = { ...args };
      if ("selector" in swapped) swapped.selector = strategy.selector;
      if ("element" in swapped) swapped.element = strategy.selector;
      const result = await dispatch(swapped);
      return {
        result,
        strategyIndex: i,
        usedSelector: strategy.selector,
        usedKind: strategy.kind
      };
    } catch (err) {
      lastErr = err;
    }
  }
  throw lastErr instanceof Error ? lastErr : new Error("all selector strategies failed for action");
}
