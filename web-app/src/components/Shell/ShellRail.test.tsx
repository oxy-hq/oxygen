// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { RAIL_EXPANDED_KEY, type RailItem, ShellRail } from "./ShellRail";

afterEach(() => {
  cleanup();
  localStorage.clear();
});

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

describe("ShellRail opening and closing", () => {
  const homeItem: RailItem = { key: "hq", label: "Home", testId: "rail-hq", letter: "H" };
  const rail = () => screen.getByTestId("shell-rail");
  const toggle = () => screen.getByTestId("shell-rail-toggle");
  const mark = <span data-testid='mark'>P</span>;

  /// Closed by default, as the reference app's rail is: marks only, each still
  /// named for a screen reader, and the head's one cell is the way to open it.
  it("starts closed, with the mark as its toggle", () => {
    render(<ShellRail top={mark} title='Poke House' groups={[[homeItem, appItem]]} />);
    expect(rail()).not.toHaveAttribute("data-expanded");
    expect(toggle()).toHaveAccessibleName("Expand sidebar");
    expect(toggle()).toHaveAttribute("aria-expanded", "false");
    expect(toggle()).toContainElement(screen.getByTestId("mark"));
    expect(screen.queryByTestId("shell-rail-title")).toBeNull();
    expect(screen.getByTestId("rail-hq")).toHaveAccessibleName("Home");
  });

  /// The complaint this replaced: the rail opened on hover (and on Tab), over the
  /// page. It opens only from its toggle now.
  it("does not open on hover or when Tab lands in it", () => {
    render(<ShellRail groups={[[homeItem]]} />);
    fireEvent.mouseEnter(rail());
    fireEvent.pointerEnter(rail());
    fireEvent.keyUp(screen.getByTestId("rail-hq"), { key: "Tab" });
    expect(rail()).not.toHaveAttribute("data-expanded");
  });

  /// Open, the head carries the mark, the workspace's name and the toggle that
  /// closes it again — the close button the hover rail never had.
  it("opens and closes from its toggle", () => {
    render(<ShellRail top={mark} title='Poke House' groups={[[homeItem]]} />);
    fireEvent.click(toggle());
    expect(rail()).toHaveAttribute("data-expanded");
    expect(toggle()).toHaveAccessibleName("Collapse sidebar");
    expect(toggle()).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByTestId("shell-rail-title")).toHaveTextContent("Poke House");
    expect(screen.getByTestId("mark")).toBeInTheDocument();

    fireEvent.click(toggle());
    expect(rail()).not.toHaveAttribute("data-expanded");
    expect(screen.queryByTestId("shell-rail-title")).toBeNull();
  });

  /// Enter on the toggle must leave focus on the toggle: the closed and open heads
  /// draw it in different places, and a remount there would drop focus to the page.
  it("keeps keyboard focus on the toggle as it opens and closes", () => {
    render(<ShellRail top={mark} title='Poke House' groups={[[homeItem]]} />);
    toggle().focus();
    fireEvent.click(toggle());
    expect(rail()).toHaveAttribute("data-expanded");
    expect(toggle()).toHaveFocus();
    fireEvent.click(toggle());
    expect(rail()).not.toHaveAttribute("data-expanded");
    expect(toggle()).toHaveFocus();
  });

  /// It takes its width in the layout rather than floating over the page: the
  /// rail itself is the flex item that changes size, with no overlay panel.
  it("is the element that changes width, in the page's flow", () => {
    render(<ShellRail groups={[[homeItem]]} />);
    expect(rail()).toHaveClass("w-12", "shrink-0");
    expect(rail()).not.toHaveClass("absolute");
    fireEvent.click(toggle());
    expect(rail()).toHaveClass("w-56");
  });

  /// Remembered per browser, as ChatGPT's and Gemini's are: a reload or the next
  /// page opens the rail the way it was left.
  it("remembers how it was left", () => {
    const { unmount } = render(<ShellRail groups={[[homeItem]]} />);
    fireEvent.click(toggle());
    unmount();
    expect(localStorage.getItem(RAIL_EXPANDED_KEY)).toBe("true");

    render(<ShellRail groups={[[homeItem]]} />);
    expect(rail()).toHaveAttribute("data-expanded");
  });
});
