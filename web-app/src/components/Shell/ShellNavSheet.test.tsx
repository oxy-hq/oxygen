// @vitest-environment jsdom

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { createBrowserRouter, RouterProvider, useLocation, useNavigate } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ShellNavSheet } from "./ShellNavSheet";
import type { RailItem } from "./ShellRail";

afterEach(cleanup);

const appItem: RailItem = {
  key: "app-1",
  label: "Bookkeeping",
  testId: "rail-app-bookkeeping",
  letter: "B",
  href: "/customer-apps/poke-house/bookkeeping/"
};

const open = () => fireEvent.click(screen.getByTestId("nav-sheet-trigger"));

describe("ShellNavSheet", () => {
  /// The point of the drawer: on a touch screen the rail's names live only in
  /// hover tooltips, so the drawer has to print every entry's name.
  it("shows every entry with its name once opened", () => {
    render(
      <ShellNavSheet
        title='Poke House'
        groups={[[{ key: "hq", label: "HQ", testId: "rail-hq", letter: "H" }], [appItem]]}
        footerItems={[{ key: "core", label: "Oxygen Factory", testId: "rail-core", letter: "O" }]}
      />
    );
    expect(screen.queryByTestId("nav-sheet")).toBeNull();
    open();
    expect(screen.getByTestId("nav-sheet-rail-hq")).toHaveTextContent("HQ");
    expect(screen.getByTestId("nav-sheet-rail-app-bookkeeping")).toHaveTextContent("Bookkeeping");
    expect(screen.getByTestId("nav-sheet-rail-core")).toHaveTextContent("Oxygen Factory");
  });

  /// Picking a destination must close the drawer, or it stays over the page
  /// the user just navigated to.
  it("runs the entry's action and closes", async () => {
    const onSelect = vi.fn();
    render(
      <ShellNavSheet
        title='Poke House'
        groups={[[{ key: "chat", label: "Chat", testId: "rail-chat", letter: "C", onSelect }]]}
      />
    );
    open();
    fireEvent.click(screen.getByTestId("nav-sheet-rail-chat"));
    expect(screen.queryByTestId("nav-sheet")).toBeNull();
    // Once the drawer's history entry has gone, so a navigation lands in its place.
    await waitFor(() => expect(onSelect).toHaveBeenCalledOnce());
  });

  /// Same contract as the rail tile: an app is a real link, opened in this tab.
  it("links an app in this tab", () => {
    render(<ShellNavSheet title='Poke House' groups={[[appItem]]} />);
    open();
    const link = screen.getByTestId("nav-sheet-rail-app-bookkeeping");
    expect(link).toHaveAttribute("href", "/customer-apps/poke-house/bookkeeping/");
    expect(link).not.toHaveAttribute("target");
    expect(link).toHaveAccessibleName("Bookkeeping");
  });

  /// Closing must hand focus back to the menu button; a bare button outside
  /// SheetTrigger left a keyboard user on <body>.
  it("returns focus to the menu button on Escape", async () => {
    render(<ShellNavSheet title='Poke House' groups={[[appItem]]} />);
    open();
    fireEvent.keyDown(screen.getByTestId("nav-sheet"), { key: "Escape" });
    expect(screen.queryByTestId("nav-sheet")).toBeNull();
    // Radix moves focus back on the next tick.
    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByTestId("nav-sheet-trigger"))
    );
  });

  /// Widening past `md` brings the rail back, so the drawer gets out of its way.
  it("closes when the window widens past md", () => {
    const listeners: Array<() => void> = [];
    const query = {
      matches: false,
      addEventListener: (_: string, fn: () => void) => listeners.push(fn),
      removeEventListener: () => {}
    };
    vi.stubGlobal("matchMedia", () => query);
    render(<ShellNavSheet title='Poke House' groups={[[appItem]]} />);
    open();
    expect(screen.getByTestId("nav-sheet")).toBeInTheDocument();
    query.matches = true;
    act(() => {
      for (const fn of listeners) fn();
    });
    expect(screen.queryByTestId("nav-sheet")).toBeNull();
    vi.unstubAllGlobals();
  });
});

