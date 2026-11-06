// @vitest-environment jsdom
//
// The phone's Back with a sheet open, against jsdom's real session history
// (`go` lands later and fires popstate, as in a browser). `here` is the mark the
// page's own entry carries: if it reads back, the page is where it was.

import * as React from "react";
import { StrictMode, useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PhoneSheet } from "./PhoneSheet";
import { $, $$, click, render as mount } from "./phone.harness";
import { holdHistoryEntry } from "./sheetHistory";

const PAGE = "http://localhost:3000/page";

// A test that fails before its own unmount still takes its sheets with it, so
// one failure does not leave an open sheet in the coordinator for the next test.
const mounted: Array<{ unmount(): Promise<void> }> = [];
async function render(ui: React.ReactNode) {
  const page = await mount(ui);
  mounted.push(page);
  return page;
}

const titles = () => $$('[data-testid="phone-sheet"] h2').map((h) => h.textContent);
const here = () => (window.history.state as { page?: string } | null)?.page;

async function settle() {
  await React.act(async () => {
    await new Promise((r) => setTimeout(r, 30));
  });
}

async function act(fn: () => void) {
  await React.act(async () => {
    fn();
    await new Promise((r) => setTimeout(r, 30));
  });
}

async function phoneBack() {
  await React.act(async () => {
    window.history.back();
    await new Promise((r) => setTimeout(r, 30));
  });
}

async function phoneForward() {
  await React.act(async () => {
    window.history.forward();
    await new Promise((r) => setTimeout(r, 30));
  });
}

const tap = async (testId: string) => {
  await click($(`[data-testid="${testId}"]`));
  await settle();
};
const tapBack = async () => {
  const backs = $$(".oxy-phone-sheet__back");
  await click(backs[backs.length - 1]);
  await settle();
};

