// @vitest-environment jsdom
//
// PhoneHint replaces hover-only tooltips: a tap pins it, a second tap,
// Escape, a tap elsewhere or a scroll closes it. A mouse hover and keyboard
// focus open it too, but a touch's emulated hover must not, or the click that
// follows would close it again.

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { PhoneHint } from "./PhoneHint";
import { PhoneSheet } from "./PhoneSheet";
import { $, click, fire, press, render } from "./phone.harness";

afterEach(() => {
  document.body.innerHTML = "";
});

const button = () => $(".oxy-phone-hint") as HTMLElement;
const pop = () => $('[data-testid="phone-hint"]');

function pointer(type: string, pointerType: string): Event {
  // jsdom has no PointerEvent; a MouseEvent with the field is what React reads.
  const e = new MouseEvent(type, { bubbles: true });
  Object.defineProperty(e, "pointerType", { value: pointerType });
  return e;
}

describe("PhoneHint", () => {
  it("keeps a too-tall explanation on screen, and scrolling inside it does not close it", async () => {
    const tall = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "offsetHeight");
    Object.defineProperty(HTMLElement.prototype, "offsetHeight", {
      configurable: true,
      get: () => 900
    });
    const page = await render(<PhoneHint label='About'>a long explanation</PhoneHint>);
    button().getBoundingClientRect = () =>
      ({ top: 276, bottom: 300, left: 100, right: 124, width: 24, height: 24 }) as DOMRect;
    await click(button());
    const box = pop() as HTMLElement;
    expect(box.style.maxHeight).toBe(`${window.innerHeight - 32}px`);
    expect(box.style.top).toBe("16px");

    await fire(box, new Event("scroll"));
    expect(pop()).not.toBeNull();
    await fire(window as unknown as HTMLElement, new Event("scroll"));
    expect(pop()).toBeNull();

    if (tall) Object.defineProperty(HTMLElement.prototype, "offsetHeight", tall);
    await page.unmount();
  });

  it("toggles its explanation on tap", async () => {
    const page = await render(<PhoneHint label='About labor cost'>Payroll ÷ net sales</PhoneHint>);
    expect(button().getAttribute("aria-label")).toBe("About labor cost");
    expect(button().getAttribute("aria-expanded")).toBe("false");
    expect(pop()).toBeNull();

    await click(button());
    expect(pop()?.textContent).toBe("Payroll ÷ net sales");
    expect(button().getAttribute("aria-expanded")).toBe("true");
    expect(button().getAttribute("aria-describedby")).toBe(pop()?.id);

    await click(button());
    expect(pop()).toBeNull();
    await page.unmount();
  });

  it("closes on Escape (not the sheet under it), on a tap elsewhere, and on scroll", async () => {
    const page = await render(
      <PhoneSheet open onClose={() => {}} title='Store'>
        <PhoneHint label='About'>why</PhoneHint>
      </PhoneSheet>
    );
    button().focus();
    await click(button());
    await press("Escape");
    expect(pop()).toBeNull();
    expect($('[data-testid="phone-sheet"]')).not.toBeNull();
    expect(document.activeElement).toBe(button());

    await click(button());
    await fire(document.body, new MouseEvent("pointerdown", { bubbles: true }));
    expect(pop()).toBeNull();

    await click(button());
    await fire(window as unknown as Element, new Event("scroll"));
    expect(pop()).toBeNull();
    await page.unmount();
  });

  it("opens on a mouse hover and closes when the mouse leaves", async () => {
    const page = await render(<PhoneHint label='About'>why</PhoneHint>);
    await fire(button(), pointer("pointerover", "mouse"));
    expect(pop()).not.toBeNull();
    await fire(button(), pointer("pointerout", "mouse"));
    expect(pop()).toBeNull();
    await page.unmount();
  });

  it("ignores a touch's hover, so the tap that follows opens it", async () => {
    const page = await render(<PhoneHint label='About'>why</PhoneHint>);
    await fire(button(), pointer("pointerover", "touch"));
    expect(pop()).toBeNull();
    await click(button());
    expect(pop()).not.toBeNull();
    await page.unmount();
  });

  it("keeps the explanation 16px inside the viewport", async () => {
    window.innerWidth = 390;
    window.innerHeight = 844;
    const page = await render(<PhoneHint label='About'>why</PhoneHint>);
    // An ⓘ at the right edge of a phone.
    button().getBoundingClientRect = () =>
      ({
        left: 360,
        right: 384,
        top: 100,
        bottom: 124,
        width: 24,
        height: 24,
        x: 360,
        y: 100,
        toJSON: () => ({})
      }) as DOMRect;
    await click(button());
    const box = pop() as HTMLElement;
    const left = Number.parseFloat(box.style.left);
    const width = Number.parseFloat(box.style.width);
    expect(width).toBe(280);
    expect(left + width).toBeLessThanOrEqual(390 - 16);
    expect(left).toBeGreaterThanOrEqual(16);
    expect(Number.parseFloat(box.style.top)).toBe(132); // below, with an 8px gap
    await page.unmount();
  });

  describe("Escape, after a keyboard press has made focus visible", () => {
    // jsdom has no focus-visible heuristic. In a browser a key press turns it
    // on, so the focus that Escape hands back to the button must not reopen it.
    const original = Element.prototype.matches;
    beforeEach(() => {
      Element.prototype.matches = function (this: Element, sel: string) {
        return sel === ":focus-visible" ? true : original.call(this, sel);
      } as typeof Element.prototype.matches;
    });
    afterEach(() => {
      Element.prototype.matches = original;
    });

    it("closes a hint pinned by a tap that never focused it, as in Safari", async () => {
      const page = await render(<PhoneHint label='About'>why</PhoneHint>);
      (document.activeElement as HTMLElement | null)?.blur();
      await click(button());
      expect(pop()).not.toBeNull();
      expect(document.activeElement).not.toBe(button());
      await press("Escape");
      expect(pop()).toBeNull();
      expect(document.activeElement).toBe(button());
      await page.unmount();
    });

    it("closes a hovered hint and leaves focus in the field being typed in", async () => {
      const page = await render(
        <>
          <input data-testid='field' />
          <PhoneHint label='About'>why</PhoneHint>
        </>
      );
      const field = $('[data-testid="field"]') as HTMLElement;
      field.focus();
      await fire(button(), pointer("pointerover", "mouse"));
      expect(pop()).not.toBeNull();
      await press("Escape");
      expect(pop()).toBeNull();
      expect(document.activeElement).toBe(field);
      await page.unmount();
    });
  });
});
