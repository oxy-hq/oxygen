// @vitest-environment jsdom
//
// The phone's Back after switching sections in the tab bar, against jsdom's real
// session history (`go` lands later and fires popstate, as in a browser). `here`
// is where the page stands: on its own entry ("base", no mark of ours) the next Back
// leaves the app, and on the bar's entry ("tab") it does not.
//
// With `historyEntry` on, Back from any section but the first returns to the first,
// and from the first it leaves, as in a native app's bottom navigation: one entry
// while away from the first section, not one per tap. It is off by default: an app
// whose router pushes an entry per section already has Back between sections, and
// the two together would loop.

import * as React from "react";
import { StrictMode, useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PhoneTabBar, type PhoneTabItem } from "./PhoneTabBar";
import { $$, click, render as mount } from "./phone.harness";

const PAGE = "http://localhost:3000/page";

const mounted: Array<{ unmount(): Promise<void> }> = [];
async function render(ui: React.ReactNode) {
  const page = await mount(ui);
  mounted.push(page);
  return page;
}

const here = () => {
  const state = window.history.state as { page?: string; oxyPhoneSheetDepth?: number } | null;
  if (state?.oxyPhoneSheetDepth) return "tab";
  return state?.page;
};
const current = () =>
  $$(".oxy-phone-tabbar__slot").find((s) => s.getAttribute("aria-current") === "page")?.textContent;
const sheetTitles = () => $$('[data-testid="phone-sheet"] h2').map((h) => h.textContent);

async function settle() {
  await React.act(async () => {
    await new Promise((r) => setTimeout(r, 30));
  });
}

async function phoneBack() {
  await React.act(async () => {
    window.history.back();
    await new Promise((r) => setTimeout(r, 30));
  });
}

async function tab(label: string) {
  const slot = $$(".oxy-phone-tabbar__slot").find(
    (s) => s.querySelector(".oxy-phone-tabbar__label")?.textContent === label
  );
  await click(slot ?? null);
  await settle();
}

async function more(label: string) {
  const row = $$(".oxy-phone-more__row").find(
    (r) => r.querySelector(".oxy-phone-more__label")?.textContent === label
  );
  await click(row ?? null);
  await settle();
}

const FOUR: PhoneTabItem[] = [
  { id: "sales", label: "Sales" },
  { id: "stores", label: "Stores" },
  { id: "post", label: "Post" },
  { id: "credit", label: "Credit" }
];
const SEVEN: PhoneTabItem[] = [
  ...FOUR,
  { id: "grubhub", label: "GrubHub" },
  { id: "doordash", label: "DoorDash" },
  { id: "settings", label: "Settings" }
];

function Bar({
  items = FOUR,
  start,
  historyEntry
}: {
  items?: PhoneTabItem[];
  start?: string;
  historyEntry?: boolean;
}) {
  const [active, setActive] = useState(start ?? items[0].id);
  return (
    <PhoneTabBar items={items} activeId={active} onSelect={setActive} historyEntry={historyEntry} />
  );
}

// An app whose router pushes `?tab=` per section and reads it back on popstate
// (Command Center's shape), using the bar's default.
const tabInUrl = () => new URLSearchParams(window.location.search).get("tab") ?? FOUR[0].id;

