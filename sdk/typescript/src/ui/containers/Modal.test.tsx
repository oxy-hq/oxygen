// @vitest-environment jsdom
//
// The centred modal (decision 41): it opens named by its title with focus on the body (nothing
// looks pre-selected) or on `initialFocus`, keeps Tab inside, closes by Escape, the backdrop and
// the X after its fade (at once under reduced motion), not while busy, gives focus back to what
// opened it, pins the page behind, and on a phone opens the kit's full-screen sheet instead,
// unless the app provides its own `ModalFrame`.

import { act, useRef, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { $, $$, click, press, render } from "../ui.harness";
import { declarations, stubMedia } from "./containers.harness";
import { Modal, ModalFrame } from "./Modal";

afterEach(() => {
  document.body.innerHTML = "";
  document.body.style.overflow = "";
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

const dialog = () => $("dialog.oxy-ui-modal") as HTMLDialogElement | null;
const named = (text: string) =>
  [...document.querySelectorAll("button")].find(
    (b) => b.textContent === text || b.getAttribute("aria-label") === text
  ) as HTMLElement;

function Page({
  busy = false,
  withInitial = false,
  onClose
}: {
  busy?: boolean;
  withInitial?: boolean;
  onClose?: () => void;
}) {
  const [open, setOpen] = useState(false);
  const search = useRef<HTMLInputElement>(null);
  return (
    <>
      <button type='button' onClick={() => setOpen(true)}>
        Open flag
      </button>
      <Modal
        open={open}
        busy={busy}
        title='Discount over cap'
        initialFocus={withInitial ? search : undefined}
        onClose={() => {
          onClose?.();
          setOpen(false);
        }}
        footer={<button type='button'>Done</button>}
      >
        <input ref={search} aria-label='Search' />
        <p>Three of five days.</p>
      </Modal>
    </>
  );
}

async function openIt() {
  named("Open flag").focus();
  await click(named("Open flag"));
}

/** The exit fade's end: a transitionend on the dialog's own opacity. */
async function fadeOut() {
  const ev = new Event("transitionend");
  Object.defineProperty(ev, "propertyName", { value: "opacity" });
  await act(async () => {
    dialog()?.dispatchEvent(ev);
  });
}

describe("Modal", () => {
  it("opens named by its title, focus on the body rather than on Close", async () => {
    await render(<Page />);
    await openIt();
    const d = dialog();
    expect(d?.hasAttribute("open")).toBe(true);
    const title = document.getElementById(d?.getAttribute("aria-labelledby") ?? "");
    expect(title?.textContent).toBe("Discount over cap");
    expect(document.activeElement).toBe($(".oxy-ui-modal__body"));
    expect(document.body.style.overflow).toBe("hidden");
  });

  it("lands on initialFocus when the caller names one", async () => {
    await render(<Page withInitial />);
    await openIt();
    expect(document.activeElement).toBe($('[aria-label="Search"]'));
  });

  it("keeps Tab inside, from the X round to Done", async () => {
    await render(<Page />);
    await openIt();
    named("Done").focus();
    await press("Tab");
    expect(document.activeElement).toBe(named("Close"));
    await press("Tab", { shiftKey: true });
    expect(document.activeElement).toBe(named("Done"));
  });

  it("closes on Escape after its fade, focus back on the opener, the page free again", async () => {
    const onClose = vi.fn();
    await render(<Page onClose={onClose} />);
    await openIt();
    await press("Escape");
    expect(dialog()?.className).toContain("oxy-ui-modal--closing");
    expect(onClose).not.toHaveBeenCalled();
    await fadeOut();
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(dialog()?.hasAttribute("open")).toBe(false);
    expect(document.activeElement).toBe(named("Open flag"));
    expect(document.body.style.overflow).toBe("");
  });

  it("closes on the backdrop and on the X, not on a press inside the panel", async () => {
    vi.useFakeTimers();
    const onClose = vi.fn();
    await render(<Page onClose={onClose} />);
    await openIt();
    await click($(".oxy-ui-modal__body"));
    await act(async () => {
      vi.advanceTimersByTime(500);
    });
    expect(onClose).not.toHaveBeenCalled();
    await click(dialog());
    // No transition ran: the backstop closes it.
    await act(async () => {
      vi.advanceTimersByTime(450);
    });
    expect(onClose).toHaveBeenCalledTimes(1);
    await openIt();
    await click(named("Close"));
    await fadeOut();
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it("does not close while busy: Escape, the backdrop and the X wait for the answer", async () => {
    const onClose = vi.fn();
    const page = await render(<Page onClose={onClose} />);
    await openIt();
    await page.rerender(<Page busy onClose={onClose} />);
    await press("Escape");
    await click(dialog());
    await click(named("Close"));
    await fadeOut();
    expect(onClose).not.toHaveBeenCalled();
    expect(named("Close").getAttribute("aria-disabled")).toBe("true");
  });

  it("closes at once under reduced motion", async () => {
    stubMedia({ reduce: true });
    const onClose = vi.fn();
    await render(<Page onClose={onClose} />);
    await openIt();
    await press("Escape");
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(named("Open flag"));
  });

  it("gives two modals two title ids", async () => {
    await render(
      <>
        <Modal open title='One' onClose={() => {}}>
          a
        </Modal>
        <Modal open title='Two' onClose={() => {}}>
          b
        </Modal>
      </>
    );
    const ids = $$("dialog.oxy-ui-modal").map((d) => d.getAttribute("aria-labelledby"));
    expect(new Set(ids).size).toBe(2);
  });

  it("on a phone opens the kit's full-screen sheet with Back, which closes it", async () => {
    stubMedia({ phone: true });
    const onClose = vi.fn();
    await render(
      <Modal open title='Discount over cap' backLabel='Flags' onClose={onClose}>
        Body
      </Modal>
    );
    expect(dialog()).toBeNull();
    const sheet = $('[data-testid="phone-sheet"]');
    expect(sheet?.getAttribute("aria-modal")).toBe("true");
    expect(sheet?.textContent).toContain("Discount over cap");
    await click(named("Flags"));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("draws what a ModalFrame provides instead", async () => {
    await render(
      <ModalFrame.Provider value={(p) => <section id='frame'>{p.title}</section>}>
        <Modal open title='Framed' onClose={() => {}}>
          Body
        </Modal>
      </ModalFrame.Provider>
    );
    expect(dialog()).toBeNull();
    expect($("#frame")?.textContent).toBe("Framed");
  });

  it("scrims with the theme's --scrim, fading in only where motion is welcome", () => {
    expect(declarations(".oxy-ui-modal::backdrop")).toMatch(/background-color: var\(--scrim\)/);
    expect(declarations(".oxy-ui-modal[open]::backdrop")).toMatch(/opacity: 1/);
    expect(declarations(".oxy-ui-modal")).not.toMatch(/transition/);
    expect(declarations(".oxy-ui-modal", "prefers-reduced-motion: no-preference")).toMatch(
      /transition: opacity/
    );
  });
});
