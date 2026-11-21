// @vitest-environment jsdom
//
// What the 2.31.0 gap round changed in ui.css, read where the rules meet (ui.css painted onto the
// document, paint.harness.ts): the failed-read message is 13px at either size (decision 85); a
// ConfirmDialog's paragraphs and lists lose the browser's margins, so the body's gap is the only
// space between them in an app with no reset (Bookkeeping); and the inset ring's forced-colours
// outline sits inside the box, where its clipping container cannot cut it (Warehouse, Command
// Center), while the outside ring's stays out.

import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import { Button } from "./actions";
import { LoadError } from "./feedback";
import { paintUi, unpaintUi } from "./paint.harness";
import { $, render } from "./ui.harness";

/** A browser's own margins on paragraphs and lists, as a document without a reset has them. The
 *  painted ui.css is one flat sheet, so a bare element rule stands in for the browser's own sheet:
 *  any class rule outranks it, as any author rule outranks the browser's. */
let ua: HTMLStyleElement;
beforeAll(() => {
  ua = document.createElement("style");
  ua.textContent = "p, ul { margin-top: 16px; margin-bottom: 16px; }";
  document.head.prepend(ua);
  paintUi();
});
afterAll(() => {
  unpaintUi();
  ua.remove();
});
afterEach(() => {
  document.body.innerHTML = "";
});

describe("ui.css after the gap round", () => {
  it("draws the failed-read message at 13px whether or not it is lg, on the callout's line", async () => {
    for (const size of ["sm", "lg"] as const) {
      const page = await render(
        <LoadError size={size} onRetry={() => {}}>
          The counts did not load.
        </LoadError>
      );
      const message = $('[role="alert"]') as HTMLElement;
      const style = getComputedStyle(message);
      expect(style.fontSize, size).toBe("13px");
      expect(style.lineHeight, size).toBe("1.45");
      await page.unmount();
    }
  });

  it("takes the browser's margins off a ConfirmDialog body's own children", async () => {
    await render(
      <div className='oxy-ui-confirm__body'>
        <p id='p'>Delete this count?</p>
        <ul id='ul'>
          <li>3 lines</li>
        </ul>
      </div>
    );
    for (const id of ["p", "ul"]) {
      const style = getComputedStyle($(`#${id}`) as HTMLElement);
      expect([parseFloat(style.marginTop), parseFloat(style.marginBottom)], id).toEqual([0, 0]);
    }
  });

  it("draws the inset ring's forced-colours outline inside, the outside ring's outside", async () => {
    await render(
      <>
        <button type='button' id='seg' className='oxy-ui-segment'>
          Month
        </button>
        <span id='row' tabIndex={-1} className='oxy-ui-focus-inset'>
          Row
        </span>
        <Button id='btn'>Save</Button>
      </>
    );
    for (const [id, offset] of [
      ["seg", "-2px"],
      ["row", "-2px"],
      ["btn", "2px"]
    ] as const) {
      const el = $(`#${id}`) as HTMLElement;
      el.focus();
      expect(document.activeElement, id).toBe(el);
      expect(getComputedStyle(el).outlineOffset, id).toBe(offset);
    }
  });
});
