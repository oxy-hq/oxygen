// @vitest-environment jsdom
//
// The modal side panel (decisions 40, 40b, 40c, 41): it opens as a named modal, moves focus in,
// keeps Tab inside, closes on Escape (not from a field, not when a layer inside took it) and on a
// press on the scrim, keeps its last content for the slide out, and gives focus back to the row
// that opened it, a click on the row's cells included. The scrim fades in with the slide on
// opacity alone (round 19); on a phone it is full screen with no scrim.

import { act, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { $, click, fire, press, render } from "../ui.harness";
import { declarations, stubMedia } from "./containers.harness";
import { SidePanel } from "./SidePanel";

afterEach(() => {
  document.body.innerHTML = "";
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

const dialog = () => $('[data-testid="side-panel"]') as HTMLDialogElement | null;
const named = (text: string) =>
  [...document.querySelectorAll("button")].find((b) => b.textContent === text) as HTMLElement;

/** A table whose rows open the panel, as an app's list does. */
function Page({ onClose }: { onClose?: () => void }) {
  const [store, setStore] = useState<string | null>(null);
  return (
    <>
      <table>
        <tbody>
          {["Downtown", "Harbor"].map((s) => (
            <tr key={s}>
              <td id={`cell-${s}`}>{s}</td>
              <td>
                <button type='button' onClick={() => setStore(s)}>{`Open ${s}`}</button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <SidePanel
        open={store !== null}
        label={store ? `${store} details` : "Store details"}
        width='34rem'
        onClose={() => {
          onClose?.();
          setStore(null);
        }}
      >
        {store && (
          <div>
            <p>{`${store} detail`}</p>
            <input aria-label='Note' />
            <button type='button'>First</button>
            <button type='button'>Last</button>
          </div>
        )}
      </SidePanel>
    </>
  );
}

async function slideOut() {
  await act(async () => {
    vi.advanceTimersByTime(300);
  });
}

describe("SidePanel", () => {
  it("opens as a modal named by its label, with focus inside, at its width", async () => {
    await render(<Page />);
    named("Open Downtown").focus();
    await click(named("Open Downtown"));
    const d = dialog();
    expect(d?.hasAttribute("open")).toBe(true);
    expect(d?.getAttribute("aria-label")).toBe("Downtown details");
    expect(d?.contains(document.activeElement)).toBe(true);
    expect(d?.className).toContain("oxy-ui-side-panel--shown");
    const panel = d?.querySelector<HTMLElement>(".oxy-ui-side-panel__panel");
    expect(panel?.style.getPropertyValue("--oxy-ui-side-panel-width")).toBe("34rem");
  });

  it("puts the caller's id on the dialog, for the aria-controls of the rows that open it", async () => {
    await render(
      <SidePanel open id='store-detail' label='Store' onClose={() => {}}>
        <button type='button'>Close</button>
      </SidePanel>
    );
    expect(dialog()?.id).toBe("store-detail");
    expect(document.getElementById("store-detail")).toBe(dialog());
  });

  it("keeps Tab inside: the last control wraps to the first and back", async () => {
    await render(<Page />);
    await click(named("Open Downtown"));
    named("Last").focus();
    await press("Tab");
    expect(document.activeElement).toBe($('[aria-label="Note"]'));
    await press("Tab", { shiftKey: true });
    expect(document.activeElement).toBe(named("Last"));
  });

  it("closes on Escape and gives focus back to the row that opened it, after the slide", async () => {
    vi.useFakeTimers();
    const onClose = vi.fn();
    await render(<Page onClose={onClose} />);
    named("Open Harbor").focus();
    await click(named("Open Harbor"));
    named("First").focus();
    await press("Escape");
    expect(onClose).toHaveBeenCalledTimes(1);
    // The slide out: still drawn, with the last content, untouchable.
    expect(dialog()?.className).toContain("oxy-ui-side-panel--closing");
    expect(dialog()?.className).not.toContain("oxy-ui-side-panel--shown");
    expect(dialog()?.textContent).toContain("Harbor detail");
    await slideOut();
    expect(dialog()).toBeNull();
    expect(document.activeElement).toBe(named("Open Harbor"));
  });

  it("gives focus back to the row's own control after a click on the row's cells", async () => {
    vi.useFakeTimers();
    await render(<Page />);
    // A click on a cell focuses nothing; the row's button opened it all the same.
    await fire($("#cell-Downtown"), new Event("pointerdown", { bubbles: true }));
    await click(named("Open Downtown"));
    (document.activeElement as HTMLElement | null)?.blur();
    await click(dialog());
    await slideOut();
    expect(document.activeElement).toBe(named("Open Downtown"));
  });

  it("closes on a press on the scrim, not on a press inside the panel", async () => {
    const onClose = vi.fn();
    await render(<Page onClose={onClose} />);
    await click(named("Open Downtown"));
    await click(named("First"));
    expect(onClose).not.toHaveBeenCalled();
    await click(dialog());
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("leaves Escape to a field being typed in, and to a layer inside that took it", async () => {
    const onClose = vi.fn();
    await render(
      <SidePanel open label='Detail' onClose={onClose}>
        <input aria-label='Search' />
        {/* biome-ignore lint/a11y/noStaticElementInteractions: a stand-in for a layer inside. */}
        <div
          onKeyDown={(e) => {
            if (e.key === "Escape") e.preventDefault();
          }}
        >
          <button type='button'>In a layer</button>
        </div>
        <button type='button'>Plain</button>
      </SidePanel>
    );
    $('[aria-label="Search"]')?.focus();
    await press("Escape");
    named("In a layer").focus();
    await press("Escape");
    expect(onClose).not.toHaveBeenCalled();
    named("Plain").focus();
    await press("Escape");
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("unmounts at once under reduced motion, focus back where it came from", async () => {
    stubMedia({ reduce: true });
    await render(<Page />);
    named("Open Downtown").focus();
    await click(named("Open Downtown"));
    await press("Escape");
    expect(dialog()).toBeNull();
    expect(document.activeElement).toBe(named("Open Downtown"));
  });

  it("on a phone is the whole screen with no scrim", async () => {
    stubMedia({ phone: true });
    await render(<Page />);
    await click(named("Open Downtown"));
    expect(dialog()?.className).toContain("oxy-ui-side-panel--phone");
    const backdrop = declarations(".oxy-ui-side-panel--phone::backdrop");
    expect(backdrop).toMatch(/background-color: transparent/);
    expect(backdrop).toMatch(/backdrop-filter: none/);
    const panel = declarations(".oxy-ui-side-panel--phone .oxy-ui-side-panel__panel");
    expect(panel).toMatch(/width: 100%/);
    expect(panel).toMatch(/padding-left: env\(safe-area-inset-left\)/);
  });

  it("is drawn the whole window's height, the scrim fading in with the slide on opacity alone", () => {
    const panel = declarations(".oxy-ui-side-panel__panel");
    expect(panel).toMatch(/top: 0/);
    expect(panel).toMatch(/bottom: 0/);
    expect(panel).toMatch(/padding-top: env\(safe-area-inset-top\)/);
    expect(panel).toMatch(/padding-bottom: env\(safe-area-inset-bottom\)/);
    // The scrim is the theme's, at nothing until the panel is shown (no full-strength first frame).
    expect(declarations(".oxy-ui-side-panel::backdrop")).toMatch(
      /background-color: var\(--scrim\).*opacity: 0/
    );
    expect(declarations(".oxy-ui-side-panel--shown::backdrop")).toMatch(/opacity: 1/);
    // Only opacity fades and only transform slides, and only where motion is welcome.
    const fade = declarations(
      ".oxy-ui-side-panel::backdrop",
      "prefers-reduced-motion: no-preference"
    );
    expect(fade).toMatch(/^ ?transition: opacity /);
    expect(fade).not.toMatch(/backdrop-filter|filter/);
    expect(
      declarations(".oxy-ui-side-panel__panel", "prefers-reduced-motion: no-preference")
    ).toMatch(/^ ?transition: transform /);
    expect(declarations(".oxy-ui-side-panel::backdrop")).not.toMatch(/transition/);
  });
});
