// @vitest-environment jsdom
//
// Back's focus ring as the sheet opens. With the theme, Back takes focus with its
// ring only when the sheet was opened from the keyboard: the control that opened
// it held focus the browser showed (`:focus-visible`). A tap, a click, or a sheet
// open on arrival rings nothing; Chrome's own guess for a script's focus() rang
// Back in those (Locations' site sheet, opened by its link). Without the theme,
// Back is focused as 2.28 focused it, its ring left to the browser.
//
// jsdom judges `:focus-visible` by the last input as a browser does (a key shows
// focus, a mouse event hides it), so the opener's state is real here; what the
// sheet asks of the browser is read off its focus() call. The browser's answer was
// measured in Chrome for the PR (Locations, by link, by a tap and by Tab + Enter).

import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PhoneSheet } from "./PhoneSheet";
import { $, click, press, render } from "./phone.harness";

function Opener() {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button
        type='button'
        data-testid='open'
        onClick={() => setOpen(true)}
        // jsdom's Enter fires no click; a browser's would, keeping the key's judgement.
        onKeyDown={(e) => e.key === "Enter" && setOpen(true)}
      >
        Open
      </button>
      <PhoneSheet open={open} onClose={() => setOpen(false)} title='Almaden'>
        <p>Store detail</p>
      </PhoneSheet>
    </>
  );
}

const root = document.documentElement;
const theme = (on: boolean) => {
  if (on) root.style.setProperty("--oxy-theme-contract", "1");
  else root.style.removeProperty("--oxy-theme-contract");
};

/** The options each focus() on a Back was called with, in order. */
function watchBackFocus() {
  const calls: (FocusOptions | undefined)[] = [];
  const real = HTMLElement.prototype.focus;
  const spy = vi.spyOn(HTMLElement.prototype, "focus").mockImplementation(function (
    this: HTMLElement,
    options?: FocusOptions
  ) {
    if (this.classList.contains("oxy-phone-sheet__back")) calls.push(options);
    real.call(this, options);
  });
  return { calls, restore: () => spy.mockRestore() };
}

let watch: ReturnType<typeof watchBackFocus>;
beforeEach(() => {
  watch = watchBackFocus();
});
afterEach(() => {
  watch.restore();
  theme(false);
  document.body.innerHTML = "";
  document.documentElement.style.overflow = "";
});

const opener = () => $('[data-testid="open"]') as HTMLElement;
const back = () => $(".oxy-phone-sheet__back");

describe("PhoneSheet Back's focus ring", () => {
  it("with the theme, rings Back for a sheet opened from the keyboard", async () => {
    theme(true);
    const page = await render(<Opener />);
    opener().focus();
    await press("Enter");
    expect(document.activeElement).toBe(back());
    expect(watch.calls).toEqual([{ focusVisible: true }]);
    await page.unmount();
  });

  it("with the theme, focuses Back without its ring for a sheet opened by a tap or a click", async () => {
    theme(true);
    const page = await render(<Opener />);
    // Chrome focuses the tapped button without showing it; Safari focuses nothing.
    opener().focus();
    await click(opener());
    expect(document.activeElement).toBe(back());
    expect(watch.calls).toEqual([{ focusVisible: false }]);
    await page.unmount();
  });

  it("with the theme, focuses Back without its ring for a sheet open on arrival", async () => {
    theme(true);
    const page = await render(
      <PhoneSheet open onClose={() => {}} title='Almaden'>
        detail
      </PhoneSheet>
    );
    expect(document.activeElement).toBe(back());
    expect(watch.calls).toEqual([{ focusVisible: false }]);
    await page.unmount();
  });

  it("without the theme, focuses Back as 2.28 did, leaving the ring to the browser", async () => {
    const page = await render(<Opener />);
    await click(opener());
    expect(document.activeElement).toBe(back());
    expect(watch.calls).toEqual([undefined]);
    await page.unmount();
  });
});
