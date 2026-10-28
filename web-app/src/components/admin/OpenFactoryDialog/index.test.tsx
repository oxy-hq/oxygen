// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { OpenFactoryDialog } from "./index";

/**
 * "Open Factory…": pick any org's workspace and land in its Factory, starting the
 * audited session inline instead of meeting the "needs an assumed role" toast.
 */

const startAssume = vi.fn();
let acting: { org_id: string; org_name: string; org_slug: string } | undefined;
let myOrgs: { id: string }[] = [];

vi.mock("@/hooks/api/adminAssume", () => ({
  useStartAssume: () => ({ mutate: startAssume, isPending: false })
}));
vi.mock("@/hooks/api/adminAssume/useActingSession", () => ({
  useActingSession: () => ({ session: acting, isActing: !!acting })
}));
vi.mock("@/hooks/api/organizations", () => ({ useOrgs: () => ({ data: myOrgs }) }));
// The newest page holds two; a server search by name also finds an older one.
const OLD = { id: "ws-old", name: "Legacy Ledger", org_id: "org-ph", org_slug: "pokehouse" };
// One org's workspaces, for the rail's scoped open. Set per test.
let orgWorkspaces: { id: string; name: string; org_id: string; org_slug: string }[] = [];
vi.mock("@/hooks/api/adminTenants/useAdminWorkspaces", () => ({
  PICKER_PAGE_SIZE: 2,
  useAdminWorkspacesList: () => ({ data: orgWorkspaces, isLoading: false, isError: false }),
  useAdminWorkspacesForPicker: (_enabled: boolean, search = "") => ({
    data: search
      ? [OLD].filter((w) => w.name.toLowerCase().includes(search.toLowerCase()))
      : [
          { id: "ws-nw", name: "Northwind Analytics", org_id: "org-nw", org_slug: "northwind" },
          { id: "ws-ph", name: "Pokehouse", org_id: "org-ph", org_slug: "pokehouse" }
        ],
    isLoading: false,
    isError: false
  })
}));
vi.mock("@/hooks/api/adminTenants/useAdminOrgs", () => ({
  useAllAdminOrgs: () => ({
    data: {
      pages: [
        [
          { id: "org-nw", name: "Northwind" },
          { id: "org-ph", name: "Poke House" }
        ]
      ]
    }
  })
}));
vi.mock("@/hooks/api/workspaces/useWorkspaces", () => ({
  useAllWorkspaces: () => ({
    data: [{ id: "ws-nw", name: "Northwind Analytics" }],
    isLoading: false,
    isError: false
  })
}));

function Location() {
  return <div data-testid='location'>{useLocation().pathname}</div>;
}

const renderDialog = () =>
  render(
    <MemoryRouter initialEntries={["/admin/workspaces"]}>
      <OpenFactoryDialog open onOpenChange={vi.fn()} />
      <Routes>
        <Route path='*' element={<Location />} />
      </Routes>
    </MemoryRouter>
  );

// cmdk measures its list; jsdom has no ResizeObserver or scrollIntoView.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;
Element.prototype.scrollIntoView ??= () => {};

beforeEach(() => {
  acting = undefined;
  myOrgs = [];
  startAssume.mockClear();
  localStorage.clear();
});
afterEach(cleanup);

describe("Open Factory from inside an org (the rail)", () => {
  const scope = { orgId: "org-nw", orgSlug: "northwind", orgName: "Northwind" };
  const renderScoped = () =>
    render(
      <MemoryRouter initialEntries={["/northwind/home"]}>
        <OpenFactoryDialog open onOpenChange={vi.fn()} initialScope={scope} />
        <Routes>
          <Route path='*' element={<Location />} />
        </Routes>
      </MemoryRouter>
    );

  it("goes straight to the reason for an org with one workspace", () => {
    orgWorkspaces = [
      { id: "ws-nw", name: "Northwind Analytics", org_id: "org-nw", org_slug: "northwind" }
    ];
    renderScoped();
    expect(screen.getByTestId("open-factory-start")).toBeInTheDocument();
    expect(screen.getByText("Open Northwind Analytics in the Factory")).toBeInTheDocument();
  });

  it("lists only this org's workspaces when it has several, with a way to search all", () => {
    orgWorkspaces = [
      { id: "ws-nw", name: "Northwind Analytics", org_id: "org-nw", org_slug: "northwind" },
      { id: "ws-nw2", name: "Northwind Sandbox", org_id: "org-nw", org_slug: "northwind" }
    ];
    renderScoped();
    expect(screen.getByTestId("open-factory-item-ws-nw2")).toBeInTheDocument();
    expect(screen.queryByText("Poke House")).toBeNull();
    fireEvent.click(screen.getByTestId("open-factory-search-all"));
    expect(screen.getByText("Poke House")).toBeInTheDocument();
  });
});

describe("Open Factory", () => {
  it("says when it shows only the newest page, and finds an older one by name", async () => {
    renderDialog();
    expect(screen.getByText(/Showing the newest 2 workspaces/)).toBeInTheDocument();
    fireEvent.change(screen.getByPlaceholderText("Search organizations and workspaces…"), {
      target: { value: "ledger" }
    });
    expect(await screen.findByTestId("open-factory-item-ws-old")).toBeInTheDocument();
  });

  it("lists every org's workspaces under the org's name", () => {
    renderDialog();
    expect(screen.getByText("Northwind")).toBeInTheDocument();
    expect(screen.getByText("Poke House")).toBeInTheDocument();
    expect(screen.getByTestId("open-factory-item-ws-ph")).toHaveTextContent("Pokehouse");
  });

  it("asks for a reason and starts the session that lands on that Factory", () => {
    renderDialog();
    fireEvent.click(screen.getByTestId("open-factory-item-ws-nw"));
    expect(screen.getByTestId("open-factory-submit")).toBeDisabled();
    fireEvent.change(screen.getByTestId("open-factory-reason"), {
      target: { value: "publishing their forecast app" }
    });
    fireEvent.click(screen.getByTestId("open-factory-submit"));
    expect(startAssume).toHaveBeenCalledWith({
      orgId: "org-nw",
      reason: "publishing their forecast app"
    });
    // Where the session lands, recorded before it starts (the start is a reload).
    const recorded = JSON.parse(localStorage.getItem("oxy.assume.destination") ?? "{}");
    expect(recorded.landing).toBe("/northwind/workspaces/ws-nw/ide");
    expect(recorded.returnTo).toBe("/");
  });

  it("opens an org the person is a member of without a session", () => {
    myOrgs = [{ id: "org-nw" }];
    renderDialog();
    fireEvent.click(screen.getByTestId("open-factory-item-ws-nw"));
    expect(startAssume).not.toHaveBeenCalled();
    expect(screen.getByTestId("location")).toHaveTextContent("/northwind/workspaces/ws-nw/ide");
  });

  it("while acting, offers that org's workspaces and opens them directly", () => {
    acting = { org_id: "org-nw", org_name: "Northwind", org_slug: "northwind" };
    renderDialog();
    expect(screen.queryByText("Poke House")).toBeNull();
    fireEvent.click(screen.getByTestId("open-factory-item-ws-nw"));
    expect(startAssume).not.toHaveBeenCalled();
    expect(screen.getByTestId("location")).toHaveTextContent("/northwind/workspaces/ws-nw/ide");
  });
});
