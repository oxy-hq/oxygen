// @vitest-environment jsdom
//
// PhoneSheet is a full-screen modal: Back and Escape close it, focus goes in
// on open and back to the opener on close, Tab stays inside, the page behind
// cannot scroll, and it portals into the shell scope so the shell's tokens and
// a `.dark` ancestor still reach it.

import { type ReactNode, useState } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { ShellPortalContext, useShellPortalContainer } from "../portal";
import { PhoneButton } from "./PhoneButton";
import { PhoneSheet } from "./PhoneSheet";
import { $, $$, click, press, render } from "./phone.harness";

afterEach(() => {
  document.body.innerHTML = "";
  document.documentElement.style.overflow = "";
});

function Opener({ footer, nested }: { footer?: boolean; nested?: boolean }) {
  const [open, setOpen] = useState(false);
  const [inner, setInner] = useState(false);
  return (
    <>
      <button type='button' data-testid='open' onClick={() => setOpen(true)}>
        Open
      </button>
      <PhoneSheet
        open={open}
        onClose={() => setOpen(false)}
        title='Almaden'
        footer={footer ? <PhoneButton>Post</PhoneButton> : undefined}
      >
        <p>Store detail</p>
        {nested && (
          <button type='button' data-testid='open-inner' onClick={() => setInner(true)}>
            Filters
          </button>
        )}
      </PhoneSheet>
      <PhoneSheet open={inner} onClose={() => setInner(false)} title='Filters'>
        filters
      </PhoneSheet>
    </>
  );
}

const sheets = () => $$('[data-testid="phone-sheet"]');
const opener = () => $('[data-testid="open"]') as HTMLElement;

async function open() {
  opener().focus();
  await click(opener());
}

