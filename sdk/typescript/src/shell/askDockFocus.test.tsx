// @vitest-environment jsdom
//
// Opening Ask Oxygen inside a custom app (⌘K or the top-bar button) must put
// the caret in the composer, on every open. The drawer stays mounted between
// opens, so nothing about mounting moves focus; the dock has to do it itself.

import * as React from "react";
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mount, type Page, servePage, settle } from "../custom-app/notifications.harness";
import { AskDock } from "./AskDock";

let page: Page | undefined;
let setOpen: (open: boolean) => void = () => {};

function Harness() {
  const [open, set] = React.useState(false);
  setOpen = set;
  return (
    <>
      <button type='button' data-testid='page'>
        page
      </button>
      <AskDock agentId='agents/ask.agentic.yml' open={open} onClose={() => set(false)} />
    </>
  );
}

const input = () => document.querySelector<HTMLTextAreaElement>(".oxy-askdock__input");
const pageButton = () => document.querySelector<HTMLButtonElement>('[data-testid="page"]');
const toggle = (open: boolean) => act(() => setOpen(open));

beforeEach(async () => {
  vi.useFakeTimers();
  // jsdom has no `scrollTo`; the dock's stick-to-bottom effect calls it.
  Element.prototype.scrollTo = () => {};
  servePage();
  const fetcher = (async () =>
    new Response(JSON.stringify([]), { status: 200 })) as unknown as typeof fetch;
  page = await mount(<Harness />, fetcher);
  await settle();
});

afterEach(async () => {
  await page?.unmount();
  page = undefined;
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe("AskDock focus", () => {
  it("focuses the composer on every open, not just the first", () => {
    expect(input()).not.toBeNull();
    expect(document.activeElement).not.toBe(input());

    toggle(true);
    expect(document.activeElement).toBe(input());

    toggle(false);
    pageButton()?.focus();
    toggle(true);
    expect(document.activeElement).toBe(input());
  });

  it("leaves focus alone when the dock closes", () => {
    toggle(true);
    pageButton()?.focus();
    toggle(false);
    expect(document.activeElement).toBe(pageButton());
  });
});
