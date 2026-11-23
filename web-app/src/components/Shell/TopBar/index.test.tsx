// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TopBar } from "./index";

/**
 * The top bar is the reference design's: the breadcrumb, then the status items
 * (only while something is wrong) and the bell. No Ask button (⌘K opens Ask), no
 * Today, no clock.
 */

vi.mock("./Breadcrumb", () => ({ Breadcrumb: () => null }));
vi.mock("./NotificationsBell/DockBell", () => ({
  DockBell: () => <span data-testid='dock-bell' />
}));
vi.mock("./MenuBar/AppHealthItem", () => ({
  AppHealthItem: () => <span data-testid='menubar-health' />
}));
let online = true;
vi.mock("@/hooks/useOnlineStatus", () => ({ default: () => online }));

afterEach(() => {
  cleanup();
  online = true;
});

const order = () =>
  Array.from(screen.getByTestId("menubar-status").children).map(
    (el) => el.getAttribute("data-testid") ?? el.firstElementChild?.getAttribute("data-testid")
  );

describe("the top bar", () => {
  it("has the status items and the bell: no Ask button, no Today, no clock", () => {
    render(<TopBar />);
    expect(screen.getByTestId("workspace-topbar")).toHaveAttribute("data-menubar", "true");
    expect(order()).toEqual(["menubar-health", "dock-bell"]);
    expect(screen.queryByTestId("ask-oxygen-button")).toBeNull();
  });

  it("says Offline when the browser is offline", () => {
    online = false;
    render(<TopBar />);
    expect(screen.getByTestId("menubar-connection")).toHaveAccessibleName("Offline");
    expect(order()).toEqual(["menubar-health", "menubar-connection", "dock-bell"]);
  });

  it("hides its bell on a phone when the page draws its own", () => {
    render(<TopBar bellOnPage />);
    expect(screen.getByTestId("dock-bell").parentElement?.className).toContain("max-md:hidden");
  });
});
