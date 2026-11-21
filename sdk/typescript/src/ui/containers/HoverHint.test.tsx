// @vitest-environment jsdom
//
// The hover hint: where its bubble goes and which presses use a control (BMG's cases, kept); a
// mouse hover opens it, a touch "hover" does not; keyboard focus opens it and the bubble describes
// the focused control; Escape closes it and only it; a tap on words opens it until the next tap
// anywhere, a tap on a control does not; inside an open dialog it portals into the dialog; with
// nothing to say it draws nothing; `label` makes the ⓘ a named button.

import { act } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { $, click, fire, press, render } from "../ui.harness";
import { HoverHint, placeHint, pressIsOnControl } from "./HoverHint";

afterEach(() => {
  document.body.innerHTML = "";
});

const bubble = () => $('[role="tooltip"]');
/** Focus from the keyboard (a Tab, then the focus it moves), inside act so the update it starts is
 *  drawn. A focus after a pointer press is not a keyboard focus (`:focus-visible` says no). */
const focusOn = async (el: Element | null | undefined) => {
  await act(async () => {
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", bubbles: true }));
    (el as HTMLElement | null)?.focus();
  });
};
const pointer = (type: string, pointerType: string) =>
  new PointerEvent(type, { bubbles: true, pointerType });

describe("placeHint", () => {
  const desktop = { width: 1440, height: 900 };
  const phone = { width: 390, height: 844 };

  it("leaves a desktop bubble right-aligned past the anchor, at its full width", () => {
    expect(placeHint({ top: 100, bottom: 120, right: 800 }, desktop, 460)).toEqual({
      top: 127,
      left: 806,
      below: true,
      maxWidth: 460
    });
  });

  it("keeps a desktop bubble off the left edge for an anchor near it", () => {
    const p = placeHint({ top: 100, bottom: 120, right: 40 }, desktop, 300);
    expect(p.left).toBe(312);
    expect(p.left - p.maxWidth).toBe(12);
  });

  it("opens above an anchor in the lower half", () => {
    const p = placeHint({ top: 700, bottom: 720, right: 800 }, desktop, 300);
    expect(p.below).toBe(false);
    expect(p.top).toBe(693);
  });

  it("narrows a wide bubble to the window on a phone, an edge clear on each side", () => {
    const p = placeHint({ top: 347, bottom: 379, right: 86 }, phone, 460);
    expect(p.maxWidth).toBe(366);
    expect(p.left).toBe(378);
    expect(p.left - p.maxWidth).toBe(12);
  });
});

describe("pressIsOnControl", () => {
  const el = (matches: boolean) => ({ closest: () => (matches ? {} : null) });

  it("treats a hint marked on-control, a column header, or a press in a control as a control", () => {
    expect(pressIsOnControl("x on-control y", false, null)).toBe(true);
    expect(pressIsOnControl(undefined, true, el(false))).toBe(true);
    expect(pressIsOnControl(undefined, false, el(true))).toBe(true);
  });

  it("treats a press on plain words as one to explain", () => {
    expect(pressIsOnControl(undefined, false, el(false))).toBe(false);
    expect(pressIsOnControl("wide-ish", false, el(false))).toBe(false);
    expect(pressIsOnControl(undefined, false, null)).toBe(false);
  });
});