describe("PhoneSheet", () => {
  it("renders nothing while closed", async () => {
    const page = await render(<Opener />);
    expect(sheets()).toHaveLength(0);
    await page.unmount();
  });

  it("opens as a labelled modal dialog with focus on Back", async () => {
    const page = await render(<Opener />);
    await open();
    const [sheet] = sheets();
    expect(sheet.getAttribute("role")).toBe("dialog");
    expect(sheet.getAttribute("aria-modal")).toBe("true");
    const titleId = sheet.getAttribute("aria-labelledby") as string;
    expect(document.getElementById(titleId)?.textContent).toBe("Almaden");
    expect(document.activeElement).toBe(sheet.querySelector(".oxy-phone-sheet__back"));
    expect(sheet.querySelector(".oxy-phone-sheet__back")?.textContent).toBe("Back");
    await page.unmount();
  });

  it("closes on Back and returns focus to the opener", async () => {
    const page = await render(<Opener />);
    await open();
    await click($(".oxy-phone-sheet__back"));
    expect(sheets()).toHaveLength(0);
    expect(document.activeElement).toBe(opener());
    await page.unmount();
  });

  it("closes on Escape", async () => {
    const page = await render(<Opener />);
    await open();
    await press("Escape");
    expect(sheets()).toHaveLength(0);
    expect(document.activeElement).toBe(opener());
    await page.unmount();
  });

  it("keeps Tab inside", async () => {
    const page = await render(<Opener footer />);
    await open();
    const back = $(".oxy-phone-sheet__back");
    const post = $(".oxy-phone-sheet__footer .oxy-phone-btn");
    await press("Tab", { shiftKey: true });
    expect(document.activeElement).toBe(post);
    await press("Tab");
    expect(document.activeElement).toBe(back);
    await page.unmount();
  });

  it("locks the page behind while open, and restores it", async () => {
    const main = document.createElement("div");
    main.className = "oxy-shell__main";
    main.style.overflow = "auto";
    document.body.appendChild(main);
    const page = await render(<Opener nested />, main);
    await open();
    expect(main.style.overflow).toBe("hidden");
    expect(document.documentElement.style.overflow).toBe("hidden");
    expect(document.body.style.overflow).toBe("hidden");
    // A second sheet on top: one lock, released only when both close.
    await click($('[data-testid="open-inner"]'));
    await press("Escape");
    expect(sheets()).toHaveLength(1);
    expect(main.style.overflow).toBe("hidden");
    await press("Escape");
    expect(sheets()).toHaveLength(0);
    expect(main.style.overflow).toBe("auto");
    expect(document.documentElement.style.overflow).toBe("");
    await page.unmount();
  });

  it("closes only the top sheet on Escape", async () => {
    const page = await render(<Opener nested />);
    await open();
    await click($('[data-testid="open-inner"]'));
    expect(sheets().map((s) => s.querySelector("h2")?.textContent)).toEqual(["Almaden", "Filters"]);
    await press("Escape");
    expect(sheets().map((s) => s.querySelector("h2")?.textContent)).toEqual(["Almaden"]);
    await page.unmount();
  });

  it("portals into the shell scope when there is one", async () => {
    const scope = document.createElement("div");
    scope.className = "oxy-shell-scope oxy-shell dark";
    document.body.appendChild(scope);
    const page = await render(
      <ShellPortalContext.Provider value={scope}>
        <Opener />
      </ShellPortalContext.Provider>
    );
    await open();
    expect(sheets()[0].parentElement).toBe(scope);
    // Its own scope too, for the body fallback: tokens and the type floor.
    expect(sheets()[0].classList.contains("oxy-shell-scope")).toBe(true);
    expect(sheets()[0].classList.contains("oxy-phone")).toBe(true);
    await page.unmount();
  });

  it("names Back from backLabel and pins the footer", async () => {
    const page = await render(
      <PhoneSheet open onClose={() => {}} title='T' backLabel='Stores' footer={<b>go</b>}>
        x
      </PhoneSheet>
    );
    expect($(".oxy-phone-sheet__back")?.textContent).toBe("Stores");
    expect($(".oxy-phone-sheet__footer")?.textContent).toBe("go");
    expect(sheets()[0].classList.contains("oxy-phone-sheet--footer")).toBe(true);
    await page.unmount();
  });

  it("returns focus to the opener when a tap never focused it, as in Safari", async () => {
    const page = await render(<Opener />);
    const opener = $('[data-testid="open"]') as HTMLElement;
    // WebKit does not focus a tapped button: the pointer lands, focus stays on body.
    (document.activeElement as HTMLElement | null)?.blur();
    opener.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
    await click(opener);
    expect(document.activeElement).toBe($(".oxy-phone-sheet__back"));
    await press("Escape");
    expect($('[data-testid="phone-sheet"]')).toBeNull();
    expect(document.activeElement).toBe(opener);
    await page.unmount();
  });

  it("keeps focus on Back when open on the shell's first render", async () => {
    // The shell's portal container only exists after its first commit, so a
    // sheet open from the start moves into it and its first Back is replaced.
    function Shell({ children }: { children: ReactNode }) {
      const { container, setContainer } = useShellPortalContainer();
      return (
        <ShellPortalContext.Provider value={container}>
          <div ref={setContainer} data-testid='shell'>
            {children}
          </div>
        </ShellPortalContext.Provider>
      );
    }
    const page = await render(
      <Shell>
        <PhoneSheet open onClose={() => {}} title='Store'>
          body
        </PhoneSheet>
      </Shell>
    );
    expect($('[data-testid="shell"] [data-testid="phone-sheet"]')).not.toBeNull();
    expect(document.activeElement).toBe($(".oxy-phone-sheet__back"));
    await page.unmount();
  });

  it("returns focus after a tap when the app mounts the sheet only while it is open", async () => {
    function MountedOnOpen() {
      const [open, setOpen] = useState(false);
      return (
        <>
          <button type='button' data-testid='open' onClick={() => setOpen(true)}>
            Open
          </button>
          {open && (
            <PhoneSheet open onClose={() => setOpen(false)} title='Store'>
              body
            </PhoneSheet>
          )}
        </>
      );
    }
    const page = await render(<MountedOnOpen />);
    const opener = $('[data-testid="open"]') as HTMLElement;
    (document.activeElement as HTMLElement | null)?.blur();
    opener.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
    await click(opener);
    await press("Escape");
    expect(document.activeElement).toBe(opener);
    await page.unmount();
  });
});
