// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
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