describe("HoverHint", () => {
  it("opens on a mouse hover and closes when it leaves; a touch's enter opens nothing", async () => {
    await render(
      <HoverHint text='Three of five days over the cap.'>
        <span id='words'>3 / 5</span>
      </HoverHint>
    );
    const anchor = $(".oxy-ui-hover-hint");
    await fire(anchor, pointer("pointerover", "touch"));
    expect(bubble()).toBeNull();
    await fire(anchor, pointer("pointerover", "mouse"));
    expect(bubble()?.textContent).toBe("Three of five days over the cap.");
    expect(bubble()?.parentElement).toBe(document.body);
    await fire(anchor, pointer("pointerout", "mouse"));
    expect(bubble()).toBeNull();
  });

  it("opens on keyboard focus and describes the focused control; blur closes and undescribes", async () => {
    await render(
      <>
        <HoverHint text='Sorts by the latest week.'>
          <button type='button'>Sales</button>
        </HoverHint>
        <button type='button'>Next</button>
      </>
    );
    const sales = [...document.querySelectorAll("button")][0];
    await focusOn(sales);
    const tip = bubble();
    expect(tip).not.toBeNull();
    expect(sales.getAttribute("aria-describedby")).toBe(tip?.id);
    await focusOn([...document.querySelectorAll("button")][1]);
    expect(bubble()).toBeNull();
    expect(sales.hasAttribute("aria-describedby")).toBe(false);
  });

  it("closes on Escape, and keeps that Escape from closing anything around it", async () => {
    const outer = vi.fn();
    document.addEventListener("keydown", outer);
    await render(
      <HoverHint text='Sorts by the latest week.'>
        <button type='button'>Sales</button>
      </HoverHint>
    );
    await focusOn($("button"));
    expect(bubble()).not.toBeNull();
    await press("Escape");
    expect(bubble()).toBeNull();
    const escapeKey = outer.mock.calls
      .map((c) => c[0] as KeyboardEvent)
      .find((e) => e.key === "Escape");
    expect(escapeKey?.defaultPrevented).toBe(true);
    document.removeEventListener("keydown", outer);
  });

  it("on a phone: a tap on words opens it until the next tap anywhere; a tap on a control does not", async () => {
    await render(
      <>
        <HoverHint text='Paid over the statement.'>
          <span id='words'>$1,204</span>
        </HoverHint>
        <HoverHint text='Opens the plan.'>
          <button type='button'>Plans in flight</button>
        </HoverHint>
        <p id='elsewhere'>Elsewhere</p>
      </>
    );
    await fire($("#words"), pointer("pointerdown", "touch"));
    await click($("#words"));
    expect(bubble()?.textContent).toBe("Paid over the statement.");
    expect(bubble()?.className).toContain("oxy-ui-hover-hint__bubble--tapped");
    await fire($("#elsewhere"), pointer("pointerdown", "touch"));
    expect(bubble()).toBeNull();
    const plans = $("button") as HTMLElement;
    await fire(plans, pointer("pointerdown", "touch"));
    await focusOn(plans);
    await click(plans);
    expect(bubble()).toBeNull();
  });

  it("portals into an open dialog, which paints above the page", async () => {
    await render(
      <dialog open>
        <HoverHint text='Inside a panel.'>
          <span id='words'>3 / 5</span>
        </HoverHint>
      </dialog>
    );
    await fire($(".oxy-ui-hover-hint"), pointer("pointerover", "mouse"));
    expect(bubble()?.parentElement).toBe($("dialog"));
  });

  it("draws nothing extra with nothing to say", async () => {
    await render(
      <HoverHint text=''>
        <span id='words'>3 / 5</span>
      </HoverHint>
    );
    expect($(".oxy-ui-hover-hint")).toBeNull();
    expect($("#words")).not.toBeNull();
  });

  it("with a label, the ⓘ is a real button of that name", async () => {
    await render(
      <HoverHint text='Rent and fees.' label='About occupancy'>
        <svg aria-hidden='true' />
      </HoverHint>
    );
    const mark = $("button.oxy-ui-hover-hint__mark");
    expect(mark?.getAttribute("aria-label")).toBe("About occupancy");
    expect(mark?.getAttribute("type")).toBe("button");
    await focusOn(mark);
    expect(mark?.getAttribute("aria-describedby")).toBe(bubble()?.id);
  });

  it("focusable: the words become a Tab stop that opens the bubble and says them once", async () => {
    await render(
      <p>
        Posted{" "}
        <HoverHint text='QuickBooks refused the bill: the vendor is inactive.' focusable>
          <span id='why'>failed</span>
        </HoverHint>
      </p>
    );
    const anchor = $(".oxy-ui-hover-hint") as HTMLElement;
    expect(anchor.tabIndex).toBe(0);
    expect(anchor.classList).toContain("oxy-ui-focus");
    // The words ride in the anchor's own text, for assistive tech, hidden from the eye.
    const words = $(".oxy-ui-hover-hint__words") as HTMLElement;
    expect(words.textContent).toBe(" QuickBooks refused the bill: the vendor is inactive.");
    expect(anchor.textContent).toBe("failed QuickBooks refused the bill: the vendor is inactive.");
    await focusOn(anchor);
    expect(bubble()?.textContent).toBe("QuickBooks refused the bill: the vendor is inactive.");
    // Not described by the bubble too: the words would be read twice.
    expect(anchor.hasAttribute("aria-describedby")).toBe(false);
    await press("Escape");
    expect(bubble()).toBeNull();
  });

  it("without focusable, words stay out of the Tab order and carry no hidden copy", async () => {
    await render(
      <HoverHint text='Posted 3 Nov'>
        <span>Posted</span>
      </HoverHint>
    );
    expect($(".oxy-ui-hover-hint")?.hasAttribute("tabindex")).toBe(false);
    expect($(".oxy-ui-hover-hint__words")).toBeNull();
  });
});
