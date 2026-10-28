// @vitest-environment jsdom

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
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
  it("runs the entry's action and closes", () => {
    const onSelect = vi.fn();
    render(
      <ShellNavSheet
        title='Poke House'
        groups={[[{ key: "chat", label: "Chat", testId: "rail-chat", letter: "C", onSelect }]]}
      />
    );
    open();
    fireEvent.click(screen.getByTestId("nav-sheet-rail-chat"));
    expect(onSelect).toHaveBeenCalledOnce();
    expect(screen.queryByTestId("nav-sheet")).toBeNull();
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
