// @vitest-environment jsdom
//
// TopBarLeft in the shell's three states, inside a real OxyAppProvider: shell-context loaded
// (the shell's own breadcrumb with the values OxyShell gives it, then the strip), still loading
// (the strip alone), unavailable (a plain-text stand-in with the app's own name, then the
// strip); and no strip for a section of one screen.

import * as React from "react";
import { afterAll, afterEach, beforeEach, describe, expect, it } from "vitest";
import { servePage } from "../../custom-app/notifications.harness";
import { OxyAppProvider } from "../../custom-app/react";
import type { ShellContextData } from "../../shell/shellContext";
import { type PaintMode, paintUi, rgb, themeValue, unpaintUi } from "../paint.harness";
import { $, $$, render } from "../ui.harness";
import { TopBarLeft, type TopBarLeftProps } from "./TopBarLeft";

const SHELL: ShellContextData = {
  workspace: { id: "ws-1", name: "Default" },
  org: { id: "org-1", slug: "acme", name: "Acme Foods" },
  logo_url: null,
  apps: [{ id: "app-1", slug: "bookings", name: "Bookings", url: "/a/bookings/" }],
  links: { home: "/home", threads: "/threads" },
  user: null
};

type Answer = "ok" | "pending" | "fail";
let answer: Answer = "ok";
const fetcher = (async (input: RequestInfo | URL) => {
  const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
  if (url.includes("/shell-context")) {
    if (answer === "pending") return new Promise<Response>(() => undefined);
    if (answer === "fail") return new Response("{}", { status: 500 });
    return new Response(JSON.stringify(SHELL));
  }
  return new Response("{}", { status: 404 });
}) as unknown as typeof fetch;

const TABS: TopBarLeftProps["tabs"] = {
  label: "Screen",
  current: "list",
  tabs: [
    { id: "list", label: "List", href: "/a/bookings/" },
    { id: "calendar", label: "Calendar", href: "/a/bookings/calendar" }
  ]
};

beforeEach(() => {
  answer = "ok";
  servePage();
});

afterEach(() => {
  document.body.innerHTML = "";
});

async function mount(props: Partial<TopBarLeftProps> = {}) {
  await render(
    <OxyAppProvider fetcher={fetcher}>
      <div data-testid='bar'>
        <TopBarLeft tabs={TABS} {...props} />
      </div>
    </OxyAppProvider>
  );
  // The manifest, then shell-context.
  await React.act(async () => {
    await new Promise((r) => setTimeout(r, 20));
  });
}

const strip = () => $('nav[data-placement="topbar"]');

describe("TopBarLeft", () => {
  it("draws the shell's breadcrumb with the shell's values, then the strip, as siblings in the bar", async () => {
    await mount();
    const crumb = $(".oxy-breadcrumb") as HTMLElement;
    expect($('[data-testid="topbar-workspace-link"]')?.textContent).toBe("Acme Foods");
    expect($('[data-testid="topbar-workspace-link"]')?.getAttribute("href")).toBe("/home");
    expect($(".oxy-breadcrumb__page")?.textContent).toBe("Bookings");
    // A fragment: the breadcrumb's box and the strip are both the bar's own children.
    const bar = $('[data-testid="bar"]') as HTMLElement;
    expect([...bar.children]).toEqual([crumb.parentElement, strip()]);
    expect($$("nav a").map((a) => a.textContent)).toEqual(["List", "Calendar"]);
  });

  it("resolves the HQ link on productBaseUrl, as OxyShell does, and takes a pageLabel", async () => {
    await mount({ productBaseUrl: "https://app.example.test/", pageLabel: "Calendar" });
    expect($('[data-testid="topbar-workspace-link"]')?.getAttribute("href")).toBe(
      "https://app.example.test/home"
    );
    expect($(".oxy-breadcrumb__page")?.textContent).toBe("Calendar");
  });

  it("draws the strip alone while shell-context is loading, as the shell draws nothing there yet", async () => {
    answer = "pending";
    await mount();
    expect($(".oxy-breadcrumb")).toBeNull();
    expect($('[data-testid="breadcrumb-fallback"]')).toBeNull();
    expect(strip()).not.toBeNull();
  });

  it("draws a plain-text stand-in with the app's own name when shell-context is unavailable", async () => {
    answer = "fail";
    await mount();
    expect($(".oxy-breadcrumb")).toBeNull();
    const fallback = $('[data-testid="breadcrumb-fallback"]') as HTMLElement;
    expect(fallback.textContent).toBe("bookings");
    expect(fallback.querySelector("a")).toBeNull();
    expect(strip()).not.toBeNull();
  });

  it("draws no strip for a section of one screen", async () => {
    await mount({ tabs: null });
    expect($(".oxy-breadcrumb")).not.toBeNull();
    expect($("nav")).toBeNull();
  });
});

