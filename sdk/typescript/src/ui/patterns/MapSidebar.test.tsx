// @vitest-environment jsdom
//
// MapSidebar and MapDetailCard: one panel, two views. The overview lays out its header and
// sections; picking a row from inside the panel opens the detail in its place with focus on the
// back arrow, and the overview stays mounted (hidden), so its scroll position survives; the back
// arrow returns to the overview and puts focus back on the row it was opened from, or on the
// overview's title when that row is gone; a detail opened from outside the panel leaves focus
// where it is; a detail replaced while focus is in it hands focus to the new back arrow. The
// section's disclosure and the tile's pressed state are driven too.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { $, $$, click, render } from "../ui.harness";
import { MapDetailCard } from "./MapDetailCard";
import { MapSidebar } from "./MapSidebar";
import { MapSection, MapTile } from "./mapPanel";

afterEach(() => {
  document.body.innerHTML = "";
});

const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "patterns.css"), "utf8");

const PLACES = ["North", "Harbour", "Midtown"];

/** A screen: a pin outside the panel, the panel, and its ranked rows. */
function Screen({ initial = null as string | null, rows = PLACES }) {
  const [open, setOpen] = useState<string | null>(initial);
  return (
    <>
      <button type='button' data-pin='' onClick={() => setOpen("Harbour")}>
        Pin
      </button>
      <button type='button' data-pin-next='' onClick={() => setOpen("Midtown")}>
        Next pin
      </button>
      <MapSidebar
        title='The region'
        meta='3 places'
        label='Region overview'
        detailLabel={open ? `${open} detail` : undefined}
        detail={
          open && (
            <MapDetailCard
              key={open}
              title={open}
              meta='rank 1'
              backLabel='Back to places'
              onBack={() => setOpen(null)}
              actions={<button type='button'>Save</button>}
            >
              <MapSection title='Why here'>
                <MapTile>Close to the road</MapTile>
              </MapSection>
            </MapDetailCard>
          )
        }
      >
        <MapSection title='Ranked' meta={`${rows.length} ranked`} foot='click a row to inspect'>
          <ul>
            {rows.map((p) => (
              <li key={p}>
                <MapTile onClick={() => setOpen(p)} pressed={open === p}>
                  {p}
                </MapTile>
              </li>
            ))}
          </ul>
        </MapSection>
      </MapSidebar>
    </>
  );
}

/** The same screen, with a way to take rows out of the list while a detail is open. */
function Shrinking() {
  const [rows, setRows] = useState(PLACES);
  return (
    <>
      <button type='button' data-drop='' onClick={() => setRows(["North"])}>
        Drop
      </button>
      <Screen rows={rows} />
    </>
  );
}

const aside = () => $("aside") as HTMLElement;
const row = (name: string) =>
  $$(".oxy-ui-map-tile--action").find((t) => t.textContent === name) as HTMLButtonElement;
const back = () => $("[data-oxy-ui-map-back]") as HTMLButtonElement;
const overview = () => aside().firstElementChild as HTMLElement;

