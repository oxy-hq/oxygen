// @vitest-environment jsdom

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { cleanup, render, screen } from "@testing-library/react";
import { useEffect } from "react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { apiClient } from "@/services/api/axios";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import type { Workspace } from "@/types/workspace";
import { FactoryGuard } from "./FactoryGuard";

function renderAtIde() {
  render(
    <MemoryRouter initialEntries={["/acme/workspaces/ws-1/ide"]}>
      <Routes>
        <Route
          path='/acme/workspaces/ws-1/ide'
          element={
            <FactoryGuard>
              <div>the IDE</div>
            </FactoryGuard>
          }
        />
        <Route path='/acme/workspaces/ws-1/home' element={<div>home</div>} />
      </Routes>
    </MemoryRouter>
  );
}

beforeEach(() => {
  useCurrentOrg.setState({ org: { id: "org-1", slug: "acme" } as never, role: "member" });
});

afterEach(() => {
  cleanup();
  useCurrentWorkspace.setState({ workspace: null });
});

describe("FactoryGuard", () => {
  it("sends someone the server refuses back to the workspace home", () => {
    useCurrentWorkspace.setState({
      workspace: { id: "ws-1", factory_access: false } as Workspace
    });
    renderAtIde();
    expect(screen.getByText("home")).toBeInTheDocument();
    expect(screen.queryByText("the IDE")).not.toBeInTheDocument();
  });

  it("renders the IDE for someone the server lets in", () => {
    useCurrentWorkspace.setState({ workspace: { id: "ws-1", factory_access: true } as Workspace });
    renderAtIde();
    expect(screen.getByText("the IDE")).toBeInTheDocument();
  });

  it("renders the IDE while the response carries no answer", () => {
    useCurrentWorkspace.setState({ workspace: { id: "ws-1" } as Workspace });
    renderAtIde();
    expect(screen.getByText("the IDE")).toBeInTheDocument();
  });
});

describe("the pages the Factory's guard stands in front of", () => {
  /// `App.tsx` is the one place the routes are written, and a page that manages the
  /// Factory's things but is not wrapped is a page whose every request is refused
  /// for a customer who types its address. A source read, because rendering the
  /// whole router needs a server.
  const app = readFileSync(resolve(__dirname, "../App.tsx"), "utf8");
  const routeElement = (path: string) => {
    const at = app.indexOf(`path='${path}'`);
    expect(at, `no route '${path}' in App.tsx`).toBeGreaterThan(-1);
    // The element runs to the next `<Route`: the route's own props, then its page.
    const next = app.slice(at + 1).search(/<Route[\s>]/);
    return app.slice(at, at + 1 + next);
  };

  it.each([
    ["ide", "IdePage"],
    ["pipelines/:pathb64", "AirwayPage"],
    ["pipelines/:pathb64/runs/:runId", "AirwayPage"],
    ["context-graph", "ContextGraphPage"]
  ])("guards /%s", (path, page) => {
    const element = routeElement(path);
    expect(element).toContain("<FactoryGuard>");
    expect(element).toContain(`<${page}`);
  });

  /// And the guard really does keep the page from running: its children never mount,
  /// so a request they would send is not sent.
  it("never mounts the page for someone the server refuses", async () => {
    const sent: string[] = [];
    apiClient.defaults.adapter = async (config) => {
      sent.push(config.url ?? "");
      return { data: [], status: 200, statusText: "OK", headers: {}, config };
    };
    function Page() {
      useEffect(() => {
        apiClient.get("/ws-1/files");
      }, []);
      return <div>the page</div>;
    }
    useCurrentWorkspace.setState({
      workspace: { id: "ws-1", factory_access: false } as Workspace
    });
    render(
      <MemoryRouter initialEntries={["/acme/workspaces/ws-1/pipelines/x"]}>
        <Routes>
          <Route
            path='/acme/workspaces/ws-1/pipelines/x'
            element={
              <FactoryGuard>
                <Page />
              </FactoryGuard>
            }
          />
          <Route path='/acme/workspaces/ws-1/home' element={<div>home</div>} />
        </Routes>
      </MemoryRouter>
    );
    expect(screen.getByText("home")).toBeInTheDocument();
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(sent).toEqual([]);
  });
});
