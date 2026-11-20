// @vitest-environment jsdom
//
// The titled section (quadrogen's test, on the SDK's recipes): a card with an eyebrow header over a
// rule, the title and its right-hand meta on one line, the body under it, and the caller's class.

import { afterEach, describe, expect, it } from "vitest";
import { ui } from "../recipes";
import { $, render } from "../ui.harness";
import { Panel } from "./Panel";

afterEach(() => {
  document.body.innerHTML = "";
});

describe("Panel", () => {
  it("is a card with an eyebrow header over a rule", async () => {
    await render(
      <Panel title='Inventory flow' right={<span id='meta'>As of 3:03 PM</span>} className='x'>
        <p id='body'>Body</p>
      </Panel>
    );
    const root = $(".oxy-ui-panel--titled") as HTMLElement;
    for (const c of [...ui.card.split(" "), ...ui.panel.split(" "), "x"])
      expect(root.classList.contains(c), c).toBe(true);
    const header = root.firstElementChild as HTMLElement;
    for (const c of [...ui.panelHeader.split(" "), ...ui.eyebrow.split(" ")])
      expect(header.classList.contains(c), c).toBe(true);
    expect(header.textContent).toBe("Inventory flowAs of 3:03 PM");
    expect(header.contains($("#meta"))).toBe(true);
    expect($("#body")?.parentElement?.className).toBe("oxy-ui-panel__body");
  });
});
