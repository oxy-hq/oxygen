// @vitest-environment jsdom
//
// Words hidden from the eye and read to assistive tech (the 1px clip: placed absolutely, clipped
// to nothing) take their box from the nearest positioned ancestor. When that ancestor is outside
// the piece, the hidden words sit at their static place in ITS coordinates, outside any scroller
// the piece is in, and widen the page: MetricCell's sentence in the Stores table's sideways
// scroller took the shell's main as its box and pushed a 1440px page to 1588px (SDK-GAPS 1). So
// every piece that hides words anchors them itself: wherever the piece is placed (here, in a
// scroller with nothing positioned around it), the hidden words' box is inside the piece. Every
// 1px-clip class in ui.css must have a case here, so a new one cannot skip the check.

import type { ReactNode } from "react";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import { HoverHint } from "./containers";
import { CountBadge, StatusDot } from "./figures";
import { AttachmentsField } from "./forms";
import { DefaultTabs } from "./navigation";
import { paintUi, uiCssText, unpaintUi } from "./paint.harness";
import { MetricCell, PanelTable } from "./tables";
import { render } from "./ui.harness";

const noop = () => {};

/** The piece, once per way it hides words. */
const CASES: Record<string, () => ReactNode> = {
  "oxy-ui-metric-cell__hint": () => (
    <MetricCell
      observed='32.4%'
      comparison='24.0%'
      value={2}
      thresholds={{ warning: 1.5 }}
      hint='Food cost 32.4% against 24.0%: above comparable stores.'
    />
  ),
  "oxy-ui-panel-table__hidden": () => (
    <PanelTable
      caption='Discounts'
      columns={[{ header: "Name", cell: (r: { name: string }) => r.name }]}
      rows={[{ name: "Staff meal" }]}
      rowKey={(r) => r.name}
      actions={() => <button type='button'>Edit</button>}
    />
  ),
  "oxy-ui-attachments__sr": () => (
    <AttachmentsField
      files={[
        { id: "a", name: "slip.pdf", size: 2048, status: "uploading" },
        { id: "b", name: "dent.jpg", size: 4096, status: "failed", error: "Too large" }
      ]}
      onAdd={noop}
      onRemove={noop}
      onRetry={noop}
    />
  ),
  "oxy-ui-count-badge__label": () => <CountBadge count={3} label='3 overdue' />,
  "oxy-ui-status-dot__word--hidden": () => (
    <StatusDot status='success' label='Reconciled' wordHidden />
  ),
  "oxy-ui-hover-hint__words": () => (
    <HoverHint text='QuickBooks refused the bill' focusable>
      <span>failed</span>
    </HoverHint>
  ),
  "oxy-ui-status-dot__word": () => (
    <DefaultTabs
      group='measure'
      label='Measure'
      value='food'
      onChange={noop}
      options={[
        {
          value: "food",
          label: "Food cost",
          icon: <StatusDot status='warning' label='Above comparable stores' />
        }
      ]}
    />
  )
};

/** The classes ui.css hides with the 1px clip. */
function clippedClasses(): string[] {
  const found = new Set<string>();
  for (const m of uiCssText().matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    if (!/position:\s*absolute/.test(m[2]) || !/clip-path:\s*inset\(50%\)/.test(m[2])) continue;
    // The rule's subject: the last class of each selector in its list.
    for (const sel of m[1].split(",")) {
      const classes = [...sel.matchAll(/\.(oxy-ui-[a-z0-9_-]+)/g)];
      const subject = classes[classes.length - 1];
      if (subject) found.add(subject[1]);
    }
  }
  return [...found].sort();
}

/** The nearest ancestor that is positioned, as a browser picks an absolute box's container. */
function containingBlock(el: HTMLElement): HTMLElement | null {
  for (let p = el.parentElement; p; p = p.parentElement)
    if (getComputedStyle(p).position !== "static") return p;
  return null;
}

beforeAll(() => paintUi());
afterAll(() => unpaintUi());
afterEach(() => {
  document.body.innerHTML = "";
});

describe("visually hidden words", () => {
  it("has a case for every 1px-clip class in ui.css", () => {
    expect(Object.keys(CASES).sort()).toEqual(clippedClasses());
  });

  for (const [cls, piece] of Object.entries(CASES))
    it(`.${cls} is anchored inside its piece, in a scroller with nothing positioned around it`, async () => {
      const scroller = document.createElement("div");
      scroller.style.overflowX = "auto";
      document.body.append(scroller);
      await render(piece(), scroller);
      const hidden = [...scroller.querySelectorAll<HTMLElement>(`.${cls}`)];
      expect(hidden.length).toBeGreaterThan(0);
      for (const el of hidden) {
        expect(getComputedStyle(el).position).toBe("absolute");
        const box = containingBlock(el);
        expect(box, `.${cls} has no positioned box of its own`).not.toBeNull();
        expect(box !== scroller && scroller.contains(box)).toBe(true);
      }
    });
});