// The stand-in with the caller's names: the reference's local stand-in printed "Northwind
// Kitchens / Reference App" (oxy-hq/reference-app@81ee8aa `src/chrome/TopBarLeft.tsx`
// BreadcrumbFallback, its fixture's COMPANY), where the SDK's showed the app's name alone.
describe("TopBarLeft stand-in names", () => {
  afterAll(() => unpaintUi());
  const standIn = { workspaceLabel: "Northwind Kitchens" };

  it("prints the caller's workspace, a slash and the page where shell-context is unavailable", async () => {
    answer = "fail";
    await mount({ standIn });
    const fallback = $('[data-testid="breadcrumb-fallback"]') as HTMLElement;
    expect([...fallback.children].map((c) => c.textContent)).toEqual([
      "Northwind Kitchens",
      "/",
      "bookings"
    ]);
    expect(fallback.querySelector("a")).toBeNull();
    expect(strip()).not.toBeNull();
  });

  it("takes the stand-in's own page name when it gives one", async () => {
    answer = "fail";
    await mount({ standIn: { workspaceLabel: "Northwind Kitchens", pageLabel: "Reference App" } });
    expect($('[data-testid="breadcrumb-fallback"]')?.textContent).toBe(
      "Northwind Kitchens/Reference App"
    );
  });

  it("changes nothing inside the shell: the shell's names, its link, no stand-in", async () => {
    await mount({ standIn });
    expect($('[data-testid="breadcrumb-fallback"]')).toBeNull();
    expect($('[data-testid="topbar-workspace-link"]')?.textContent).toBe("Acme Foods");
    expect($(".oxy-breadcrumb__page")?.textContent).toBe("Bookings");
  });

  for (const mode of ["light", "dark"] as PaintMode[])
    it(`draws it as the reference did: 13px, 8px apart, the workspace semibold in the body ink (${mode})`, async () => {
      answer = "fail";
      paintUi({ mode });
      await mount({ standIn });
      const fallback = getComputedStyle($(".oxy-ui-top-bar-left__fallback--crumbs") as HTMLElement);
      const workspace = getComputedStyle(
        $(".oxy-ui-top-bar-left__fallback-workspace") as HTMLElement
      );
      expect([fallback.display, fallback.fontSize, fallback.gap]).toEqual(["flex", "13px", "8px"]);
      expect(fallback.color).toBe(rgb(themeValue("--muted-foreground", mode)));
      expect([workspace.fontWeight, workspace.color]).toEqual([
        "600",
        rgb(themeValue("--foreground", mode))
      ]);
    });
});

// The breadcrumb's box from 1024px: the Sections column's 220px less the bar's 12px padding and
// 12px gap, 196px, the reference's `lg:min-w-[12.25rem]`.
describe("TopBarLeft, painted", () => {
  afterAll(() => unpaintUi());

  it("holds the breadcrumb's box to 196px (12.25rem) from 1024px", async () => {
    paintUi({ media: ["(min-width: 1024px)"] });
    await mount();
    expect(getComputedStyle($(".oxy-ui-top-bar-left") as HTMLElement).minWidth).toBe("196px");
  });
});
