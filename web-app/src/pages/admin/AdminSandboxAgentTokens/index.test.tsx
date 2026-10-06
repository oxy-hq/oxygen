// @vitest-environment jsdom

import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AxiosError, type AxiosResponse } from "axios";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Grant, Token } from "@/types/apiToken";
import type { PlatformCapability } from "@/types/auth";

vi.setConfig({ testTimeout: 20000 });

const useSandboxAgentTokens = vi.fn();
const revoke = vi.fn();
let revoking: { isPending: boolean; variables?: { id: string } } = { isPending: false };

// Who is looking. `operate_platform` is what opens this page; the audit log needs `view_audit`.
const OPERATOR: PlatformCapability[] = ["operate_platform"];
let viewer: { is_owner: boolean; platform_capabilities: PlatformCapability[] } = {
  is_owner: false,
  platform_capabilities: OPERATOR
};
vi.mock("@/hooks/api/users/useCurrentUser", () => ({
  default: () => ({ data: viewer, isPending: false })
}));

// The hooks are the seam: this file is about what the page shows for each answer, and what a
// revoke sends.
vi.mock("@/hooks/api/sandboxAgentTokens/useSandboxAgentTokens", () => ({
  useSandboxAgentTokens: () => useSandboxAgentTokens(),
  useRevokeSandboxAgentToken: () => ({ mutate: revoke, ...revoking })
}));

import AdminSandboxAgentTokens from "./index";

const HOUR = 60 * 60 * 1000;
const inHours = (hours: number) => new Date(Date.now() + hours * HOUR).toISOString();

const grant = (org: string, app: string, over: Partial<Grant> = {}): Grant => ({
  id: `g-${org}-${app}`,
  kind: "app_sandbox",
  org_id: `o-${org}`,
  org_name: org,
  workspace_id: null,
  workspace_name: null,
  role_ceiling: null,
  app_id: `a-${app}`,
  app_name: app,
  org_slug: org,
  app_slug: app,
  revoked_at: null,
  ...over
});

const token = (over: Partial<Token> = {}): Token => ({
  id: "t1",
  name: "refunds task",
  kind: "sandbox_agent",
  display_prefix: "oxy_sbx_Ab3x",
  last_four: "wxyz",
  all_access: false,
  platform: true,
  partner: false,
  grants: [grant("acme", "store-ops")],
  // Half an hour past the hour, so the countdown does not tick over mid-test.
  expires_at: inHours(7.5),
  last_used_at: inHours(-0.2),
  created_at: inHours(-0.5),
  revoked_at: null,
  status: "active",
  source: "oxyc",
  owner: { type: "user", id: "u-ada", label: "ada@oxy.tech" },
  blocked_orgs: [],
  ...over
});

const expired = (over: Partial<Token> = {}): Token =>
  token({
    id: "t-expired",
    name: "old task",
    status: "expired",
    expires_at: inHours(-50),
    ...over
  });

const revoked = (over: Partial<Token> = {}): Token =>
  token({
    id: "t-revoked",
    name: "stopped task",
    status: "revoked",
    revoked_at: inHours(-3.1),
    ...over
  });

const loaded = (tokens: Token[]) => ({
  data: tokens,
  isPending: false,
  isError: false,
  error: null,
  refetch: vi.fn()
});

const failed = (status: number) => {
  const response = { status, data: undefined } as AxiosResponse;
  return {
    data: undefined,
    isPending: false,
    isError: true,
    error: new AxiosError("Request failed", undefined, undefined, undefined, response),
    refetch: vi.fn()
  };
};

const show = (query: unknown) => {
  useSandboxAgentTokens.mockReturnValue(query);
  // Routed at the real path: the heading comes from the admin route map.
  render(
    <MemoryRouter initialEntries={["/admin/sandbox-agent-tokens"]}>
      <AdminSandboxAgentTokens />
    </MemoryRouter>
  );
  return userEvent.setup();
};

const rows = () => screen.getAllByTestId("admin-sandbox-tokens-row");
const rowNamed = (name: string) => {
  const found = rows().find((row) => row.getAttribute("data-token-name") === name);
  if (!found) throw new Error(`no row for ${name}`);
  return found;
};

afterEach(() => {
  cleanup();
  useSandboxAgentTokens.mockReset();
  revoke.mockReset();
  revoking = { isPending: false };
  viewer = { is_owner: false, platform_capabilities: OPERATOR };
});

