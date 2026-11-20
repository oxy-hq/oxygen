// @vitest-environment jsdom
//
// The modal helper SidePanel, Modal and ConfirmDialog share: focus moves in on open and goes back
// to what opened it on close or unmount; Tab and Shift+Tab wrap inside the top-most modal only,
// past hidden and disabled controls; a click on a row's cells names the row's own control as the
// opener; Escape stays with a field someone is typing in.

import { useLayoutEffect, useRef } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { $, fire, press, render } from "../ui.harness";
import { focusableIn, isTopModal, openerFor, trapTab, typingIn, useModalFocus } from "./modalFocus";

afterEach(() => {
  document.body.innerHTML = "";
});

/** A stand-in modal: a box with four controls, two of which Tab must skip. */
function Box({ open, name }: { open: boolean; name: string }) {
  const root = useRef<HTMLDivElement>(null);
  const focus = useModalFocus(root, open);
  useLayoutEffect(() => {
    if (!open) {
      focus.restore();
      return;
    }
    focus.remember();
    focus.enter(root.current?.querySelector<HTMLElement>("button"));
  }, [open, focus]);
  if (!open) return null;
  return (
    <div ref={root} role='dialog' aria-label={name}>
      <button type='button'>{`${name} one`}</button>
      <button type='button' hidden>
        hidden
      </button>
      <button type='button' disabled>
        off
      </button>
      <button type='button'>{`${name} two`}</button>
    </div>
  );
}

function Page({ open, inner = false }: { open: boolean; inner?: boolean }) {
  return (
    <>
      <button type='button' id='opener'>
        Open
      </button>
      <Box open={open} name='outer' />
      <Box open={inner} name='inner' />
    </>
  );
}

const named = (text: string) =>
  [...document.querySelectorAll("button")].find((b) => b.textContent === text) as HTMLElement;
const active = () => (document.activeElement as HTMLElement | null)?.textContent;

describe("useModalFocus", () => {
  it("moves focus in on open and gives it back to the opener on close", async () => {
    const page = await render(<Page open={false} />);
    $("#opener")?.focus();
    await page.rerender(<Page open />);
    expect(active()).toBe("outer one");
    await page.rerender(<Page open={false} />);
    expect(document.activeElement).toBe($("#opener"));
  });

  it("keeps Tab inside: the last wraps to the first and back, past hidden and disabled", async () => {
    await render(<Page open />);
    named("outer two").focus();
    await press("Tab");
    expect(active()).toBe("outer one");
    await press("Tab", { shiftKey: true });
    expect(active()).toBe("outer two");
    // Focus that escaped (a click on the page behind) comes back in on the next Tab.
    $("#opener")?.focus();
    await press("Tab");
    expect(active()).toBe("outer one");
  });

  it("traps only in the top-most modal: a confirm over a panel holds focus", async () => {
    const page = await render(<Page open />);
    await page.rerender(<Page open inner />);
    expect(active()).toBe("inner one");
    expect(isTopModal($('[aria-label="inner"]'))).toBe(true);
    expect(isTopModal($('[aria-label="outer"]'))).toBe(false);
    named("inner two").focus();
    await press("Tab");
    expect(active()).toBe("inner one");
    await page.rerender(<Page open inner={false} />);
    expect(isTopModal($('[aria-label="outer"]'))).toBe(true);
    named("outer two").focus();
    await press("Tab");
    expect(active()).toBe("outer one");
  });

  it("gives focus back when the modal is unmounted while still open", async () => {
    const opener = (
      <button type='button' id='opener'>
        Open
      </button>
    );
    const page = await render(
      <>
        {opener}
        <Box open={false} name='b' />
      </>
    );
    $("#opener")?.focus();
    await page.rerender(
      <>
        {opener}
        <Box open name='b' />
      </>
    );
    expect(active()).toBe("b one");
    await page.rerender(opener);
    expect(document.activeElement).toBe($("#opener"));
  });

  it("leaves focus alone on close when the caller moved it somewhere on purpose", async () => {
    const page = await render(
      <>
        <Page open={false} />
        <p id='refusal' tabIndex={-1}>
          Refused
        </p>
      </>
    );
    $("#opener")?.focus();
    await page.rerender(
      <>
        <Page open />
        <p id='refusal' tabIndex={-1}>
          Refused
        </p>
      </>
    );
    $("#refusal")?.focus();
    await page.rerender(
      <>
        <Page open={false} />
        <p id='refusal' tabIndex={-1}>
          Refused
        </p>
      </>
    );
    expect(document.activeElement).toBe($("#refusal"));
  });
});

describe("trapTab and focusableIn", () => {
  it("count only what Tab can reach", async () => {
    await render(<Page open />);
    const box = $('[aria-label="outer"]') as HTMLElement;
    expect(focusableIn(box).map((b) => b.textContent)).toEqual(["outer one", "outer two"]);
  });

  it("hold focus still in a modal with nothing to Tab to", () => {
    const box = document.createElement("div");
    box.tabIndex = -1;
    document.body.append(box);
    box.focus();
    const tab = new KeyboardEvent("keydown", { key: "Tab", cancelable: true });
    trapTab(tab, box);
    expect(tab.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(box);
  });
});

describe("openerFor", () => {
  it("names the focused control", () => {
    const b = document.createElement("button");
    document.body.append(b);
    b.focus();
    expect(openerFor(document.activeElement)).toBe(b);
  });

  it("names the pressed row's own control after a click on its cells", async () => {
    await render(
      <table>
        <tbody>
          <tr>
            <td id='cell'>Downtown</td>
            <td>
              <button type='button'>Open Downtown</button>
            </td>
          </tr>
        </tbody>
      </table>
    );
    await fire($("#cell"), new Event("pointerdown", { bubbles: true }));
    expect(openerFor(document.body)?.textContent).toBe("Open Downtown");
  });

  it("names the pressed row's control, not the scroller a cell click focused", async () => {
    await render(
      // biome-ignore lint/a11y/noNoninteractiveTabindex: a scroll box is a Tab stop (useScrollFocusable).
      <div id='scroller' tabIndex={0}>
        <table>
          <tbody>
            <tr>
              <td id='cell'>Downtown</td>
              <td>
                <button type='button'>Open Downtown</button>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    );
    $("#scroller")?.focus();
    await fire($("#cell"), new Event("pointerdown", { bubbles: true }));
    expect(openerFor(document.activeElement)?.textContent).toBe("Open Downtown");
  });
});

describe("typingIn", () => {
  it("is true for a field and an editable box, false for a button", () => {
    const make = (tag: string) => document.createElement(tag);
    expect(typingIn(make("input"))).toBe(true);
    expect(typingIn(make("textarea"))).toBe(true);
    expect(typingIn(make("select"))).toBe(true);
    const editable = make("div");
    Object.defineProperty(editable, "isContentEditable", { value: true });
    expect(typingIn(editable)).toBe(true);
    expect(typingIn(make("button"))).toBe(false);
    expect(typingIn(null)).toBe(false);
  });
});