describe("MapSidebar", () => {
  it("draws the overview: one header, the sections, named for the view", async () => {
    await render(<Screen />);
    expect(aside().getAttribute("aria-label")).toBe("Region overview");
    expect($("h2")?.textContent).toBe("The region");
    expect($(".oxy-ui-map-panel__meta")?.textContent).toBe("3 places");
    expect($(".oxy-ui-map-section__head")?.textContent).toBe("Ranked3 ranked");
    expect($(".oxy-ui-map-section__foot")?.textContent).toBe("click a row to inspect");
    expect($$(".oxy-ui-map-tile--action").map((t) => t.getAttribute("aria-pressed"))).toEqual([
      "false",
      "false",
      "false"
    ]);
    expect(back()).toBeNull();
  });

  it("opens a row in place with focus on the back arrow, the overview kept mounted", async () => {
    await render(<Screen />);
    const list = overview();
    row("Harbour").focus();
    await click(row("Harbour"));
    expect(aside().getAttribute("aria-label")).toBe("Harbour detail");
    expect(list.hidden).toBe(true);
    expect(list.isConnected).toBe(true);
    // Hidden for real: the view's own `display: flex` must not beat the attribute (jsdom's
    // computed style cannot tell, so the rule is read from the stylesheet).
    expect(css).toMatch(/\.oxy-ui-map-view \{\s*display: flex;/);
    expect(css).toMatch(/\.oxy-ui-map-view\[hidden\] \{\s*display: none;\s*\}/);
    expect(row("Harbour").getAttribute("aria-pressed")).toBe("true");
    expect(document.activeElement).toBe(back());
    expect(back().getAttribute("aria-label")).toBe("Back to places");
    expect($$("h2").map((h) => h.textContent)).toEqual(["The region", "Harbour"]);
  });

  it("goes back to the same overview, with focus on the row it was opened from", async () => {
    await render(<Screen />);
    const list = overview();
    row("Midtown").focus();
    await click(row("Midtown"));
    await click(back());
    expect(aside().getAttribute("aria-label")).toBe("Region overview");
    expect(overview()).toBe(list);
    expect(list.hidden).toBe(false);
    expect($("[data-oxy-ui-map-detail]")).toBeNull();
    expect(document.activeElement).toBe(row("Midtown"));
  });

  it("puts focus on the overview's title when the row it came from is gone", async () => {
    await render(<Shrinking />);
    row("Harbour").focus();
    await click(row("Harbour"));
    expect(document.activeElement).toBe(back());
    // The list changes under the open detail: its rows are now only "North".
    const drop = $("[data-drop]") as HTMLButtonElement;
    await click(drop);
    back().focus();
    await click(back());
    expect(row("Harbour")).toBeUndefined();
    expect(document.activeElement).toBe($("h2"));
  });

  it("leaves focus where it is when the detail is opened from outside the panel", async () => {
    await render(<Screen />);
    const pin = $("[data-pin]") as HTMLButtonElement;
    pin.focus();
    await click(pin);
    expect(aside().getAttribute("aria-label")).toBe("Harbour detail");
    expect(document.activeElement).toBe(pin);
  });

  it("hands focus to the new back arrow when another item replaces the open one", async () => {
    await render(<Screen />);
    row("Harbour").focus();
    await click(row("Harbour"));
    const first = back();
    expect(document.activeElement).toBe(first);
    // A pin pressed by pointer while focus is on the back arrow: the detail is replaced, and the
    // element that held focus goes with it.
    await click($("[data-pin-next]"));
    expect(aside().getAttribute("aria-label")).toBe("Midtown detail");
    expect(first.isConnected).toBe(false);
    expect(document.activeElement).toBe(back());
  });
});

describe("MapDetailCard", () => {
  it("draws its header, sections and actions; back calls onBack; framed draws a frame", async () => {
    const onBack = vi.fn();
    await render(
      <MapDetailCard title='Harbour' meta='rank 2' onBack={onBack} framed actions='ACTIONS'>
        <MapSection title='Trade area'>figures</MapSection>
      </MapDetailCard>
    );
    const card = $("[data-oxy-ui-map-detail]") as HTMLElement;
    expect(card.classList).toContain("oxy-ui-map-panel");
    expect(back().getAttribute("aria-label")).toBe("Back");
    expect(back().classList).toContain("oxy-ui-btn--text");
    expect($("h2")?.textContent).toBe("Harbour");
    expect($(".oxy-ui-map-panel__actions")?.textContent).toBe("ACTIONS");
    expect($(".oxy-ui-map-panel__body")?.lastElementChild?.className).toBe(
      "oxy-ui-map-panel__actions"
    );
    await click(back());
    expect(onBack).toHaveBeenCalledTimes(1);
  });
});

describe("MapSection and MapTile", () => {
  it("folds a collapsible section until asked, and says which way it goes", async () => {
    await render(
      <MapSection title='Signal breakdown' collapsible foot='per hex'>
        <dl data-rows=''>rows</dl>
      </MapSection>
    );
    const toggle = $(".oxy-ui-map-section__toggle") as HTMLButtonElement;
    expect(toggle.closest("h3")).not.toBeNull();
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(toggle.textContent).toBe("Signal breakdownshow all");
    expect($("[data-rows]")).toBeNull();
    expect($(".oxy-ui-map-section__foot")).toBeNull();
    await click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    expect(toggle.textContent).toBe("Signal breakdownhide");
    const body = $("[data-rows]")?.parentElement as HTMLElement;
    expect(toggle.getAttribute("aria-controls")).toBe(body.id);
    expect($(".oxy-ui-map-section__foot")?.textContent).toBe("per hex");
    await click(toggle);
    expect($("[data-rows]")).toBeNull();
  });

  it("is a plain box without onClick, a pressed button with it", async () => {
    const onClick = vi.fn();
    await render(
      <>
        <MapTile layout='row'>reason</MapTile>
        <MapTile onClick={onClick} pressed>
          chosen
        </MapTile>
      </>
    );
    const [plain, pick] = $$(".oxy-ui-map-tile");
    expect(plain.tagName).toBe("DIV");
    expect(plain.classList).toContain("oxy-ui-map-tile--row");
    expect(pick.tagName).toBe("BUTTON");
    expect(pick.getAttribute("aria-pressed")).toBe("true");
    await click(pick);
    expect(onClick).toHaveBeenCalledTimes(1);
  });
});
