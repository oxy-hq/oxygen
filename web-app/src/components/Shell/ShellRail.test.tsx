// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { createPortal } from "react-dom";
import { afterEach, describe, expect, it } from "vitest";
import { type RailItem, ShellRail } from "./ShellRail";

afterEach(cleanup);

const appItem: RailItem = {
  key: "app-1",
  label: "Bookkeeping",
  testId: "rail-app-bookkeeping",
  letter: "B",
  href: "/customer-apps/poke-house/bookkeeping/"
};

describe("ShellRail app entries", () => {
  /// The rail is the dock: a tile takes this tab to the app, which owns the
  /// address bar (its deep links and reload work) and draws the same rail, so
  /// the next app is one click away there too. apps-first-shell.md § 4.1.
  it("opens an app in this tab, as a plain anchor", () => {
    render(<ShellRail groups={[[appItem]]} />);
    const link = screen.getByTestId("rail-app-bookkeeping");
    expect(link).toHaveAttribute("href", "/customer-apps/poke-house/bookkeeping/");
    expect(link).not.toHaveAttribute("target");
    expect(link).toHaveAccessibleName("Bookkeeping");
  });
});

describe("ShellRail opening from the keyboard", () => {
  const homeItem: RailItem = { key: "hq", label: "Home", testId: "rail-hq", letter: "H" };
  const panel = () => screen.getByTestId("shell-rail-panel");

  /// Tab is what opens it: `keyup` fires where focus landed, so a Tab into the
  /// rail is seen and the rail shows its names until focus leaves.
  it("opens when Tab lands focus in it, and closes when focus leaves", () => {
    render(
      <>
        <ShellRail groups={[[homeItem, appItem]]} />
        <button type='button'>page</button>
      </>
    );
    const home = screen.getByTestId("rail-hq");
    fireEvent.keyUp(home, { key: "Tab" });
    expect(panel()).toHaveAttribute("data-tabbed-in");

    fireEvent.focusOut(home, { relatedTarget: screen.getByTestId("rail-app-bookkeeping") });
    expect(panel()).toHaveAttribute("data-tabbed-in");

    fireEvent.focusOut(home, { relatedTarget: screen.getByRole("button", { name: "page" }) });
    expect(panel()).not.toHaveAttribute("data-tabbed-in");
  });

  /// The review's F1: a menu closed with Escape hands focus back to its trigger
  /// in the rail, visibly, and the rail used to stay open over the page after
  /// the pointer had gone.
  it("does not open for the Escape that closed a menu and gave its trigger focus back", () => {
    render(
      <ShellRail
        groups={[[homeItem]]}
        bottom={
          <button type='button' data-testid='user-menu'>
            LV
          </button>
        }
      />
    );
    fireEvent.keyUp(screen.getByTestId("user-menu"), { key: "Escape" });
    expect(panel()).not.toHaveAttribute("data-tabbed-in");
  });

  /// A menu opened from the rail renders in a portal: its keys are React
  /// children of the rail but not DOM children, and do not open it.
  it("ignores keys pressed inside a portalled menu", () => {
    render(
      <ShellRail
        groups={[[homeItem]]}
        bottom={createPortal(
          <button type='button' data-testid='menu-item'>
            Settings
          </button>,
          document.body
        )}
      />
    );
    fireEvent.keyUp(screen.getByTestId("menu-item"), { key: "Tab" });
    expect(panel()).not.toHaveAttribute("data-tabbed-in");
  });
});
