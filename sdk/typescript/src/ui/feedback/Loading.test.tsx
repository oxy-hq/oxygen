// @vitest-environment jsdom
//
// The wait: every loading piece is a polite status, never a Tab stop of its own, and a place focus
// can be held after Try again (`tabIndex=-1`, no ring); the bar says its count and never draws
// empty; the skeleton is hidden from assistive tech and keeps its widths across renders; a kit
// row saying a re-read is running holds focus as the line does.

import { useRef } from "react";
import { afterAll, afterEach, describe, expect, it } from "vitest";
import { markRetry, useRetryButtonFocus } from "../focus/useRetryFocus";
import { paintUi, unpaintUi } from "../paint.harness";
import { ui } from "../recipes";
import { $, $$, click, render } from "../ui.harness";
import { KitLoadingText, Loading, LoadingBar, LoadingLine, SkeletonRows } from "./Loading";

afterEach(() => {
  document.body.innerHTML = "";
});

const fill = () => $(".oxy-ui-loading-bar__fill") as HTMLElement;

describe("LoadingBar", () => {
  it("is a polite status named with its count, held focus but never a Tab stop", async () => {
    await render(<LoadingBar label='Comparing stores…' progress={{ ready: 2, total: 8 }} />);
    const bar = $('[role="status"]') as HTMLElement;
    expect(bar.getAttribute("aria-live")).toBe("polite");
    expect(bar.getAttribute("aria-label")).toBe("Comparing stores… — 2 of 8 parts ready");
    expect(bar.getAttribute("tabindex")).toBe("-1");
    expect(bar.hasAttribute("data-retry-focus")).toBe(true);
    expect(fill().style.width).toBe("25%");
  });

  it("never draws empty: a sliver at zero, and at nothing issued", async () => {
    const page = await render(<LoadingBar label='Reading' progress={{ ready: 0, total: 8 }} />);
    expect(fill().style.width).toBe("4%");
    await page.rerender(<LoadingBar label='Reading' progress={{ ready: 0, total: 0 }} />);
    expect(fill().style.width).toBe("4%");
  });
});

describe("SkeletonRows", () => {
  it("draws the table's shape, hidden, with the same widths every render", async () => {
    const page = await render(<SkeletonRows rows={3} cols={4} />);
    expect($(".oxy-ui-skeleton")?.getAttribute("aria-hidden")).toBe("true");
    expect($$(".oxy-ui-skeleton__row")).toHaveLength(3);
    expect($$(".oxy-ui-skeleton__bar")).toHaveLength(3 * 5);
    const widths = () => $$(".oxy-ui-skeleton__bar").map((b) => b.style.width);
    const before = widths();
    await page.rerender(<SkeletonRows rows={3} cols={4} />);
    expect(widths()).toEqual(before);
  });
});

describe("Loading and LoadingLine", () => {
  it("Loading says what it reads over the skeleton, on a card", async () => {
    await render(<Loading label="Loading this week's ads…" rows={2} cols={2} />);
    const root = $(".oxy-ui-loading") as HTMLElement;
    expect(root.classList.contains(ui.card)).toBe(true);
    const line = root.querySelector('[role="status"]') as HTMLElement;
    expect(line.textContent).toBe("Loading this week's ads…");
    expect(line.getAttribute("tabindex")).toBe("-1");
    expect(root.querySelector(".oxy-ui-skeleton")).not.toBeNull();
  });

  it("LoadingLine is a polite status on the note recipe", async () => {
    await render(<LoadingLine>Loading orders…</LoadingLine>);
    const line = $('[role="status"]') as HTMLElement;
    expect(line.classList.contains(ui.note)).toBe(true);
    expect(line.getAttribute("aria-live")).toBe("polite");
    expect(line.getAttribute("tabindex")).toBe("-1");
  });
});

function Retry({ onRetry }: { onRetry: () => void }) {
  const ref = useRef<HTMLButtonElement>(null);
  useRetryButtonFocus(ref);
  return (
    <button
      ref={ref}
      type='button'
      onClick={(e) => {
        markRetry(e.currentTarget);
        onRetry();
      }}
    >
      Try again
    </button>
  );
}

describe("KitLoadingText", () => {
  it("holds focus on the kit row around it after Try again", async () => {
    const page = await render(<Retry onRetry={() => {}} />);
    const b = $("button") as HTMLElement;
    b.focus();
    await click(b);
    await page.rerender(
      <div className='oxy-phone-row'>
        <KitLoadingText>Loading orders…</KitLoadingText>
      </div>
    );
    expect(document.activeElement).toBe($(".oxy-phone-row"));
  });
});

// Where the bar sits, with ui.css painted on: the reference's `px-4 py-6` (its LoadingBar,
// oxy-hq/reference-app@81ee8aa `src/components/Loading.tsx:43`). It was drawn at 32px over and
// under once, which set the bar and its sentence 8px lower than the reference (SDK-GAPS 5).
describe("LoadingBar, painted", () => {
  afterAll(() => unpaintUi());

  it("pads the bar 24px over and under and 16px at the sides, as the reference does", async () => {
    paintUi();
    await render(<LoadingBar label='Comparing stores…' progress={{ ready: 3, total: 5 }} />);
    const bar = getComputedStyle($(".oxy-ui-loading-bar") as HTMLElement);
    expect([bar.paddingTop, bar.paddingRight, bar.paddingBottom, bar.paddingLeft]).toEqual([
      "24px",
      "16px",
      "24px",
      "16px"
    ]);
  });
});
