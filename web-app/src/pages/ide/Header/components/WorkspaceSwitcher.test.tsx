// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { WorkspaceSwitcher } from "./WorkspaceSwitcher";

/**
 * The Factory's top bar names the workspace and, where there is somewhere to go,
 * moves to another: an org's other workspaces for anyone in its Factory, and other
 * organizations for Oxy staff, who move between customers here.
 */

const ws = (id: string, name: string) => ({ id, name, org_id: "org-acme" });
let workspaces = [ws("ws-1", "Acme Analytics")];
let staff = false;

vi.mock("@/hooks/api/workspaces/useWorkspaces", () => ({
  useAllWorkspaces: () => ({ data: workspaces })
}));
vi.mock("@/hooks/useStaffFactory", () => ({ useOperatesPlatform: () => staff }));
vi.mock("@/stores/useCurrentOrg", () => ({
  default: <T,>(selector: (s: { org: { id: string; slug: string; name: string } }) => T) =>
    selector({ org: { id: "org-acme", slug: "acme", name: "Acme" } })
}));
vi.mock("@/stores/useCurrentWorkspace", () => ({
  default: () => ({ workspace: { id: "ws-1", name: "Acme Analytics" } })
}));
// The dialog is tested on its own; here only whether the switcher opens it.
vi.mock("@/components/admin/OpenFactoryDialog", () => ({
  OpenFactoryDialog: ({ open }: { open: boolean }) =>
    open ? <div data-testid='open-factory-dialog' /> : null
}));

// cmdk measures its list; jsdom has no ResizeObserver or scrollIntoView.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;
Element.prototype.scrollIntoView ??= () => {};

function Location() {
  return <div data-testid='location'>{useLocation().pathname}</div>;
}

const renderAt = (path: string) =>
  render(
    <MemoryRouter initialEntries={[path]}>
      <WorkspaceSwitcher />
      <Routes>
        <Route path='*' element={<Location />} />
      </Routes>
    </MemoryRouter>
  );

afterEach(() => {
  cleanup();
  workspaces = [ws("ws-1", "Acme Analytics")];
  staff = false;
});

describe("the Factory's workspace switcher", () => {
  it("is only the name for an org's own admins when it has one workspace", () => {
    renderAt("/acme/workspaces/ws-1/ide/semantic");
    expect(screen.getByTestId("ide-workspace-name")).toHaveTextContent("Acme Analytics");
    expect(screen.queryByTestId("ide-workspace-switcher")).toBeNull();
  });

  it("moves to another of the org's workspaces on the same section", () => {
    workspaces = [ws("ws-1", "Acme Analytics"), ws("ws-2", "Acme Sandbox")];
    renderAt("/acme/workspaces/ws-1/ide/coordinator/runs/run-9");
    fireEvent.click(screen.getByTestId("ide-workspace-switcher"));
    expect(screen.queryByTestId("ide-open-other-org")).toBeNull();
    fireEvent.click(screen.getByTestId("ide-workspace-option-ws-2"));
    expect(screen.getByTestId("location")).toHaveTextContent(
      /^\/acme\/workspaces\/ws-2\/ide\/coordinator\/runs$/
    );
  });

  it("stays put when the current workspace is picked", () => {
    workspaces = [ws("ws-1", "Acme Analytics"), ws("ws-2", "Acme Sandbox")];
    renderAt("/acme/workspaces/ws-1/ide/semantic");
    fireEvent.click(screen.getByTestId("ide-workspace-switcher"));
    fireEvent.click(screen.getByTestId("ide-workspace-option-ws-1"));
    expect(screen.getByTestId("location")).toHaveTextContent(
      /^\/acme\/workspaces\/ws-1\/ide\/semantic$/
    );
  });

  it("names the organization for staff and opens another organization's Factory", () => {
    staff = true;
    renderAt("/acme/workspaces/ws-1/ide");
    const trigger = screen.getByTestId("ide-workspace-switcher");
    expect(trigger).toHaveTextContent("Acme/Acme Analytics");
    fireEvent.click(trigger);
    expect(screen.queryByTestId("open-factory-dialog")).toBeNull();
    fireEvent.click(screen.getByTestId("ide-open-other-org"));
    expect(screen.getByTestId("open-factory-dialog")).toBeInTheDocument();
  });
});
