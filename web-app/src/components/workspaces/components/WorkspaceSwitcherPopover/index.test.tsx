// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { WorkspaceSwitcherPopover } from "./index";

/**
 * The set switcher. Flag off: "Manage workspaces" and a switch that lands on the
 * other workspace's home, as before. Apps-first: only the Factory has it, so it
 * keeps the engineer in the Factory and offers no management.
 */

let appsFirst = false;
vi.mock("@/hooks/useAppsFirstExperience", () => ({ default: () => appsFirst }));
vi.mock("@/components/ui/shadcn/sidebar-context", () => ({
  default: () => ({ isMobile: false, setOpenMobile: vi.fn() })
}));
vi.mock("@/hooks/api/workspaces/useWorkspaces", () => ({
  useAllWorkspaces: () => ({
    data: [
      { id: "ws-1", name: "Analytics", status: "ready", org_id: "org-1" },
      { id: "ws-2", name: "Sandbox", status: "ready", org_id: "org-1" }
    ]
  })
}));
vi.mock("@/stores/useCurrentOrg", () => ({
  default: <T,>(selector: (s: unknown) => T) => selector({ org: { id: "org-1", slug: "acme" } })
}));
vi.mock("@/stores/useCurrentWorkspace", () => ({
  default: () => ({ workspace: { id: "ws-1" } })
}));

function Location() {
  return <div data-testid='location'>{useLocation().pathname}</div>;
}

const openSwitcher = () => {
  render(
    <MemoryRouter initialEntries={["/acme/workspaces/ws-1/ide"]}>
      <WorkspaceSwitcherPopover>
        <button type='button'>switch</button>
      </WorkspaceSwitcherPopover>
      <Routes>
        <Route path='*' element={<Location />} />
      </Routes>
    </MemoryRouter>
  );
  fireEvent.click(screen.getByText("switch"));
};

afterEach(() => {
  cleanup();
  appsFirst = false;
});

describe("the set switcher", () => {
  it("offers Manage workspaces and lands on the other's home with the flag off", () => {
    openSwitcher();
    expect(screen.getByText("Manage workspaces")).toBeInTheDocument();
    fireEvent.click(screen.getByText("Sandbox"));
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/acme\/workspaces\/ws-2$/);
  });

  it("offers no management and stays in the Factory with the flag on", () => {
    appsFirst = true;
    openSwitcher();
    expect(screen.queryByText("Manage workspaces")).toBeNull();
    fireEvent.click(screen.getByText("Sandbox"));
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/acme\/workspaces\/ws-2\/ide$/);
  });
});