function One({
  onClose,
  historyEntry,
  body
}: {
  onClose?: () => void;
  historyEntry?: boolean;
  body?: (close: () => void) => React.ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const close = () => {
    onClose?.();
    setOpen(false);
  };
  return (
    <>
      <button type='button' data-testid='open' onClick={() => setOpen(true)}>
        Open
      </button>
      <PhoneSheet open={open} onClose={close} title='One' historyEntry={historyEntry}>
        {body?.(close)}
      </PhoneSheet>
    </>
  );
}

function Nested() {
  const [outer, setOuter] = useState(false);
  const [inner, setInner] = useState(false);
  return (
    <>
      <button type='button' data-testid='open' onClick={() => setOuter(true)}>
        Open
      </button>
      <PhoneSheet open={outer} onClose={() => setOuter(false)} title='Outer'>
        <button type='button' data-testid='open-inner' onClick={() => setInner(true)}>
          Inner
        </button>
      </PhoneSheet>
      <PhoneSheet open={inner} onClose={() => setInner(false)} title='Inner'>
        inner
      </PhoneSheet>
    </>
  );
}

// Bookkeeping's ImportPhone: the list hides while an import is open over it, and
// comes back when the import closes.
function ListAndDetail() {
  const [list, setList] = useState(false);
  const [detail, setDetail] = useState(false);
  return (
    <>
      <button type='button' data-testid='open' onClick={() => setList(true)}>
        Open
      </button>
      <PhoneSheet open={list && !detail} onClose={() => setList(false)} title='List'>
        <button type='button' data-testid='row' onClick={() => setDetail(true)}>
          Row
        </button>
      </PhoneSheet>
      <PhoneSheet open={detail} onClose={() => setDetail(false)} title='Detail'>
        detail
      </PhoneSheet>
    </>
  );
}

beforeEach(() => {
  window.history.pushState({ page: "base" }, "", PAGE);
});

afterEach(async () => {
  for (const page of mounted.splice(0)) await page.unmount();
  await settle();
  document.body.innerHTML = "";
  document.documentElement.style.overflow = "";
});

describe("the phone's Back with a sheet open", () => {
  it("closes the sheet and leaves the page and its URL where they were", async () => {
    const onClose = vi.fn();
    const page = await render(<One onClose={onClose} />);
    await tap("open");
    expect(titles()).toEqual(["One"]);
    expect(here()).toBe("base"); // the sheet's entry keeps the state it found
    expect(window.history.state.oxyPhoneSheetDepth).toBe(1);
    await phoneBack();
    expect(titles()).toEqual([]);
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(window.location.href).toBe(PAGE);
    expect(window.history.state).toEqual({ page: "base" });
    await page.unmount();
  });

  it("closes nested sheets top first, one Back each", async () => {
    const page = await render(<Nested />);
    await tap("open");
    await tap("open-inner");
    expect(titles()).toEqual(["Outer", "Inner"]);
    await phoneBack();
    expect(titles()).toEqual(["Outer"]);
    await phoneBack();
    expect(titles()).toEqual([]);
    expect(window.history.state).toEqual({ page: "base" });
    await page.unmount();
  });

  it("does not bring a sheet back on Forward, and the next Back still closes the one below", async () => {
    const page = await render(<Nested />);
    await tap("open");
    await tap("open-inner");
    await phoneBack();
    await phoneForward();
    await settle();
    expect(titles()).toEqual(["Outer"]);
    await phoneBack();
    expect(titles()).toEqual([]);
    expect(window.history.state).toEqual({ page: "base" });
    await page.unmount();
  });

  it("keeps asking a sheet that does not close, as its own Back would", async () => {
    // A sheet with unsaved work: onClose asks first, and the sheet stays.
    const onClose = vi.fn();
    function Guarded() {
      const [open, setOpen] = useState(true);
      return (
        <PhoneSheet open={open} onClose={onClose} title='Guarded'>
          <button type='button' data-testid='discard' onClick={() => setOpen(false)}>
            Discard
          </button>
        </PhoneSheet>
      );
    }
    const page = await render(<Guarded />);
    await settle();
    await phoneBack();
    await settle();
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(titles()).toEqual(["Guarded"]);
    expect(window.history.state.oxyPhoneSheetDepth).toBe(1); // its entry is back
    await phoneBack();
    await settle();
    expect(onClose).toHaveBeenCalledTimes(2);
    expect(titles()).toEqual(["Guarded"]);
    await tap("discard");
    expect(titles()).toEqual([]);
    expect(window.history.state).toEqual({ page: "base" });
    expect(window.location.href).toBe(PAGE);
    await page.unmount();
  });
});

describe("closing a sheet any other way", () => {
  it("takes its entry away, so the page's own entry is current again", async () => {
    const page = await render(<One />);
    const before = window.history.length;
    await tap("open");
    expect(window.history.length).toBe(before + 1);
    await tapBack();
    expect(window.history.state).toEqual({ page: "base" });
    expect(window.location.href).toBe(PAGE);
    await page.unmount();
  });

  it("does not grow the history however many times a sheet opens", async () => {
    const page = await render(<One />);
    await tap("open");
    await tapBack();
    const once = window.history.length;
    for (let i = 0; i < 3; i += 1) {
      await tap("open");
      await tapBack();
    }
    expect(window.history.length).toBe(once);
    expect(window.history.state).toEqual({ page: "base" });
    await page.unmount();
  });

  it("takes its entry away when the sheet unmounts while open", async () => {
    function Host() {
      const [mounted, setMounted] = useState(true);
      return (
        <>
          <button type='button' data-testid='drop' onClick={() => setMounted(false)}>
            Drop
          </button>
          {mounted && (
            <PhoneSheet open onClose={() => {}} title='Mounted'>
              x
            </PhoneSheet>
          )}
        </>
      );
    }
    const page = await render(<Host />);
    await settle();
    expect(window.history.state.oxyPhoneSheetDepth).toBe(1);
    await tap("drop");
    expect(titles()).toEqual([]);
    expect(window.history.state).toEqual({ page: "base" });
    await page.unmount();
  });
});

describe("one commit, more than one sheet", () => {
  it("moves nothing when one sheet closes as another opens on the same URL", async () => {
    const page = await render(<ListAndDetail />);
    await tap("open");
    const length = window.history.length;
    const go = vi.spyOn(window.history, "go");
    const pushed = vi.spyOn(window.history, "pushState");
    await tap("row");
    expect(titles()).toEqual(["Detail"]);
    expect(go).not.toHaveBeenCalled();
    expect(pushed).not.toHaveBeenCalled();
    expect(window.history.length).toBe(length);
    go.mockRestore();
    pushed.mockRestore();
    await page.unmount();
  });

  it("when a swap moves the URL in place, Back lands on the new URL with the page's own state", async () => {
    // Store Ops' "View submission": the task sheet closes, the submission's opens,
    // and the module in the URL changes, all at once.
    function Swap() {
      const [which, setWhich] = useState<"task" | "submission" | null>("task");
      return (
        <>
          <PhoneSheet open={which === "task"} onClose={() => {}} title='Task'>
            <button
              type='button'
              data-testid='swap'
              onClick={() => {
                window.history.replaceState(window.history.state, "", `${PAGE}?module=forms`);
                setWhich("submission");
              }}
            >
              View
            </button>
          </PhoneSheet>
          <PhoneSheet
            open={which === "submission"}
            onClose={() => setWhich(null)}
            title='Submission'
          >
            submission
          </PhoneSheet>
        </>
      );
    }
    const page = await render(<Swap />);
    await settle();
    await tap("swap");
    expect(titles()).toEqual(["Submission"]);
    expect(window.history.state.oxyPhoneSheetDepth).toBe(1);
    expect(window.location.href).toBe(`${PAGE}?module=forms`);
    await phoneBack();
    expect(titles()).toEqual([]);
    expect(window.location.href).toBe(`${PAGE}?module=forms`);
    expect(window.history.state).toEqual({ page: "base" });
    await page.unmount();
  });

  it("when a swap pushes a URL, the new sheet gets an entry on top of it", async () => {
    function Swap() {
      const [which, setWhich] = useState<"first" | "second" | null>("first");
      return (
        <>
          <PhoneSheet open={which === "first"} onClose={() => {}} title='First'>
            <button
              type='button'
              data-testid='swap'
              onClick={() => {
                window.history.pushState(null, "", "/elsewhere");
                setWhich("second");
              }}
            >
              Next
            </button>
          </PhoneSheet>
          <PhoneSheet open={which === "second"} onClose={() => setWhich(null)} title='Second'>
            second
          </PhoneSheet>
        </>
      );
    }
    const page = await render(<Swap />);
    await settle();
    await tap("swap");
    expect(titles()).toEqual(["Second"]);
    expect(window.history.state.oxyPhoneSheetDepth).toBe(1);
    await phoneBack();
    expect(titles()).toEqual([]);
    expect(window.location.pathname).toBe("/elsewhere");
    await page.unmount();
  });

  it("gives a sheet the app reopens as its child closes an entry of its own", async () => {
    const page = await render(<ListAndDetail />);
    await tap("open");
    await tap("row");
    await phoneBack();
    await settle();
    expect(titles()).toEqual(["List"]); // what the detail's own Back shows
    expect(window.history.state.oxyPhoneSheetDepth).toBe(1);
    await phoneBack();
    await settle();
    expect(titles()).toEqual([]);
    expect(window.history.state).toEqual({ page: "base" });
    await page.unmount();
  });

  it("keeps a sheet that opens while the entry of the last one is being taken away", async () => {
    // At the coordinator, because React's act would hold the opening commit back
    // until the move has landed, and this is about the moment between the two.
    const closeSecond = vi.fn();
    const releaseFirst = holdHistoryEntry(() => {});
    await settle();
    const realGo = window.history.go.bind(window.history);
    let releaseSecond = () => {};
    const go = vi.spyOn(window.history, "go").mockImplementation((delta) => {
      realGo(delta);
      releaseSecond = holdHistoryEntry(closeSecond);
    });
    releaseFirst();
    await settle();
    expect(go).toHaveBeenCalledTimes(1);
    go.mockRestore();
    expect(closeSecond).not.toHaveBeenCalled();
    expect(window.history.state.oxyPhoneSheetDepth).toBe(1);
    await phoneBack();
    expect(closeSecond).toHaveBeenCalledTimes(1);
    releaseSecond();
    await settle();
    expect(window.history.state).toEqual({ page: "base" });
  });

  it("pushes once under StrictMode's mount, unmount, mount", async () => {
    const pushed = vi.spyOn(window.history, "pushState");
    const go = vi.spyOn(window.history, "go");
    const page = await render(
      <StrictMode>
        <One />
      </StrictMode>
    );
    await tap("open");
    expect(pushed).toHaveBeenCalledTimes(1);
    expect(go).not.toHaveBeenCalled();
    pushed.mockRestore();
    go.mockRestore();
    await phoneBack();
    expect(titles()).toEqual([]);
    expect(window.history.state).toEqual({ page: "base" });
    await page.unmount();
  });
});

describe("when the URL changed while the sheet was open", () => {
  it.each([
    ["wiping the entry's state", () => null],
    ["keeping the entry's state", () => window.history.state]
  ])("a filter replacing the URL, %s, stays when the sheet closes", async (_, state) => {
    const page = await render(
      <One
        body={() => (
          <button
            type='button'
            data-testid='filter'
            onClick={() => window.history.replaceState(state(), "", `${PAGE}?status=late`)}
          >
            Late
          </button>
        )}
      />
    );
    await tap("open");
    await tap("filter");
    await tapBack();
    expect(window.location.href).toBe(`${PAGE}?status=late`);
    // On the page's own entry, not one above it: Back has nothing of ours to undo.
    expect(window.history.state).toEqual({ page: "base" });
    await page.unmount();
  });

  it("a router reading the URL on popstate sees the carried one, never the old", async () => {
    const seen: string[] = [];
    const read = () => seen.push(window.location.search);
    window.addEventListener("popstate", read);
    const page = await render(
      <One
        body={() => (
          <button
            type='button'
            data-testid='filter'
            onClick={() => window.history.replaceState(null, "", `${PAGE}?status=late`)}
          >
            Late
          </button>
        )}
      />
    );
    await tap("open");
    await tap("filter");
    await tapBack();
    expect(seen).toEqual(["?status=late"]);
    window.removeEventListener("popstate", read);
    await page.unmount();
  });

  it("the phone's Back still closes the sheet, on the URL below", async () => {
    const page = await render(
      <One
        body={() => (
          <button
            type='button'
            data-testid='filter'
            onClick={() => window.history.replaceState(null, "", `${PAGE}?status=late`)}
          >
            Late
          </button>
        )}
      />
    );
    await tap("open");
    await tap("filter");
    await phoneBack();
    expect(titles()).toEqual([]);
    expect(window.location.href).toBe(PAGE);
    expect(window.history.state).toEqual({ page: "base" });
    await page.unmount();
  });

  it("a sheet the URL names by replacing does not reopen after it closes", async () => {
    // Store Ops and Bookkeeping: `?task=3` written with replaceState, so the page's
    // own entry says it too, and a sheet that reads the URL would open again there.
    function UrlNamed() {
      const [, rerender] = useState(0);
      const task = new URLSearchParams(window.location.search).get("task");
      React.useEffect(() => {
        const read = () => rerender((n) => n + 1);
        window.addEventListener("popstate", read);
        return () => window.removeEventListener("popstate", read);
      }, []);
      const write = (v: string | null) => {
        window.history.replaceState(window.history.state, "", v ? `${PAGE}?task=${v}` : PAGE);
        rerender((n) => n + 1);
      };
      return (
        <>
          <button type='button' data-testid='open' onClick={() => write("3")}>
            Open
          </button>
          <PhoneSheet open={task !== null} onClose={() => write(null)} title='Task'>
            task
          </PhoneSheet>
        </>
      );
    }
    const page = await render(<UrlNamed />);
    await tap("open");
    await tapBack();
    await settle();
    expect(titles()).toEqual([]);
    expect(window.location.href).toBe(PAGE);
    expect(window.history.state).toEqual({ page: "base" });

    await tap("open");
    await phoneBack();
    await settle();
    expect(titles()).toEqual([]);
    expect(window.location.href).toBe(PAGE);
    expect(window.history.state).toEqual({ page: "base" });
    await page.unmount();
  });

  it("a navigation from inside the sheet is not undone, and a later sheet still closes on Back", async () => {
    const page = await render(
      <One
        body={(close) => (
          <button
            type='button'
            data-testid='go'
            onClick={() => {
              window.history.pushState(null, "", "/elsewhere");
              close();
            }}
          >
            Go
          </button>
        )}
      />
    );
    await tap("open");
    await tap("go");
    expect(titles()).toEqual([]);
    expect(window.location.pathname).toBe("/elsewhere");
    expect(window.history.state).toBeNull(); // on the entry the app pushed, not moved off it

    // The page there opens a sheet: its Back lands on the page, not on the old mark.
    await tap("open");
    expect(titles()).toEqual(["One"]);
    await phoneBack();
    expect(titles()).toEqual([]);
    expect(window.location.pathname).toBe("/elsewhere");

    // One Back goes all the way to the page: the entry left behind steps down off
    // itself rather than being a Back that does nothing. No sheet opens.
    await phoneBack();
    await settle();
    expect(titles()).toEqual([]);
    expect(window.location.href).toBe(PAGE);
    expect(window.history.state).toEqual({ page: "base" });
    await page.unmount();
  });

  it("a sheet still open when the one above navigates gets a new entry", async () => {
    function Host() {
      const [outer, setOuter] = useState(true);
      const [inner, setInner] = useState(true);
      return (
        <>
          <PhoneSheet open={outer} onClose={() => setOuter(false)} title='Outer'>
            outer
          </PhoneSheet>
          <PhoneSheet open={inner} onClose={() => setInner(false)} title='Inner'>
            <button
              type='button'
              data-testid='go'
              onClick={() => {
                window.history.pushState(null, "", "/elsewhere");
                setInner(false);
              }}
            >
              Go
            </button>
          </PhoneSheet>
        </>
      );
    }
    const page = await render(<Host />);
    await settle();
    await tap("go");
    expect(titles()).toEqual(["Outer"]);
    expect(window.location.pathname).toBe("/elsewhere");
    await phoneBack();
    expect(titles()).toEqual([]);
    expect(window.location.pathname).toBe("/elsewhere");
    await page.unmount();
  });

  it("nested: an app wiping the mark under a sheet does not close two on one Back", async () => {
    function Host() {
      const [outer, setOuter] = useState(true);
      const [inner, setInner] = useState(false);
      return (
        <>
          <PhoneSheet open={outer} onClose={() => setOuter(false)} title='Outer'>
            <button
              type='button'
              data-testid='next'
              onClick={() => {
                window.history.replaceState(null, "", window.location.href);
                setInner(true);
              }}
            >
              Next
            </button>
          </PhoneSheet>
          <PhoneSheet open={inner} onClose={() => setInner(false)} title='Inner'>
            inner
          </PhoneSheet>
        </>
      );
    }
    const page = await render(<Host />);
    await settle();
    await tap("next");
    await phoneBack();
    expect(titles()).toEqual(["Outer"]);
    await phoneBack();
    expect(titles()).toEqual([]);
    await page.unmount();
  });
});

describe("a hash navigation from inside a sheet", () => {
  const link = (href: string) => (
    <a data-testid='link' href={href}>
      Jump to the section
    </a>
  );

  it("keeps the sheet open, and Back then closes it without leaving the page", async () => {
    const page = await render(<One body={() => link("#section")} />);
    await tap("open");
    await tap("link");
    expect(titles()).toEqual(["One"]);
    expect(window.location.hash).toBe("#section");

    await phoneBack();
    expect(titles()).toEqual([]);
    expect(window.location.href).toBe(`${PAGE}#section`);
    await settle();
    await page.unmount();
  });

  it("does not count as a Back when set from code, and leaves the entries it passed behind one Back", async () => {
    const page = await render(
      <One
        body={() => (
          <button
            type='button'
            data-testid='set'
            onClick={() => {
              window.location.hash = "section";
            }}
          >
            Set
          </button>
        )}
      />
    );
    await tap("open");
    await tap("set");
    expect(titles()).toEqual(["One"]);

    await phoneBack();
    expect(titles()).toEqual([]);
    expect(window.location.hash).toBe("#section");
    // Off the page's old sheet entry, which holds nothing now, down to the page.
    await phoneBack();
    await settle();
    expect(window.location.href).toBe(PAGE);
    expect(here()).toBe("base");
    await page.unmount();
  });

  it("keeps the sheet after a filter moved the URL in place, so only the length tells", async () => {
    const page = await render(
      <One
        body={() => (
          <>
            <button
              type='button'
              data-testid='filter'
              onClick={() => window.history.replaceState(window.history.state, "", `${PAGE}?f=1`)}
            >
              Filter
            </button>
            {link("#section")}
          </>
        )}
      />
    );
    await tap("open");
    await tap("filter");
    await tap("link");
    expect(titles()).toEqual(["One"]);
    expect(window.location.search).toBe("?f=1");
    await page.unmount();
  });

  it("keeps both of two nested sheets, which then close one Back each", async () => {
    function Host() {
      const [outer, setOuter] = useState(true);
      const [inner, setInner] = useState(true);
      return (
        <>
          <PhoneSheet open={outer} onClose={() => setOuter(false)} title='Outer'>
            outer
          </PhoneSheet>
          <PhoneSheet open={inner} onClose={() => setInner(false)} title='Inner'>
            {link("#section")}
          </PhoneSheet>
        </>
      );
    }
    const page = await render(<Host />);
    await settle();
    await tap("link");
    expect(titles()).toEqual(["Outer", "Inner"]);
    await phoneBack();
    expect(titles()).toEqual(["Outer"]);
    await phoneBack();
    expect(titles()).toEqual([]);
    expect(window.location.hash).toBe("#section");
    await page.unmount();
  });

  it("keeps a sheet when its forward entry was overwritten, so the history did not grow", async () => {
    // Back closed the inner sheet and left its entry ahead of us: the jump takes
    // that slot, and `history.length` reads the same as before.
    const page = await render(<Nested />);
    await tap("open");
    await tap("open-inner");
    await phoneBack();
    expect(titles()).toEqual(["Outer"]);
    await act(() => {
      window.location.hash = "section";
    });
    expect(titles()).toEqual(["Outer"]);
    await phoneBack();
    expect(titles()).toEqual([]);
    expect(window.location.hash).toBe("#section");
    await page.unmount();
  });

  it("is not how a Back to an entry an app pushed under the sheet reads: that still closes it", async () => {
    // Two pushes with the sheet left open, so Back lands on one with no mark, on
    // another path. Not a jump: the sheet goes, as it would on any other Back.
    const page = await render(<One />);
    await tap("open");
    window.history.pushState(null, "", "/one");
    window.history.pushState(null, "", "/two");
    await phoneBack();
    expect(window.location.pathname).toBe("/one");
    expect(titles()).toEqual([]);
    await page.unmount();
  });

  it("is not how a Back to a page entry with a hash of its own reads", async () => {
    window.history.pushState(null, "", `${PAGE}#top`);
    const page = await render(<One />);
    await tap("open");
    await phoneBack();
    expect(titles()).toEqual([]);
    expect(window.location.href).toBe(`${PAGE}#top`);
    await page.unmount();
  });

  it("takes a popstate that names no state over an entry that kept its own as no move at all", async () => {
    // A tap on a link to the URL already showing: browsers fire popstate with a
    // null state and do not touch the entry (measured in Chromium and WebKit).
    // jsdom pushes there, so the event is sent by hand.
    const page = await render(<One />);
    await tap("open");
    const go = vi.spyOn(window.history, "go");
    await act(() => {
      window.dispatchEvent(new PopStateEvent("popstate", { state: null }));
    });
    expect(titles()).toEqual(["One"]);
    expect(go).not.toHaveBeenCalled();
    go.mockRestore();
    await phoneBack();
    expect(titles()).toEqual([]);
    expect(here()).toBe("base");
    await page.unmount();
  });
});

describe("a Back that closes the top of two nested sheets", () => {
  it("is no swap: one history move, and the URL replaced under the top one stays off the page's entry", async () => {
    function Host() {
      const [outer, setOuter] = useState(false);
      const [inner, setInner] = useState(false);
      return (
        <>
          <button type='button' data-testid='open' onClick={() => setOuter(true)}>
            Open
          </button>
          <PhoneSheet open={outer} onClose={() => setOuter(false)} title='Outer'>
            <button
              type='button'
              data-testid='filter'
              onClick={() => window.history.replaceState(window.history.state, "", `${PAGE}?f=1`)}
            >
              Filter
            </button>
            <button type='button' data-testid='open-inner' onClick={() => setInner(true)}>
              Inner
            </button>
          </PhoneSheet>
          <PhoneSheet open={inner} onClose={() => setInner(false)} title='Inner'>
            inner
          </PhoneSheet>
        </>
      );
    }
    const page = await render(<Host />);
    await tap("open");
    await tap("filter");
    await tap("open-inner");

    const go = vi.spyOn(window.history, "go");
    const pushed = vi.spyOn(window.history, "pushState");
    const popped = vi.fn();
    window.addEventListener("popstate", popped);
    await phoneBack();
    await settle();
    window.removeEventListener("popstate", popped);
    expect(titles()).toEqual(["Outer"]);
    expect(window.location.search).toBe("?f=1");
    expect(go).not.toHaveBeenCalled();
    expect(pushed).not.toHaveBeenCalled();
    expect(popped).toHaveBeenCalledTimes(1);
    go.mockRestore();
    pushed.mockRestore();

    // The page's entry never heard of the filter.
    await phoneBack();
    expect(titles()).toEqual([]);
    expect(window.location.href).toBe(PAGE);
    expect(here()).toBe("base");
    await page.unmount();
  });
});

describe("historyEntry={false}", () => {
  // Opens by pushing its own entry, as Supply Chain's `?item=` does.
  function Pushed({ onClose }: { onClose: () => void }) {
    const [, rerender] = useState(0);
    React.useEffect(() => {
      const read = () => rerender((n) => n + 1);
      window.addEventListener("popstate", read);
      return () => window.removeEventListener("popstate", read);
    }, []);
    const item = new URLSearchParams(window.location.search).get("item");
    return (
      <>
        <button
          type='button'
          data-testid='open'
          onClick={() => {
            window.history.pushState(null, "", `${PAGE}?item=1`);
            rerender((n) => n + 1);
          }}
        >
          Open
        </button>
        <PhoneSheet open={item !== null} onClose={onClose} title='Item' historyEntry={false}>
          item
        </PhoneSheet>
      </>
    );
  }

  it("adds no entry and marks none, and Back's own move closes the sheet", async () => {
    const onClose = vi.fn();
    const page = await render(<Pushed onClose={onClose} />);
    const pushed = vi.spyOn(window.history, "pushState");
    const go = vi.spyOn(window.history, "go");
    await tap("open");
    expect(titles()).toEqual(["Item"]);
    expect(pushed).toHaveBeenCalledTimes(1); // the app's, none of ours
    expect(window.history.state).toBeNull();
    await phoneBack();
    expect(titles()).toEqual([]);
    expect(onClose).not.toHaveBeenCalled();
    expect(go).not.toHaveBeenCalled();
    expect(window.location.href).toBe(PAGE);
    pushed.mockRestore();
    go.mockRestore();
    await page.unmount();
  });

  it("a held sheet above a URL-named one closes alone, then Back reaches the URL's entry", async () => {
    function Over() {
      const [, rerender] = useState(0);
      const [held, setHeld] = useState(false);
      React.useEffect(() => {
        const read = () => rerender((n) => n + 1);
        window.addEventListener("popstate", read);
        return () => window.removeEventListener("popstate", read);
      }, []);
      const item = new URLSearchParams(window.location.search).get("item");
      return (
        <>
          <button
            type='button'
            data-testid='open'
            onClick={() => {
              window.history.pushState(null, "", `${PAGE}?item=1`);
              rerender((n) => n + 1);
            }}
          >
            Open
          </button>
          <PhoneSheet open={item !== null} onClose={() => {}} title='Item' historyEntry={false}>
            <button type='button' data-testid='held' onClick={() => setHeld(true)}>
              Held
            </button>
          </PhoneSheet>
          <PhoneSheet open={held} onClose={() => setHeld(false)} title='Held'>
            held
          </PhoneSheet>
        </>
      );
    }
    const page = await render(<Over />);
    await tap("open");
    await tap("held");
    expect(titles()).toEqual(["Item", "Held"]);
    await phoneBack();
    expect(titles()).toEqual(["Item"]);
    expect(window.location.search).toBe("?item=1");
    await phoneBack();
    expect(titles()).toEqual([]);
    expect(window.location.href).toBe(PAGE);
    await page.unmount();
  });
});

describe("the popstate listener", () => {
  it("is added once, at load, ahead of any a router adds in an effect", async () => {
    // Chromium runs a window's listeners in the order they were added, whatever
    // the phase (jsdom does not, so the order itself is checked in a browser).
    vi.resetModules();
    const added = vi.spyOn(window, "addEventListener");
    const popstates = () => added.mock.calls.filter(([type]) => type === "popstate").length;
    const fresh = await import("./sheetHistory");
    expect(popstates()).toBe(1);
    const release = fresh.holdHistoryEntry(() => {});
    await settle();
    release();
    await settle();
    expect(popstates()).toBe(1);
    added.mockRestore();
  });

  it("leaves a Back between the app's own entries alone", async () => {
    window.history.pushState({ page: "above" }, "", `${PAGE}?above=1`);
    const replaced = vi.spyOn(window.history, "replaceState");
    const pushed = vi.spyOn(window.history, "pushState");
    await phoneBack();
    await settle();
    expect(replaced).not.toHaveBeenCalled();
    expect(pushed).not.toHaveBeenCalled();
    expect(window.history.state).toEqual({ page: "base" });
    replaced.mockRestore();
    pushed.mockRestore();
  });
});
