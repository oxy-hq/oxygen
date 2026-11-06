// Shared by the phone kit's jsdom tests (not itself a test file): mount a tree
// for real, and drive it the way a person does, inside `act`.

import * as React from "react";
import { createRoot, type Root } from "react-dom/client";

// Tell React this is a test runner, so `act` flushes without a warning.
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

export interface Mounted {
  container: HTMLDivElement;
  rerender(ui: React.ReactNode): Promise<void>;
  unmount(): Promise<void>;
}

export async function render(ui: React.ReactNode, host?: HTMLElement): Promise<Mounted> {
  const container = document.createElement("div");
  (host ?? document.body).appendChild(container);
  const root: Root = createRoot(container);
  await React.act(async () => root.render(ui));
  return {
    container,
    rerender: async (next) => {
      await React.act(async () => root.render(next));
    },
    unmount: async () => {
      await React.act(async () => root.unmount());
      container.remove();
    }
  };
}

export const $ = (sel: string) => document.querySelector<HTMLElement>(sel);
export const $$ = (sel: string) => [...document.querySelectorAll<HTMLElement>(sel)];

export async function click(el: Element | null): Promise<void> {
  if (!el) throw new Error("nothing to click");
  await React.act(async () => {
    el.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
  });
}

/** A key pressed on whatever has focus, bubbling up to window as in a browser. */
export async function press(key: string, init: KeyboardEventInit = {}): Promise<void> {
  const target = document.activeElement ?? document.body;
  await React.act(async () => {
    target.dispatchEvent(
      new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init })
    );
  });
}

export async function fire(el: Element | null, event: Event): Promise<void> {
  if (!el) throw new Error("no element");
  await React.act(async () => {
    el.dispatchEvent(event);
  });
}

/** A `matchMedia` whose answer the test sets and can flip later. */
export function stubMatchMedia(initial: boolean) {
  const listeners = new Set<() => void>();
  const queries: string[] = [];
  const state = { matches: initial };
  const matchMedia = (query: string) => {
    queries.push(query);
    return {
      get matches() {
        return state.matches;
      },
      media: query,
      addEventListener: (_: string, fn: () => void) => listeners.add(fn),
      removeEventListener: (_: string, fn: () => void) => listeners.delete(fn)
    };
  };
  return {
    matchMedia,
    queries,
    listeners,
    async set(matches: boolean) {
      state.matches = matches;
      await React.act(async () => {
        for (const fn of listeners) fn();
      });
    }
  };
}
