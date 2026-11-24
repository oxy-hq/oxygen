// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import type { Workspace } from "@/types/workspace";
import SettingsWorkspaceGate from "./SettingsWorkspaceGate";

const WS = { id: "ws-1", name: "Acme Internal Analytics" } as unknown as Workspace;

vi.mock("@/components/useResolveOrgForGuard", () => ({
  useResolveOrgForGuard: () => ({ org: { id: "org-1", slug: "acme" }, isPending: false })
}));
vi.mock("@/hooks/api/workspaces/useWorkspaces", () => ({
  useAllWorkspaces: () => ({ data: [WS], isPending: false }),
  useWorkspace: () => ({ data: WS })
}));
vi.mock("@/hooks/useLandingWorkspace", () => ({
  useLandingWorkspace: () => ({ target: WS, isPending: false })
}));
vi.mock("@/hooks/useOauthConnectReturn", () => ({ default: () => {} }));

/** Records what the gate decided, instead of rendering the whole surface. */
const seen: boolean[] = [];
vi.mock("./SettingsPage", () => ({
  default: ({ contextReady }: { contextReady?: boolean }) => {
    seen.push(!!contextReady);
    return <output data-testid='ready'>{String(contextReady)}</output>;
  }
}));

beforeEach(() => {
  seen.length = 0;
  useCurrentWorkspace.setState({ workspace: null });
});
afterEach(cleanup);

describe("SettingsWorkspaceGate", () => {
  it("is not ready until the workspace is in the store, not merely fetched", async () => {
    // The race this exists for: keyed on the query, readiness flipped true on the render the
    // fetch resolved — one commit before the effect writes the store. In that render the nav
    // still sees no workspace, so a Data section is not in it, and because child effects run
    // first the redirect fired before `setWorkspace`. Every cold load of a workspace-gated
    // address was bounced: refresh, a bookmark, a shared link, the OAuth return.
    render(
      <MemoryRouter initialEntries={["/acme/settings/workspace/oltp"]}>
        <Routes>
          <Route path=':orgSlug/settings/*' element={<SettingsWorkspaceGate />} />
        </Routes>
      </MemoryRouter>
    );

    // Whatever it reported first, it was not "ready" before the store was written.
    expect(seen[0]).toBe(false);

    await waitFor(() => expect(screen.getByTestId("ready").textContent).toBe("true"));
    expect(useCurrentWorkspace.getState().workspace?.id).toBe("ws-1");
  });
});