describe("Admin → Sandbox agent tokens", () => {
  it("is named by the route map, like every admin page", () => {
    show(loaded([token()]));
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Sandbox agent tokens");
  });

  it("lists the tokens in the order the server sent them", () => {
    show(loaded([token(), revoked(), expired()]));
    expect(rows().map((row) => row.getAttribute("data-token-name"))).toEqual([
      "refunds task",
      "stopped task",
      "old task"
    ]);
    expect(rows().map((row) => row.getAttribute("data-token-status"))).toEqual([
      "active",
      "revoked",
      "expired"
    ]);
  });

  it("shows who minted a token, its apps by reference, when it dies and when it was used", () => {
    show(loaded([token({ grants: [grant("acme", "store-ops"), grant("acme", "refunds")] })]));
    const row = within(rowNamed("refunds task"));
    expect(row.getByTestId("admin-sandbox-tokens-name")).toHaveTextContent("refunds task");
    // The prefix is how an audit row names the token.
    expect(row.getByTestId("admin-sandbox-tokens-name")).toHaveAttribute(
      "title",
      "refunds task\noxy_sbx_Ab3x…wxyz"
    );
    expect(row.getByTestId("admin-sandbox-tokens-minter")).toHaveTextContent("ada@oxy.tech");
    const apps = row.getByTestId("admin-sandbox-tokens-apps");
    expect(apps).toHaveTextContent("acme/store-ops");
    expect(apps).toHaveTextContent("acme/refunds");
    expect(row.getByTestId("admin-sandbox-tokens-expiry")).toHaveTextContent(
      "Active, expires in 7 hours"
    );
    expect(row.getByTestId("admin-sandbox-tokens-last-used")).toHaveTextContent("12m ago");
  });

  it("names two apps and counts the rest, the way the account list does", () => {
    const apps = ["a", "b", "c", "d", "e"].map((app) => grant("acme", app));
    show(loaded([token({ grants: apps })]));
    const cell = within(rowNamed("refunds task")).getByTestId("admin-sandbox-tokens-apps");
    expect(cell).toHaveTextContent("acme/a");
    expect(cell).toHaveTextContent("acme/b");
    // The count is always in view; every app is in the cell's hover.
    expect(cell).toHaveTextContent("+3 more");
    expect(cell).not.toHaveTextContent("acme/c");
  });

  it("falls back to an app's name when its grant carries no reference", () => {
    // The server sends the slugs empty when the org or the app is gone.
    show(loaded([token({ grants: [grant("acme", "store-ops", { org_slug: "", app_slug: "" })] })]));
    const cell = within(rowNamed("refunds task")).getByTestId("admin-sandbox-tokens-apps");
    expect(cell).toHaveTextContent("store-ops");
    expect(cell).not.toHaveTextContent("/store-ops");
  });

  it("says a token reaches nothing when every grant it held was taken away", () => {
    const gone = grant("acme", "store-ops", { revoked_at: inHours(-1) });
    show(loaded([token({ grants: [gone] })]));
    expect(
      within(rowNamed("refunds task")).getByTestId("admin-sandbox-tokens-apps")
    ).toHaveTextContent("No access");
  });

  it("says how a token ended, and how long ago", () => {
    show(loaded([revoked(), expired()]));
    expect(
      within(rowNamed("stopped task")).getByTestId("admin-sandbox-tokens-expiry")
    ).toHaveTextContent("Revoked3h ago");
    expect(
      within(rowNamed("old task")).getByTestId("admin-sandbox-tokens-expiry")
    ).toHaveTextContent("Expired2d ago");
    // A token that works has no end to report.
    expect(screen.queryAllByTestId("admin-sandbox-tokens-ended")).toHaveLength(2);
  });

  it("offers Revoke on a token that works, and nothing on one that does not", () => {
    show(loaded([token(), revoked(), expired()]));
    expect(
      within(rowNamed("refunds task")).getByTestId("admin-sandbox-tokens-revoke")
    ).toBeEnabled();
    for (const name of ["stopped task", "old task"]) {
      expect(within(rowNamed(name)).queryByRole("button")).not.toBeInTheDocument();
    }
  });

  it("offers nothing on a token that lapsed after the list was fetched", () => {
    show(loaded([token({ status: "active", expires_at: inHours(-0.1) })]));
    expect(rowNamed("refunds task")).toHaveAttribute("data-token-status", "expired");
    expect(screen.queryByTestId("admin-sandbox-tokens-revoke")).not.toBeInTheDocument();
  });

  it("asks before revoking, naming the token and who minted it", async () => {
    const user = show(loaded([token()]));
    await user.click(screen.getByTestId("admin-sandbox-tokens-revoke"));
    expect(revoke).not.toHaveBeenCalled();

    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveTextContent("Revoke refunds task?");
    expect(dialog).toHaveTextContent("ada@oxy.tech minted it.");

    await user.click(within(dialog).getByTestId("admin-sandbox-tokens-revoke-confirm"));
    expect(revoke).toHaveBeenCalledTimes(1);
    expect(revoke).toHaveBeenCalledWith({ id: "t1", name: "refunds task" });
  });

  it("revokes nothing when the confirmation is cancelled", async () => {
    const user = show(loaded([token()]));
    await user.click(screen.getByTestId("admin-sandbox-tokens-revoke"));
    const dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(revoke).not.toHaveBeenCalled();
  });

  it("holds the one row's Revoke while its request is in flight", () => {
    revoking = { isPending: true, variables: { id: "t1" } };
    show(loaded([token(), token({ id: "t2", name: "other task" })]));
    const busy = within(rowNamed("refunds task")).getByTestId("admin-sandbox-tokens-revoke");
    expect(busy).toBeDisabled();
    expect(busy).toHaveTextContent("Revoking…");
    expect(within(rowNamed("other task")).getByTestId("admin-sandbox-tokens-revoke")).toBeEnabled();
  });

  it("sums the list up: how many are live, for how many people, and how many ended", () => {
    show(
      loaded([
        token(),
        token({ id: "t2", owner: { type: "user", id: "u-lin", label: "lin@oxy.tech" } }),
        revoked()
      ])
    );
    expect(screen.getByTestId("admin-sandbox-tokens-summary")).toHaveTextContent(
      "2 tokens are live, minted by 2 people. 1 more has expired or been revoked."
    );
  });

  it("says no agent holds a token when every listed one has ended", () => {
    show(loaded([revoked(), expired()]));
    expect(screen.getByTestId("admin-sandbox-tokens-summary")).toHaveTextContent(
      "No agent holds a token right now. The 2 below have expired or been revoked."
    );
    // They are still listed: the page is also the record of what was minted.
    expect(rows()).toHaveLength(2);
  });

  it("shows placeholders while the list loads", () => {
    show({ data: undefined, isPending: true, isError: false, error: null });
    expect(screen.getByTestId("admin-async-loading")).toBeInTheDocument();
    expect(screen.queryByTestId("admin-sandbox-tokens-table")).not.toBeInTheDocument();
    expect(screen.queryByTestId("admin-sandbox-tokens-empty")).not.toBeInTheDocument();
  });

  it("says so when no token has been minted", () => {
    show(loaded([]));
    expect(screen.getByTestId("admin-sandbox-tokens-empty")).toHaveTextContent(
      "No agent holds a token right now."
    );
    expect(screen.queryByTestId("admin-sandbox-tokens-table")).not.toBeInTheDocument();
  });

  it("tells a viewer without the capability so, instead of showing a broken table", () => {
    show(failed(403));
    expect(screen.getByTestId("admin-sandbox-tokens-refused")).toHaveTextContent(
      "operate_platform"
    );
    expect(screen.queryByTestId("admin-async-error")).not.toBeInTheDocument();
    expect(screen.queryByTestId("admin-sandbox-tokens-table")).not.toBeInTheDocument();
    expect(screen.queryByTestId("admin-sandbox-tokens-empty")).not.toBeInTheDocument();
  });

  it("reports any other failure as a failure, with a way to try again", async () => {
    const query = failed(500);
    const user = show(query);
    expect(screen.getByTestId("admin-async-error")).toHaveTextContent(
      "Couldn’t load sandbox agent tokens."
    );
    expect(screen.queryByTestId("admin-sandbox-tokens-refused")).not.toBeInTheDocument();
    // Not the empty state: a server that is down has not said "no tokens".
    expect(screen.queryByTestId("admin-sandbox-tokens-empty")).not.toBeInTheDocument();
    await user.click(screen.getByTestId("admin-async-retry"));
    expect(query.refetch).toHaveBeenCalledTimes(1);
  });

  it("says when the list was cut at the server's limit", () => {
    const many = Array.from({ length: 500 }, (_, index) =>
      expired({ id: `t-${index}`, name: `task ${index}` })
    );
    show(loaded(many));
    expect(screen.getByTestId("admin-sandbox-tokens-truncated")).toHaveTextContent(
      "Showing the newest 500."
    );
  });

  it("says nothing about a limit for a list under it", () => {
    show(loaded([token()]));
    expect(screen.queryByTestId("admin-sandbox-tokens-truncated")).not.toBeInTheDocument();
  });

  describe("a token's audit trail", () => {
    it("is one click from its name for a viewer the audit log admits", () => {
      viewer = { is_owner: false, platform_capabilities: [...OPERATOR, "view_audit"] };
      show(loaded([token(), revoked()]));
      const trail = within(rowNamed("refunds task")).getByTestId("admin-sandbox-tokens-trail");
      expect(trail).toHaveTextContent("refunds task");
      expect(trail).toHaveAttribute("href", "/admin/audit?token_id=t1");
      // What an agent did matters after its token ended, so that row links too.
      expect(
        within(rowNamed("stopped task")).getByTestId("admin-sandbox-tokens-trail")
      ).toHaveAttribute("href", "/admin/audit?token_id=t-revoked");
    });

    it("is there for an owner, who holds every capability", () => {
      viewer = { is_owner: true, platform_capabilities: [] };
      show(loaded([token()]));
      expect(screen.getByTestId("admin-sandbox-tokens-trail")).toBeInTheDocument();
    });

    it("is not offered to a viewer the audit log would turn away", () => {
      show(loaded([token()]));
      expect(screen.queryByTestId("admin-sandbox-tokens-trail")).not.toBeInTheDocument();
      // The name is still there, as text.
      expect(screen.getByTestId("admin-sandbox-tokens-name")).toHaveTextContent("refunds task");
      expect(within(rowNamed("refunds task")).queryByRole("link")).not.toBeInTheDocument();
    });
  });
});