function RouterBar() {
  const [active, setActive] = useState(tabInUrl);
  React.useEffect(() => {
    const onPop = () => setActive(tabInUrl());
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, []);
  return (
    <PhoneTabBar
      items={FOUR}
      activeId={active}
      onSelect={(id) => {
        window.history.pushState({ page: `tab-${id}` }, "", `${PAGE}?tab=${id}`);
        setActive(id);
      }}
    />
  );
}

let phone = true;
beforeEach(() => {
  phone = true;
  vi.stubGlobal("matchMedia", (media: string) => ({
    matches: phone,
    media,
    addEventListener() {},
    removeEventListener() {}
  }));
  window.history.pushState({ page: "base" }, "", PAGE);
});

afterEach(async () => {
  for (const page of mounted.splice(0)) await page.unmount();
  await settle();
  vi.unstubAllGlobals();
  document.body.innerHTML = "";
});

describe("the phone's Back after a tab tap", () => {
  it("returns to the first section, and the page's own entry is under it", async () => {
    await render(<Bar historyEntry />);
    await tab("Stores");
    expect(current()).toBe("Stores");
    expect(here()).toBe("tab");

    await phoneBack();
    expect(current()).toBe("Sales");
    expect(here()).toBe("base");
  });

  it("holds one entry however many sections are visited, so Back lands on the first", async () => {
    await render(<Bar historyEntry />);
    const before = window.history.length;
    await tab("Stores");
    await tab("Post");
    await tab("Credit");
    expect(window.history.length).toBe(before + 1);

    await phoneBack();
    expect(current()).toBe("Sales");
    expect(here()).toBe("base");
  });

  it("takes its entry away when the first section is tapped, so the next Back leaves", async () => {
    await render(<Bar historyEntry />);
    await tab("Post");
    await tab("Sales");
    await settle();
    expect(current()).toBe("Sales");
    expect(here()).toBe("base");
  });

  it("adds no entry for a tap on the first section, or on the one showing", async () => {
    await render(<Bar historyEntry />);
    const before = window.history.length;
    await tab("Sales");
    expect(window.history.length).toBe(before);
    expect(here()).toBe("base");
  });

  it("holds the entry for an app that opens on another section", async () => {
    await render(<Bar start='post' historyEntry />);
    await settle();
    expect(here()).toBe("tab");
    await phoneBack();
    expect(current()).toBe("Sales");
    expect(here()).toBe("base");
  });

  it("holds one entry under StrictMode's double effects", async () => {
    await render(
      <StrictMode>
        <Bar historyEntry />
      </StrictMode>
    );
    const before = window.history.length;
    await tab("Stores");
    expect(window.history.length).toBe(before + 1);
    await phoneBack();
    expect(current()).toBe("Sales");
    expect(here()).toBe("base");
  });
});

describe("More", () => {
  it("a section picked from More returns to the first on Back", async () => {
    await render(<Bar items={SEVEN} historyEntry />);
    await tab("More");
    expect(sheetTitles()).toEqual(["More"]);
    await more("DoorDash");
    expect(sheetTitles()).toEqual([]);
    expect(current()).toBe("More");

    await phoneBack();
    expect(current()).toBe("Sales");
    expect(here()).toBe("base");
  });

  it("Back with More open closes More and stays on the section", async () => {
    await render(<Bar items={SEVEN} historyEntry />);
    await tab("Stores");
    await tab("More");
    await phoneBack();
    expect(sheetTitles()).toEqual([]);
    expect(current()).toBe("Stores");

    await phoneBack();
    expect(current()).toBe("Sales");
    expect(here()).toBe("base");
  });
});

describe("where the bar holds no entry", () => {
  it("holds none by default", async () => {
    await render(<Bar />);
    const before = window.history.length;
    await tab("Stores");
    await tab("Post");
    expect(window.history.length).toBe(before);
    expect(here()).toBe("base");
  });

  it("leaves an app router's own per-tab entries to step Back, and Back still leaves", async () => {
    await render(<RouterBar />);
    await tab("Stores");
    await tab("Post");
    await phoneBack();
    expect(current()).toBe("Stores");
    await phoneBack();
    expect(current()).toBe("Sales");
    expect(here()).toBe("base");
  });

  it("outside the phone layout (a tablet, a desktop) nothing changes", async () => {
    phone = false;
    await render(<Bar historyEntry />);
    const before = window.history.length;
    await tab("Stores");
    expect(current()).toBe("Stores");
    expect(window.history.length).toBe(before);
    expect(here()).toBe("base");
  });
});
