// @vitest-environment jsdom
//
// The titled section (quadrogen's test, on the SDK's recipes): a card with an eyebrow header over a
// rule, the title and its right-hand meta on one line, the body under it, and the caller's class.
// 2.31.0: nothing changes without the new options; the fold (a caret button with its expanded
// state, the body and rule gone, held or not), a header of the caller's, a heading title and a
// landmark named by it, the compact density with its eyebrow on the title alone, no header at all.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";
import { ui } from "../recipes";
import { $, click, render } from "../ui.harness";
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

describe("Panel, 2.31.0 options", () => {
  it("draws the same panel as before when none of them is passed", async () => {
    await render(
      <Panel title='Inventory flow' right='As of 3:03 PM'>
        <p>Body</p>
      </Panel>
    );
    const root = $(".oxy-ui-panel--titled") as HTMLElement;
    expect(root.tagName).toBe("DIV");
    expect(root.getAttribute("aria-label")).toBeNull();
    expect(root.querySelector("button, h2, h3, h4")).toBeNull();
    const header = root.firstElementChild as HTMLElement;
    expect(header.innerHTML).toBe("<span>Inventory flow</span>As of 3:03 PM");
    for (const c of ["oxy-ui-panel--compact", "oxy-ui-panel--folded"])
      expect(root.classList.contains(c), c).toBe(false);
  });

  it("folds behind a caret button on the title: expanded state, the body gone, the rule dropped", async () => {
    const seen: boolean[] = [];
    await render(
      <Panel title='Layers' foldable right={(open) => (open ? "3 on" : "3 on, folded")}>
        <p id='body'>Body</p>
      </Panel>
    );
    const fold = $(".oxy-ui-panel__fold") as HTMLButtonElement;
    expect(fold.tagName).toBe("BUTTON");
    expect(fold.textContent).toBe("Layers");
    expect(fold.getAttribute("aria-expanded")).toBe("true");
    const body = $("#body")?.parentElement as HTMLElement;
    expect(fold.getAttribute("aria-controls")).toBe(body.id);
    expect($(".oxy-ui-panel-header")?.textContent).toBe("Layers3 on");
    fold.focus();
    await click(fold);
    seen.push(fold.getAttribute("aria-expanded") === "true");
    expect(seen).toEqual([false]);
    expect($("#body")).toBeNull();
    expect(fold.getAttribute("aria-controls")).toBeNull();
    expect($(".oxy-ui-panel--titled")?.classList).toContain("oxy-ui-panel--folded");
    expect($(".oxy-ui-panel-header")?.textContent).toBe("Layers3 on, folded");
    // Focus stays on the fold, so the reader can open it again from where they are.
    expect(document.activeElement).toBe(fold);
    await click(fold);
    expect(fold.getAttribute("aria-expanded")).toBe("true");
    expect($("#body")).not.toBeNull();
  });

  it("starts folded with defaultOpen={false}, and a held fold asks the caller", async () => {
    await render(
      <Panel title='Network snapshot' foldable defaultOpen={false}>
        <p id='body'>Body</p>
      </Panel>
    );
    expect($("#body")).toBeNull();
    document.body.innerHTML = "";
    const asked: boolean[] = [];
    const page = await render(
      <Panel title='Top candidates' foldable open onOpenChange={(o) => asked.push(o)}>
        <p id='held'>Body</p>
      </Panel>
    );
    await click($(".oxy-ui-panel__fold"));
    expect(asked).toEqual([false]);
    // Held: nothing folds until the caller says so.
    expect($("#held")).not.toBeNull();
    await page.rerender(
      <Panel title='Top candidates' foldable open={false} onOpenChange={(o) => asked.push(o)}>
        <p id='held'>Body</p>
      </Panel>
    );
    expect($("#held")).toBeNull();
  });

  it("folds to its header from the caller's own control (open, no foldable)", async () => {
    await render(
      <Panel header={<button type='button'>Collapse</button>} open={false}>
        <p id='body'>Body</p>
      </Panel>
    );
    expect($("#body")).toBeNull();
    expect($(".oxy-ui-panel__fold")).toBeNull();
    expect($(".oxy-ui-panel--titled")?.classList).toContain("oxy-ui-panel--folded");
  });

  it("takes a whole header in place of the eyebrow title, in its own type", async () => {
    await render(
      <Panel header={<span id='own'>Harbour Point</span>} right={<span id='meta'>Live</span>}>
        <p>Body</p>
      </Panel>
    );
    const header = $(".oxy-ui-panel-header") as HTMLElement;
    expect(header.contains($("#own"))).toBe(true);
    expect(header.contains($("#meta"))).toBe(true);
    for (const c of ui.eyebrow.split(" ")) expect(header.classList.contains(c), c).toBe(false);
  });

  it("titles with a heading, and as a section or aside is a landmark named by it or by label", async () => {
    await render(
      <>
        <Panel title='Assumptions' headingLevel={2} as='section'>
          <p>Body</p>
        </Panel>
        <Panel title='Live events' as='aside' label='Live events rail'>
          <p>Body</p>
        </Panel>
      </>
    );
    const h2 = $("h2") as HTMLElement;
    expect(h2.textContent).toBe("Assumptions");
    expect(h2.className).toBe("oxy-ui-panel__title");
    const section = $("section") as HTMLElement;
    expect(section.getAttribute("aria-labelledby")).toBe(h2.id);
    expect(section.getAttribute("aria-label")).toBeNull();
    expect($("aside")?.getAttribute("aria-label")).toBe("Live events rail");
  });

  it("names a landmark by its heading only when the heading is drawn", async () => {
    await render(
      <Panel as='section' headingLevel={2} title='Assumptions' header={<span>Own header</span>}>
        <p>Body</p>
      </Panel>
    );
    const section = $("section") as HTMLElement;
    expect($("h2")).toBeNull();
    // aria-labelledby must never point at an id that is not on the page.
    expect(section.getAttribute("aria-labelledby")).toBeNull();
  });

  it("does not name a landmark by an empty heading when the title is left out", async () => {
    await render(
      <Panel as='section' headingLevel={2}>
        <p>Body</p>
      </Panel>
    );
    expect(($("section") as HTMLElement).getAttribute("aria-labelledby")).toBeNull();
  });

  it("puts a fold inside its heading, the disclosure pattern", async () => {
    await render(
      <Panel title='Layers' headingLevel={3} foldable>
        <p>Body</p>
      </Panel>
    );
    expect($("h3 > button.oxy-ui-panel__fold")?.textContent).toBe("Layers");
  });

  it("is compact on request: its own class, and the eyebrow on the title alone", async () => {
    await render(
      <Panel title='Open findings' right={<span id='pill'>3 open</span>} density='compact'>
        <p>Body</p>
      </Panel>
    );
    const root = $(".oxy-ui-panel--titled") as HTMLElement;
    expect(root.classList).toContain("oxy-ui-panel--compact");
    const header = root.firstElementChild as HTMLElement;
    for (const c of ui.eyebrow.split(" ")) {
      expect(header.classList.contains(c), c).toBe(false);
      expect(header.firstElementChild?.classList.contains(c), c).toBe(true);
    }
    expect(($("#pill") as HTMLElement).closest(`.${ui.eyebrow}`)).toBeNull();
  });

  it("draws no header (and no rule) with no title, and puts the caller's class on the body", async () => {
    await render(
      <Panel bodyClassName='scrolls'>
        <p id='body'>Body</p>
      </Panel>
    );
    expect($(".oxy-ui-panel-header")).toBeNull();
    expect($("#body")?.parentElement?.className).toBe("oxy-ui-panel__body scrolls");
  });

  it("puts the data-* props it is given on its root, as a div or a section (2.35.0)", async () => {
    await render(
      <>
        <Panel title='Waste catalog' data-tour='waste-catalog' data-testid='catalog'>
          <p>Body</p>
        </Panel>
        <Panel as='section' headingLevel={2} title='Schedules' data-tour='schedules' foldable>
          <p>Body</p>
        </Panel>
      </>
    );
    const [plain, section] = [
      ...document.querySelectorAll(".oxy-ui-panel--titled")
    ] as HTMLElement[];
    expect(plain.getAttribute("data-tour")).toBe("waste-catalog");
    expect(plain.getAttribute("data-testid")).toBe("catalog");
    expect(section.tagName).toBe("SECTION");
    expect(section.getAttribute("data-tour")).toBe("schedules");
    expect(section.getAttribute("aria-labelledby")).not.toBeNull();
    expect(document.querySelectorAll("[data-tour]")).toHaveLength(2);
  });

  it("styles each part it draws (the uiCss test checks the classes exist)", () => {
    const css = readFileSync(
      join(dirname(fileURLToPath(import.meta.url)), "containers.css"),
      "utf8"
    );
    expect(css).toMatch(
      /\.oxy-ui-panel--folded > \.oxy-ui-panel-header \{[^}]*border-bottom-width: 0/
    );
    expect(css).toMatch(/\.oxy-ui-panel--compact \{[^}]*padding: var\(--padding-sm-x\)/);
    expect(css).toMatch(
      /\.oxy-ui-panel--compact > \.oxy-ui-panel__body \{[^}]*--type-scale-xs-size/
    );
  });
});