// Against jsdom's real session history, under a real browser router: `back` lands
// later and fires popstate, as in a browser.
describe("ShellNavSheet and the phone's Back", () => {
  const settle = () => act(() => new Promise<void>((r) => setTimeout(r, 40)));
  const phoneBack = async () => {
    act(() => window.history.back());
    await settle();
  };
  const drawer = () => screen.queryByTestId("nav-sheet");
  const path = () => window.location.pathname;

  function Page() {
    const navigate = useNavigate();
    const { pathname } = useLocation();
    return (
      <>
        <h1 data-testid='at'>{pathname}</h1>
        <ShellNavSheet
          title='Poke House'
          groups={[
            [
              {
                key: "chat",
                label: "Chat",
                testId: "chat",
                letter: "C",
                onSelect: () => {
                  void navigate("/chat");
                }
              }
            ]
          ]}
          bottom={
            <button
              type='button'
              data-testid='inside'
              onClick={() => {
                void navigate("/inside");
              }}
            >
              Inside
            </button>
          }
        />
      </>
    );
  }

  // Each test starts on /second, reached from /first, as a Back needs somewhere to go.
  async function onSecond() {
    window.history.pushState(null, "", "/first");
    window.history.pushState(null, "", "/second");
    const router = createBrowserRouter([{ path: "*", element: <Page /> }]);
    render(<RouterProvider router={router} />);
    await settle();
    return router;
  }

  it("closes only the drawer: the page stays, and the next Back moves it", async () => {
    await onSecond();
    open();
    await settle();
    expect(drawer()).toBeInTheDocument();
    await phoneBack();
    expect(drawer()).toBeNull();
    expect(path()).toBe("/second");
    expect(screen.getByTestId("at")).toHaveTextContent("/second");
    await phoneBack();
    expect(path()).toBe("/first");
    expect(screen.getByTestId("at")).toHaveTextContent("/first");
  });

  it("leaves the history where it was when closed another way", async () => {
    await onSecond();
    const before = window.history.state;
    open();
    await settle();
    fireEvent.keyDown(screen.getByTestId("nav-sheet"), { key: "Escape" });
    await settle();
    expect(drawer()).toBeNull();
    expect(path()).toBe("/second");
    expect(window.history.state).toEqual(before);
    await phoneBack();
    expect(path()).toBe("/first");
  });

  it("does not grow the history however many times it opens", async () => {
    await onSecond();
    const length = window.history.length;
    for (let i = 0; i < 3; i++) {
      open();
      await settle();
      fireEvent.keyDown(screen.getByTestId("nav-sheet"), { key: "Escape" });
      await settle();
    }
    // One push over what was: each open reuses the slot the last one left ahead.
    expect(window.history.length).toBeLessThanOrEqual(length + 1);
    await phoneBack();
    expect(path()).toBe("/first");
  });

  it("navigates on a pick, and Back from there reaches the old page in one step", async () => {
    await onSecond();
    open();
    await settle();
    fireEvent.click(screen.getByTestId("nav-sheet-chat"));
    await settle();
    expect(drawer()).toBeNull();
    expect(path()).toBe("/chat");
    await phoneBack();
    expect(path()).toBe("/second");
    expect(screen.getByTestId("at")).toHaveTextContent("/second");
    await phoneBack();
    expect(path()).toBe("/first");
  });

  it("runs a pick's action only once the drawer's entry has gone", async () => {
    window.history.pushState(null, "", "/second");
    const seen: unknown[] = [];
    const onSelect = () =>
      seen.push((window.history.state as { oxyNavDrawer?: boolean } | null)?.oxyNavDrawer);
    render(
      <ShellNavSheet
        title='Poke House'
        groups={[[{ key: "chat", label: "Chat", testId: "chat", letter: "C", onSelect }]]}
      />
    );
    open();
    await settle();
    fireEvent.click(screen.getByTestId("nav-sheet-chat"));
    await settle();
    expect(seen).toEqual([undefined]);
  });

  it("steps over its entry when the page navigated from inside the open drawer", async () => {
    await onSecond();
    open();
    await settle();
    fireEvent.click(screen.getByTestId("inside"));
    await settle();
    expect(path()).toBe("/inside");
    // Back closes the drawer and lands on the old page, with no stop on its entry.
    await phoneBack();
    expect(drawer()).toBeNull();
    expect(path()).toBe("/second");
    expect((window.history.state as { oxyNavDrawer?: boolean }).oxyNavDrawer).toBeUndefined();
  });

  it("steps off an entry a reload left it on", async () => {
    window.history.pushState(null, "", "/first");
    window.history.pushState(null, "", "/second");
    window.history.pushState({ oxyNavDrawer: true }, "", "/second");
    render(<ShellNavSheet title='Poke House' groups={[[appItem]]} />);
    await settle();
    expect(window.history.state).toBeNull();
    expect(path()).toBe("/second");
  });
});
