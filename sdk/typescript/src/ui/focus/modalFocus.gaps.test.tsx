// @vitest-environment jsdom
//
// The modal helper's 2.31.0 changes: a closed <details> hides its controls from the trap, so Tab
// from the real last control wraps (Platform Pulse's area panel); and `{ auto: true }` does the
// whole job for a dialog an app draws itself, including one that mounts open and focuses a field
// of its own before any effect runs (BMG Inventory Management's `.review-backdrop` sheets).

import { act, useRef, useState } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { $, click, press, render } from "../ui.harness";
import { focusableIn, useModalFocus } from "./modalFocus";

afterEach(() => {
  document.body.innerHTML = "";
});

const named = (text: string) =>
  [...document.querySelectorAll<HTMLElement>("button, a, input, summary")].find(
    (b) => b.textContent === text || b.getAttribute("aria-label") === text
  ) as HTMLElement;

describe("focusableIn and a closed <details>", () => {
  it("leaves out what a closed details hides, keeps its summary, and counts an open one whole", async () => {
    await render(
      <div id='root'>
        <button type='button'>First</button>
        <details>
          <summary>How it is answered</summary>
          <a href='/method'>Method</a>
        </details>
        <details open>
          <summary>Sources</summary>
          <a href='/census'>Census</a>
          <details>
            <summary>Older</summary>
            <a href='/old'>Old</a>
          </details>
        </details>
      </div>
    );
    const stops = focusableIn($("#root") as HTMLElement).map((el) => el.textContent);
    expect(stops).toEqual(["First", "How it is answered", "Sources", "Census", "Older"]);
  });

  it("a summary inside a closed details is hidden too", async () => {
    await render(
      <div id='root'>
        <details>
          <summary>Outer</summary>
          <details open>
            <summary>Inner</summary>
            <a href='/x'>Deep</a>
          </details>
        </details>
      </div>
    );
    expect(focusableIn($("#root") as HTMLElement).map((el) => el.textContent)).toEqual(["Outer"]);
  });
});

/** A dialog drawn by hand, mounted only while open, focusing its own field on mount. */
function Sheet({ onDone }: { onDone: () => void }) {
  const root = useRef<HTMLDivElement>(null);
  useModalFocus(root, true, { auto: true });
  return (
    <div ref={root} role='dialog' aria-modal='true' aria-label='Shipment review'>
      {/* biome-ignore lint/a11y/noAutofocus: the hand-drawn dialog this case stands for does. */}
      <input aria-label='Note' autoFocus />
      <details>
        <summary>Why held</summary>
        <a href='/why'>Rule</a>
      </details>
      <button type='button' onClick={onDone}>
        Done
      </button>
    </div>
  );
}

/** A dialog drawn by hand that stays mounted and shows by `open`, with no field of its own. */
function Shown({ open, onDone }: { open: boolean; onDone: () => void }) {
  const root = useRef<HTMLDivElement>(null);
  useModalFocus(root, open, { auto: true });
  return (
    <div ref={root} role='dialog' aria-label='Digest' hidden={!open}>
      <button type='button'>Mark read</button>
      <button type='button' onClick={onDone}>
        Close
      </button>
    </div>
  );
}

/** The 2.30.0 call, with no options: the hook traps and returns, and moves nothing itself. */
function Bare() {
  const root = useRef<HTMLDivElement>(null);
  useModalFocus(root, true);
  return (
    <div ref={root}>
      <button type='button'>Inside</button>
    </div>
  );
}

function Page({ kind }: { kind: "mounted" | "shown" }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button type='button'>Before</button>
      <button type='button' onClick={() => setOpen(true)}>
        Review shipment
      </button>
      {kind === "mounted" ? (
        open && <Sheet onDone={() => setOpen(false)} />
      ) : (
        <Shown open={open} onDone={() => setOpen(false)} />
      )}
    </>
  );
}

describe("useModalFocus({ auto: true }) for a hand-drawn dialog", () => {
  it("remembers the opener though the dialog focused its own field first, traps Tab, and gives focus back", async () => {
    await render(<Page kind='mounted' />);
    const opener = named("Review shipment");
    named("Before").focus();
    opener.focus();
    await click(opener);
    expect(document.activeElement).toBe(named("Note"));
    // Tab from the last control wraps: the folded details' link is no stop.
    named("Done").focus();
    await press("Tab");
    expect(document.activeElement).toBe(named("Note"));
    await press("Tab", { shiftKey: true });
    expect(document.activeElement).toBe(named("Done"));
    await click(named("Done"));
    expect($('[role="dialog"]')).toBeNull();
    expect(document.activeElement).toBe(opener);
  });

  it("moves focus in when nothing inside has it, and back as open turns false", async () => {
    await render(<Page kind='shown' />);
    const opener = named("Review shipment");
    opener.focus();
    await click(opener);
    expect(document.activeElement).toBe(named("Mark read"));
    await click(named("Close"));
    expect(document.activeElement).toBe(opener);
  });

  it("without auto, the hook moves nothing by itself (2.30.0 callers unchanged)", async () => {
    const outside = document.createElement("button");
    document.body.append(outside);
    outside.focus();
    await act(async () => {
      await render(<Bare />);
    });
    expect(document.activeElement).toBe(outside);
  });
});
