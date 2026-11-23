// @vitest-environment jsdom
//
// 2.34.0: `<OxyShell>` has no Ask entry and no Chat entry, as Oxygen's own chrome has
// none: no top-bar button, no ⌘K / Ctrl+K, no dock, even for an app whose manifest
// binds an agent. The shortcut is the app's to use. Mounted for real against a fake
// shell-context.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mount, type Page, servePage, settle } from "../custom-app/notifications.harness";
import { OxyShell } from "./OxyShell";
import type { ShellContextData } from "./shellContext";

const SHELL_CONTEXT: ShellContextData = {
  workspace: { id: "ws-1", name: "Default" },
  org: { id: "org-1", slug: "acme", name: "Poke House" },
  logo_url: null,
  apps: [
    {
      id: "app-1",
      name: "Bookings",
      slug: "bookings",
      url: "/customer-apps/acme/bookings/",
      // The app binds an agent: before 2.34.0 that put Ask in its top bar.
      default_agent: "agents/ask.agentic.yml"
    },
    {
      id: "app-2",
      name: "Command Center",
      slug: "command-center",
      url: "/customer-apps/acme/cc/",
      icon_url: "/customer-apps/acme/cc/icon.svg"
    }
  ],
  links: { home: "/", threads: "/threads" },
  user: null
};

let page: Page | undefined;

beforeEach(() => {
  vi.useFakeTimers();
  servePage();
});

afterEach(async () => {
  await page?.unmount();
  page = undefined;
  vi.restoreAllMocks();
  vi.useRealTimers();
});

const $ = (sel: string) => document.querySelector(sel);

describe("OxyShell, like Oxygen's own chrome", () => {
  it("draws no Ask button, no Chat entry and no dock, and leaves ⌘K alone", async () => {
    const fetcher = (async () =>
      new Response(JSON.stringify(SHELL_CONTEXT), { status: 200 })) as unknown as typeof fetch;
    page = await mount(<OxyShell>app body</OxyShell>, fetcher);
    await settle();

    expect($('[data-testid="rail-hq"]')).not.toBeNull();
    expect($('[data-testid="ask-oxygen-button"]')).toBeNull();
    expect($('[data-testid="rail-chat"]')).toBeNull();
    expect($(".oxy-askdock")).toBeNull();

    const key = new KeyboardEvent("keydown", { key: "k", metaKey: true, cancelable: true });
    window.dispatchEvent(key);
    await settle();
    expect(key.defaultPrevented).toBe(false);
    expect($(".oxy-askdock")).toBeNull();
  });

  it("draws an app as its mark in the row's colour, never its coloured icon", async () => {
    const fetcher = (async () =>
      new Response(JSON.stringify(SHELL_CONTEXT), { status: 200 })) as unknown as typeof fetch;
    page = await mount(<OxyShell>app body</OxyShell>, fetcher);
    await settle();
    const entry = $('[data-testid="rail-app-command-center"]');
    expect(entry?.querySelector("img")).toBeNull();
    expect(entry?.querySelector("[data-glyph]")).not.toBeNull();
  });
});
