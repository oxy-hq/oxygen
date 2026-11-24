// @vitest-environment jsdom
import { cleanup, render, waitFor } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation, useNavigationType } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import type { Workspace } from "@/types/workspace";
import useOauthConnectReturn from "./useOauthConnectReturn";

vi.mock("sonner", () => ({ toast: { success: vi.fn() } }));
vi.mock("@/stores/useCurrentOrg", () => ({
  default: (selector: (s: { org: { slug: string } }) => unknown) =>
    selector({ org: { slug: "acme" } })
}));

const seen: { path: string; search: string; type: string }[] = [];

function Probe() {
  useOauthConnectReturn();
  const { pathname, search } = useLocation();
  seen.push({ path: pathname, search, type: useNavigationType() });
  return null;
}

const landOn = (url: string) =>
  render(
    <MemoryRouter initialEntries={["/acme/home", url]}>
      <Routes>
        <Route path='*' element={<Probe />} />
      </Routes>
    </MemoryRouter>
  );

beforeEach(() => {
  seen.length = 0;
  useCurrentWorkspace.setState({
    workspace: { id: "ws-1", current_user_role: "admin" } as unknown as Workspace
  });
});
afterEach(cleanup);

describe("useOauthConnectReturn", () => {
  it("replaces the return address rather than pushing a second copy of it", async () => {
    // The flag used to be stripped with `history.replaceState`, which bypasses the router,
    // so the router still believed the location carried `?oxy_connected=ok` — and the
    // `open()` that followed was not a same-page navigation, it pushed. History then held
    // the same Connections address twice and the first Back did nothing. Harmless while
    // Connections was a dialog over another page; not once settings is an address.
    landOn("/acme/settings/workspace/connections?oxy_connected=ok");

    await waitFor(() =>
      expect(seen.at(-1)).toMatchObject({ path: "/acme/settings/workspace/connections" })
    );
    expect(seen.at(-1)?.search).toBe("");
    // REPLACE, not PUSH: the entry carrying the flag is gone, not buried under a copy.
    expect(seen.at(-1)?.type).toBe("REPLACE");
  });

  it("drops QuickBooks' realm_id along with the flag", async () => {
    landOn("/acme/settings/workspace/connections?oxy_connected=ok&realm_id=42");
    await waitFor(() => expect(seen.at(-1)?.search).toBe(""));
  });

  it("does nothing at all without the flag", async () => {
    landOn("/acme/settings/workspace/connections");
    await new Promise((r) => setTimeout(r, 20));
    expect(seen.every((s) => s.type !== "REPLACE")).toBe(true);
  });

  it("waits for the role, so the flag survives the first commit", async () => {
    useCurrentWorkspace.setState({ workspace: null });
    landOn("/acme/settings/workspace/connections?oxy_connected=ok");
    await new Promise((r) => setTimeout(r, 20));
    // Still carrying the flag: nothing was consumed while the role was unknown.
    expect(seen.at(-1)?.search).toBe("?oxy_connected=ok");
  });
});
