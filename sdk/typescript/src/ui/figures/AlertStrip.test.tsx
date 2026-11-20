// @vitest-environment jsdom
//
// The attention strip: a named group whose alerts are doors (real links followed in place on a
// plain click and left to the browser otherwise, or buttons), read-only alerts that are no
// control at all, the all-clear said in the success tone, and decision 37's padding and the
// end fade in figures.css (not in an app's index.css).

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ui } from "../recipes";
import { $, $$, click, fire, render } from "../ui.harness";
import { AlertStrip } from "./AlertStrip";

afterEach(() => {
  document.body.innerHTML = "";
});

const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "figures.css"), "utf8");
const block = (selector: string) => {
  const at = css.indexOf(`${selector} {`);
  return at < 0 ? "" : css.slice(at, css.indexOf("}", at));
};

const clickWith = (init: MouseEventInit) =>
  new MouseEvent("click", { bubbles: true, cancelable: true, button: 0, ...init });

describe("AlertStrip", () => {
  it("is a group named by its eyebrow, each alert a callout in its tone", async () => {
    await render(
      <AlertStrip
        label='Reorder'
        alerts={[
          { tone: "error", label: "3 out", href: "#gaps" },
          { tone: "warning", label: "2 late" }
        ]}
      />
    );
    const group = $("[role='group']") as HTMLElement;
    expect(group.getAttribute("aria-label")).toBe("Reorder");
    expect(group.querySelector(".oxy-ui-alert-strip__label")?.className).toContain(ui.eyebrow);
    const [first, second] = $$(".oxy-ui-alert-strip__alert");
    expect(first.className).toContain(ui.calloutTone.error);
    expect(second.className).toContain(ui.calloutTone.warning);
    expect(first.className).toContain(ui.callout);
  });

  it("makes a link alert a real link: a plain click goes in place, any other is the browser's", async () => {
    const onNavigate = vi.fn();
    await render(
      <AlertStrip
        alerts={[
          {
            tone: "error",
            label: "3 out",
            ariaLabel: "3 out. See the gaps.",
            href: "#gaps",
            onNavigate
          }
        ]}
      />
    );
    const link = $("a") as HTMLAnchorElement;
    expect(link.getAttribute("href")).toBe("#gaps");
    expect(link.getAttribute("aria-label")).toBe("3 out. See the gaps.");
    expect(link.querySelector("svg")?.getAttribute("aria-hidden")).toBe("true");
    const plain = clickWith({});
    await fire(link, plain);
    expect(onNavigate).toHaveBeenCalledTimes(1);
    expect(plain.defaultPrevented).toBe(true);
    for (const init of [{ metaKey: true }, { ctrlKey: true }, { shiftKey: true }, { button: 1 }]) {
      const other = clickWith(init);
      await fire(link, other);
      expect(other.defaultPrevented, JSON.stringify(init)).toBe(false);
    }
    expect(onNavigate).toHaveBeenCalledTimes(1);
  });

  it("follows Supply Chain's onFollow the same way, and leaves a link with no handler alone", async () => {
    const onFollow = vi.fn();
    await render(
      <AlertStrip
        alerts={[
          { tone: "warning", label: "2 late", href: "#late", onFollow },
          { tone: "info", label: "1 new", href: "#new" }
        ]}
      />
    );
    const [late, fresh] = $$("a");
    const a = clickWith({});
    await fire(late, a);
    expect(onFollow).toHaveBeenCalledTimes(1);
    expect(a.defaultPrevented).toBe(true);
    const b = clickWith({});
    await fire(fresh, b);
    expect(b.defaultPrevented).toBe(false);
  });

  it("makes an onSelect alert a button, and an alert with neither no control at all", async () => {
    const onSelect = vi.fn();
    await render(
      <AlertStrip
        alerts={[
          { tone: "warning", label: "4 follow-ups open", onSelect },
          { tone: "info", label: "Counts close at 9" }
        ]}
      />
    );
    const button = $("button") as HTMLButtonElement;
    expect(button.type).toBe("button");
    expect(button.textContent).toBe("4 follow-ups open");
    await click(button);
    expect(onSelect).toHaveBeenCalledTimes(1);
    const info = $$(".oxy-ui-alert-strip__alert")[1];
    expect(info.tagName).toBe("SPAN");
    expect(info.querySelector("svg")).toBeNull();
    expect(info.classList.contains("oxy-ui-alert-strip__door")).toBe(false);
    expect($$("a, button")).toHaveLength(1);
  });

  it("says its all-clear in the success tone when there is nothing, without a live region", async () => {
    const page = await render(<AlertStrip alerts={[]} />);
    const clear = $("[data-all-clear]") as HTMLElement;
    expect(clear.textContent).toBe("Nothing needs attention");
    expect(clear.className).toContain(ui.calloutTone.success);
    expect($("[role='status'], [role='alert'], [aria-live]")).toBeNull();
    await page.rerender(<AlertStrip alerts={[]} allClear={<b>All counted</b>} />);
    expect($("[data-all-clear] b")?.textContent).toBe("All counted");
  });

  it("takes decision 37's inset at both ends, in its own CSS, and fades only where scroll drives it", () => {
    const strip = block(".oxy-ui-alert-strip");
    expect(strip).toMatch(/--oxy-ui-alert-strip-inset: calc\(var\(--padding-sm-x\) \+ 1px\)/);
    expect(strip).toMatch(/overflow-x: auto/);
    expect(strip).not.toMatch(/padding-inline|padding-left|padding-right/);
    expect(block(".oxy-ui-alert-strip__label")).toMatch(
      /padding-left: var\(--oxy-ui-alert-strip-inset\)/
    );
    expect(block(".oxy-ui-alert-strip::after")).toMatch(
      /margin-left: calc\(var\(--oxy-ui-alert-strip-inset\) - 2\.5rem - var\(--space-sm\)\)/
    );
    const supports = css.slice(css.indexOf("@supports (animation-timeline: scroll())"));
    expect(supports).toMatch(
      /animation: oxy-ui-alert-strip-end linear both;\s*animation-timeline: scroll\(nearest inline\)/
    );
    expect(css.slice(0, css.indexOf("@supports (animation-timeline: scroll())"))).not.toMatch(
      /animation:/
    );
  });
});
