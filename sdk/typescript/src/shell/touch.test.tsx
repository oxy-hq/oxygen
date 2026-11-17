// @vitest-environment jsdom
//
// The shell's 44px tap areas on a touch screen (decision 16). shell.css draws them; jsdom cannot
// evaluate a pointer media query or a style query, so the rules are held as text here (and
// measured in Chrome for the PR), and the opt-out is mounted for real.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mount, type Page, servePage, settle } from "../custom-app/notifications.harness";
import { OxyShell } from "./OxyShell";
import { useOxyTouchTargets } from "./touch";

const css = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "shell.css"),
  "utf8"
).replace(/\/\*[\s\S]*?\*\//g, "");

/** The body of the first `@media (pointer: coarse)` block whose first rule is a theme style query. */
function coarseThemedBlock(): string {
  const re = /@media \(pointer: coarse\) \{\s*@container style\(--oxy-theme-contract: 1\) \{/g;
  const m = re.exec(css);
  if (!m) return "";
  let depth = 2;
  let i = m.index + m[0].length;
  const start = i;
  for (; i < css.length && depth > 1; i++) {
    if (css[i] === "{") depth++;
    if (css[i] === "}") depth--;
  }
  return css.slice(start, i - 1);
}

const ON = ':root:not([data-oxy-touch-targets="off"])';

/** A kiosk frame around the shell, as Store Ops' KioskDesk wraps its desk. */
function Kiosk({ children }: { children: ReactNode }) {
  useOxyTouchTargets(false);
  return <>{children}</>;
}
const CONTROLS = [".oxy-rail__item", ".oxy-clock--button", ".oxy-askbtn", ".oxy-updatepill"];

describe("shell.css: 44px tap areas on a coarse pointer, with the theme", () => {
  const block = coarseThemedBlock();
  const rule = (selector: string) => {
    const esc = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    return new RegExp(`${esc}[^{]*\\{([^}]*)\\}`).exec(block)?.[1] ?? "";
  };

  it("sits inside a pointer query and the theme's style query", () => {
    expect(block.length).toBeGreaterThan(0);
  });

  it.each(CONTROLS)("%s answers a tap within 44px, centred, unless the app opts out", (sel) => {
    const area = rule(`${ON} ${sel}::before`);
    expect(area).toMatch(/content: "";/);
    expect(area).toMatch(/position: absolute;/);
    // Each side grows by half of what the box lacks of 44px, and never shrinks.
    for (const side of ["top", "right", "bottom", "left"])
      expect(area).toMatch(
        new RegExp(
          `${side}: min\\(0px, calc\\(\\(100% - var\\(--control-h-lg, 44px\\)\\) / 2\\)\\);`
        )
      );
    expect(rule(`${ON} ${sel}`)).toMatch(/position: relative;/);
  });

  // The phone layout's top-bar rule gives the clock, the Ask button and the update pill a centred
  // 44px box of its own (`width`/`height` + `transform`); on a phone with the theme both rules
  // apply, and with that box left on the insets moved the area 22px up and left, off the clock.
  it.each(CONTROLS)(
    "%s: the insets alone place the area, the phone bar's centred box reset",
    (sel) => {
      const area = rule(`${ON} ${sel}::before`);
      expect(area).toMatch(/width: auto;/);
      expect(area).toMatch(/height: auto;/);
      expect(area).toMatch(/transform: none;/);
      expect(css).toMatch(
        /\.oxy-clock--button::before,[^{]*\{[^}]*width: max\(100%, var\(--control-h-lg, 44px\)\);[^}]*transform: translate\(-50%, -50%\);/
      );
    }
  );

  it("spaces the rail's 32px entries 44px apart, so their areas tile without overlapping", () => {
    // 2rem (32px) entry + gap = --control-h-lg (44px); each area grows 6px into each gap.
    expect(rule(`${ON} .oxy-rail__nav`)).toMatch(
      /gap: calc\(var\(--control-h-lg, 44px\) - 2rem\);/
    );
    for (const slot of [".oxy-rail__footer", ".oxy-rail__bottom"])
      expect(block).toContain(`${ON} ${slot}`);
    expect(css).toMatch(/\.oxy-rail__item \{[^}]*height: 2rem;[^}]*width: 2rem;/);
  });

  it("makes the mode menu's rows a real 44px", () => {
    expect(rule(`${ON} .oxy-modeswitch__item`)).toMatch(/min-height: var\(--control-h-lg, 44px\);/);
  });

  it("draws no rail or Ask tap area outside the theme's block", () => {
    const outside = css.replace(block, "");
    expect(outside).not.toMatch(/\.oxy-rail__item::before/);
    // Outside it, the opt-out is read once: by the home link's own touch height (below).
    expect(outside.match(/data-oxy-touch-targets/g) ?? []).toHaveLength(1);
  });
});

describe("shell.css: the breadcrumb's home link on a coarse pointer", () => {
  // 2.27's 44px home link, with or without the theme. A screen that turns the tap areas off keeps
  // the link at its drawn size, as Store Ops' tablet needs (it held the link in deskTouch.css).
  const rules = [
    ...css.matchAll(/@media \(pointer: coarse\) \{\s*([^{}]*)\{([^}]*)\}\s*\}/g)
  ].filter((m) => m[1].includes(".oxy-breadcrumb__workspace"));

  it("is one rule, and it honours the opt-out", () => {
    expect(rules).toHaveLength(1);
    expect(rules[0][1].trim()).toBe(`:where(${ON}) .oxy-breadcrumb__workspace`);
    expect(rules[0][2]).toMatch(/min-height: var\(--control-h-lg, 2\.75rem\);/);
  });

  it("weighs one class, as on 2.28, so an app's own rule for the link still wins", () => {
    // Everything but the class sits inside :where(), which weighs nothing.
    const selector = rules[0]?.[1].trim() ?? "";
    expect(selector.replace(/:where\((?:[^()]|\([^()]*\))*\)/g, "").trim()).toBe(
      ".oxy-breadcrumb__workspace"
    );
  });
});

describe("the opt-out", () => {
  const root = document.documentElement;
  let page: Page | undefined;

  beforeEach(() => {
    vi.useFakeTimers();
    servePage();
    root.removeAttribute("data-oxy-touch-targets");
  });

  afterEach(async () => {
    await page?.unmount();
    page = undefined;
    vi.useRealTimers();
  });

  const fetcher = (async () => new Response("{}", { status: 404 })) as unknown as typeof fetch;

  it("<OxyShell touchTargets={false}> turns the areas off while mounted", async () => {
    page = await mount(<OxyShell touchTargets={false}>app</OxyShell>, fetcher);
    await settle();
    expect(root.getAttribute("data-oxy-touch-targets")).toBe("off");
    await page.unmount();
    page = undefined;
    expect(root.hasAttribute("data-oxy-touch-targets")).toBe(false);
  });

  it("leaves them on by default", async () => {
    page = await mount(<OxyShell>app</OxyShell>, fetcher);
    await settle();
    expect(root.hasAttribute("data-oxy-touch-targets")).toBe(false);
  });

  it("useOxyTouchTargets(false) in a frame around the shell, and the earlier value comes back", async () => {
    root.setAttribute("data-oxy-touch-targets", "on");
    page = await mount(
      <Kiosk>
        <OxyShell>app</OxyShell>
      </Kiosk>,
      fetcher
    );
    await settle();
    expect(root.getAttribute("data-oxy-touch-targets")).toBe("off");
    await page.unmount();
    page = undefined;
    expect(root.getAttribute("data-oxy-touch-targets")).toBe("on");
  });
});
