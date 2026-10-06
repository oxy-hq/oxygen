// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AxiosError, type AxiosResponse } from "axios";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Token, TokenOptions } from "@/types/apiToken";
import TokenRow from ".";

vi.setConfig({ testTimeout: 20000 });

const revoke = vi.fn();
const regenerate = vi.fn();
const update = vi.fn();
const rename = vi.fn();

// The hooks are the seam: this file is about which actions a row offers, and what each sends.
vi.mock("@/hooks/api/userTokens/useUserTokenMutations", () => ({
  useRevokeUserToken: () => ({ mutate: revoke, isPending: false }),
  useRegenerateUserToken: () => ({ mutate: regenerate, isPending: false }),
  useUpdateUserToken: () => ({ mutate: update, isPending: false }),
  useRenameUserToken: () => ({ mutateAsync: rename, isPending: false })
}));

const options: TokenOptions = {
  can_platform: false,
  can_partner: false,
  orgs: [
    {
      org_id: "o1",
      org_name: "Acme",
      org_slug: "acme",
      role: "admin",
      via: "member",
      policy: { max_lifetime_days: null, allow_all_access_tokens: true },
      workspaces: [{ workspace_id: "w1", name: "Analytics", role: "admin" }]
    }
  ]
};
vi.mock("@/hooks/api/userTokens/useUserTokens", () => ({
  useTokenOptions: () => ({ data: options, isLoading: false, isError: false, refetch: vi.fn() })
}));
vi.mock("@/hooks/api/apiKeys/useApiKeyMutations", () => ({
  useExtendApiKey: () => ({ mutate: vi.fn(), isPending: false })
}));
vi.mock("@/hooks/api/apiKeys/useApiKeyActivity", () => ({
  API_KEY_ACTIVITY_LIMIT: 100,
  default: () => ({ isLoading: true, error: null, refetch: vi.fn() })
}));

afterEach(() => {
  cleanup();
  revoke.mockReset();
  regenerate.mockReset();
  update.mockReset();
  rename.mockReset();
});

const token = (over: Partial<Token> = {}): Token => ({
  id: "t1",
  name: "laptop",
  kind: "personal",
  display_prefix: "oxy_pat_Ab3x",
  last_four: "wxyz",
  all_access: false,
  platform: false,
  partner: false,
  grants: [
    {
      id: "g1",
      kind: "workspace",
      org_id: "o1",
      org_name: "Acme",
      workspace_id: "w1",
      workspace_name: "Analytics",
      role_ceiling: "viewer",
      app_id: null,
      app_name: null,
      revoked_at: null
    }
  ],
  expires_at: new Date(Date.now() + 30 * 24 * 60 * 60 * 1000).toISOString(),
  last_used_at: null,
  created_at: "2026-10-01T00:00:00Z",
  revoked_at: null,
  status: "active",
  source: "ui",
  owner: { type: "user", id: "u1", label: "me@example.com" },
  blocked_orgs: [],
  ...over
});

const onRegenerated = vi.fn();

const show = (value: Token) => {
  const user = userEvent.setup({ delay: null });
  render(
    <table>
      <tbody>
        <TokenRow token={value} onRegenerated={onRegenerated} />
      </tbody>
    </table>
  );
  return user;
};

const openMenu = async (user: ReturnType<typeof userEvent.setup>) =>
  user.click(screen.getByTestId("account-token-menu-button"));

describe("TokenRow", () => {
  it("shows the masked token, its access and when it was last used", () => {
    show(token({ platform: true }));
    expect(screen.getByTestId("account-token-masked")).toHaveTextContent("oxy_pat_Ab3x…wxyz");
    expect(screen.getByTestId("account-token-access-label")).toHaveTextContent(
      "1 workspace in 1 org"
    );
    // Standing reads on in the same line, in words.
    expect(screen.getByTestId("account-token-standing-platform")).toHaveTextContent("staff");
    expect(screen.getByTestId("account-token-access")).toHaveTextContent(
      "1 workspace in 1 org, with staff standing"
    );
    expect(screen.getByTestId("account-token-kind")).toHaveTextContent("Personal");
    expect(screen.getByTestId("account-token-row")).toHaveTextContent("Never");
  });

  it("names both standings when a token carries both", () => {
    show(token({ all_access: true, platform: true, partner: true }));
    expect(screen.getByTestId("account-token-access")).toHaveTextContent(
      "All access, with staff and partner standing"
    );
  });

  it("carries the token's prefix as the name cell's title, under the name in full", () => {
    // Where the table's box is too narrow for the Token column, this is where the prefix is.
    show(token({ name: "a name far too long for the width of its column" }));
    const cell = screen.getByTestId("account-token-name-cell");
    expect(cell).toHaveAttribute(
      "title",
      "a name far too long for the width of its column\noxy_pat_Ab3x…wxyz"
    );
    expect(cell).toContainElement(screen.getByTestId("account-token-name-text"));
    cleanup();

    show(token({ kind: "sandbox_agent", display_prefix: "oxy_sbx_Qr7k", name: "refunds task" }));
    expect(screen.getByTestId("account-token-name-cell").title).toContain("oxy_sbx_Qr7k…wxyz");
  });

  it("puts the whole of the access in its one tooltip, for a column that cuts it short", async () => {
    show(token({ all_access: true, platform: true, partner: true }));
    // One hover text: a native title beside the tooltip would show two at once.
    expect(screen.getByTestId("account-token-access")).not.toHaveAttribute("title");
    await userEvent.hover(screen.getByTestId("account-token-access-label"));
    const whole = await screen.findAllByText(
      "All access, with staff and partner standing",
      {},
      { timeout: 5000 }
    );
    expect(whole.length).toBeGreaterThan(0);
    cleanup();

    show(token());
    expect(screen.getByTestId("account-token-access")).not.toHaveAttribute("title");
    await userEvent.hover(screen.getByTestId("account-token-access-label"));
    const summary = await screen.findAllByText("1 workspace in 1 org", {}, { timeout: 5000 });
    // The label in the cell, and the same words leading the tooltip.
    expect(summary.length).toBeGreaterThan(1);
  });

  it("says when it was last used: a word, then the day alone with the hour in a title", () => {
    const DAY = 24 * 60 * 60 * 1000;
    const used = () => screen.getByTestId("account-token-row").querySelector("td:nth-child(6)");
    show(token());
    expect(used()).toHaveTextContent(/^Never$/);
    expect(used()).not.toHaveAttribute("title");
    cleanup();

    show(token({ last_used_at: new Date(Date.now() - 3 * DAY).toISOString() }));
    expect(used()).toHaveTextContent(/^3 days ago$/);
    cleanup();

    show(token({ last_used_at: "2026-03-04T15:30:00Z" }));
    // No hour in the cell. The title has the moment in full.
    expect(used()).toHaveTextContent(/^Mar \d, 2026$/);
    expect(used()?.getAttribute("title")).toMatch(/^Mar \d, 2026, \d{2}:\d{2} [AP]M$/);
  });

  it("says when it dies as a short phrase: a countdown, no expiry, expired or revoked", () => {
    const HOUR = 60 * 60 * 1000;
    const status = () => screen.getByTestId("account-token-row").querySelector("td:nth-child(5)");
    show(token({ expires_at: new Date(Date.now() + 7.5 * HOUR).toISOString() }));
    expect(status()).toHaveTextContent(/^Active, expires in 7 hours$/);
    cleanup();
    show(token({ expires_at: null }));
    expect(status()).toHaveTextContent(/^Active, No expiry$/);
    cleanup();
    show(token({ status: "expired", expires_at: new Date(Date.now() - HOUR).toISOString() }));
    expect(status()).toHaveTextContent(/^Expired$/);
    cleanup();
    show(token({ status: "revoked", revoked_at: "2026-10-02T00:00:00Z" }));
    expect(status()).toHaveTextContent(/^Revoked$/);
  });

  it("warns when an org's policy blocks the token", () => {
    show(token({ blocked_orgs: [{ org_id: "o2", org_name: "Globex", reason: "max_lifetime" }] }));
    expect(screen.getByTestId("account-token-blocked-chip")).toHaveTextContent("Blocked in 1 org");
  });

  it("offers Extend, Activity, Edit access, Regenerate and Revoke on a personal token", async () => {
    const user = show(token());
    expect(screen.getByTestId("account-token-activity-button")).toBeInTheDocument();
    expect(screen.getByTestId("api-key-extend-button")).toBeInTheDocument();
    // The three a row is read for are words in the row. The rest are behind the menu.
    for (const name of ["Extend laptop", "Activity for laptop", "Revoke laptop"]) {
      expect(screen.getByRole("button", { name })).toHaveTextContent(/^(Extend|Activity|Revoke)$/);
    }
    expect(screen.getByTestId("account-token-revoke")).toHaveTextContent("Revoke");
    await openMenu(user);
    expect(screen.getByTestId("account-token-edit-access")).toBeInTheDocument();
    expect(screen.getByTestId("account-token-regenerate")).toBeInTheDocument();
  });

  it("offers Extend on a lapsed personal token, since extending is how it comes back", () => {
    show(token({ status: "expired", expires_at: "2026-01-01T00:00:00Z" }));
    expect(screen.getByTestId("api-key-expired-extend-button")).toHaveTextContent("Extend");
    expect(screen.queryByTestId("api-key-extend-button")).not.toBeInTheDocument();
    expect(screen.getByTestId("account-token-revoke")).toBeInTheDocument();
  });

  it("offers no Extend on a token that never expires", () => {
    show(token({ expires_at: null }));
    expect(screen.queryByTestId("api-key-extend-button")).not.toBeInTheDocument();
    expect(screen.queryByTestId("api-key-expired-extend-button")).not.toBeInTheDocument();
  });

  it("offers nothing to change on a revoked token", () => {
    show(token({ status: "revoked", revoked_at: "2026-10-02T00:00:00Z" }));
    expect(screen.getByTestId("account-token-row")).toHaveTextContent("Revoked");
    expect(screen.queryByTestId("account-token-menu-button")).not.toBeInTheDocument();
    expect(screen.queryByTestId("account-token-revoke")).not.toBeInTheDocument();
    expect(screen.queryByTestId("account-token-rename-button")).not.toBeInTheDocument();
    expect(screen.queryByTestId("api-key-extend-button")).not.toBeInTheDocument();
    // What it did is still worth reading.
    expect(screen.getByTestId("account-token-activity-button")).toBeInTheDocument();
  });

  it("asks before revoking", async () => {
    const user = show(token());
    await user.click(screen.getByTestId("account-token-revoke"));
    expect(revoke).not.toHaveBeenCalled();
    await user.click(await screen.findByTestId("account-token-revoke-confirm"));
    expect(revoke).toHaveBeenCalledWith({ id: "t1", name: "laptop" });
  });

  it("asks before regenerating, then hands the new secret up to be shown once", async () => {
    const user = show(token());
    await openMenu(user);
    await user.click(screen.getByTestId("account-token-regenerate"));
    expect(regenerate).not.toHaveBeenCalled();
    await user.click(await screen.findByTestId("account-token-regenerate-confirm"));
    expect(regenerate).toHaveBeenCalledWith("t1", { onSuccess: onRegenerated });
  });

  it("renames in place, sending the trimmed name and nothing else", async () => {
    rename.mockResolvedValue(token({ name: "work laptop" }));
    const user = show(token());
    await user.click(screen.getByTestId("account-token-rename-button"));
    const input = screen.getByTestId("account-token-rename-input");
    expect(input).toHaveValue("laptop");
    await user.clear(input);
    await user.type(input, "  work laptop {Enter}");
    expect(rename).toHaveBeenCalledWith({ id: "t1", name: "work laptop" });
    // Saved: the box gives way to the name again.
    expect(await screen.findByTestId("account-token-rename-button")).toBeInTheDocument();
  });

  it("sends nothing for an unchanged or empty name, and Cancel leaves it alone", async () => {
    const user = show(token());
    await user.click(screen.getByTestId("account-token-rename-button"));
    await user.clear(screen.getByTestId("account-token-rename-input"));
    expect(screen.getByTestId("account-token-rename-save")).toBeDisabled();
    await user.type(screen.getByTestId("account-token-rename-input"), "laptop{Enter}");
    expect(screen.queryByTestId("account-token-rename-input")).not.toBeInTheDocument();

    await user.click(screen.getByTestId("account-token-rename-button"));
    await user.type(screen.getByTestId("account-token-rename-input"), " two");
    await user.click(screen.getByTestId("account-token-rename-cancel"));
    expect(screen.getByTestId("account-token-name-text")).toHaveTextContent("laptop");
    expect(rename).not.toHaveBeenCalled();
  });

  it("keeps the box open and says why when the rename is refused", async () => {
    rename.mockRejectedValue(
      new AxiosError("Request failed", "ERR", undefined, undefined, {
        status: 409,
        data: { error: "the token is revoked", code: "revoked" }
      } as AxiosResponse)
    );
    const user = show(token());
    await user.click(screen.getByTestId("account-token-rename-button"));
    await user.type(screen.getByTestId("account-token-rename-input"), " two{Enter}");
    expect(await screen.findByTestId("account-token-rename-error")).toHaveTextContent(
      "This token was revoked, so it can't be changed."
    );
    expect(screen.getByTestId("account-token-rename-input")).toHaveValue("laptop two");
  });

  describe("for a sandbox agent token", () => {
    const HOUR = 60 * 60 * 1000;
    const [workspaceGrant] = token().grants;
    // `slugs` is what the server sends beside the names: `{}` is an older server, which sends none.
    const appGrant = (
      id: string,
      appName: string,
      slugs: { org_slug?: string; app_slug?: string } = {}
    ) => ({
      ...workspaceGrant,
      id,
      kind: "app_sandbox" as const,
      workspace_id: null,
      workspace_name: null,
      role_ceiling: null,
      app_id: id,
      app_name: appName,
      ...slugs
    });
    const acme = (appSlug: string) => ({ org_slug: "acme", app_slug: appSlug });
    const sandbox = (over: Partial<Token> = {}): Token =>
      token({
        name: "refunds task",
        kind: "sandbox_agent",
        display_prefix: "oxy_sbx_Qr7k",
        // How the kind is stored. It is not standing the token carries.
        platform: true,
        grants: [appGrant("a1", "Store Ops"), appGrant("a2", "Refunds")],
        // Half an hour past the seventh, so the count below can't tip over while the test runs.
        expires_at: new Date(Date.now() + 7.5 * HOUR).toISOString(),
        ...over
      });

    it("shows what it is, its apps and how long it has left", () => {
      show(sandbox());
      const row = screen.getByTestId("account-token-row");
      expect(row).toHaveAttribute("data-token-kind", "sandbox_agent");
      expect(screen.getByTestId("account-token-kind-badge")).toHaveTextContent("Sandbox agent");
      expect(screen.getByTestId("account-token-masked")).toHaveTextContent("oxy_sbx_Qr7k…wxyz");
      // An older server sends a grant no slug, so its apps keep their names.
      expect(screen.getByTestId("account-token-access-label")).toHaveTextContent(
        "Sandboxes of Store Ops, Refunds"
      );
      expect(screen.getByTestId("api-key-expiry-countdown")).toHaveTextContent("in 7 hours");
    });

    it("names its apps by the reference each grant carries, and by name where it carries none", async () => {
      show(
        sandbox({
          grants: [appGrant("a1", "Store Ops", acme("store-ops")), appGrant("a2", "Refunds")]
        })
      );
      const access = screen.getByTestId("account-token-access-label");
      expect(access).toHaveTextContent("Sandboxes of acme/store-ops, Refunds");
      // The row stays one line: past two apps, the rest are a count.
      cleanup();
      show(
        sandbox({
          grants: [
            appGrant("a1", "App a1", acme("store-ops")),
            appGrant("a2", "App a2", acme("refunds")),
            appGrant("a3", "App a3"),
            appGrant("a4", "App a4")
          ]
        })
      );
      expect(screen.getByTestId("account-token-access-label")).toHaveTextContent(
        "Sandboxes of acme/store-ops, acme/refunds, +2 more"
      );
      // The tooltip names every app: the two shown, and the two the count stands for.
      await userEvent.hover(screen.getByTestId("account-token-access-label"));
      const whole = await screen.findAllByText(
        "Sandboxes of acme/store-ops, acme/refunds, App a3, App a4",
        {},
        { timeout: 5000 }
      );
      expect(whole.length).toBeGreaterThan(0);
    });

    it("falls back to the app's name when either half of the reference is empty", () => {
      // The server sends a slug empty, not absent, for an org or an app that is gone.
      show(
        sandbox({
          grants: [
            appGrant("a1", "Store Ops", { org_slug: "acme", app_slug: "" }),
            appGrant("a2", "Refunds", { org_slug: "", app_slug: "refunds" })
          ]
        })
      );
      expect(screen.getByTestId("account-token-access-label")).toHaveTextContent(
        "Sandboxes of Store Ops, Refunds"
      );
    });

    it("never reads its apps as every workspace, or its storage flag as staff access", () => {
      show(sandbox());
      expect(screen.queryByTestId("account-token-standing-platform")).not.toBeInTheDocument();
      expect(screen.getByTestId("account-token-access")).not.toHaveTextContent(/workspace/i);
    });

    it("offers Activity and Revoke, and no Rename, Extend, Edit access or Regenerate", async () => {
      const user = show(sandbox());
      expect(screen.getByTestId("account-token-activity-button")).toBeInTheDocument();
      expect(screen.queryByTestId("account-token-rename-button")).not.toBeInTheDocument();
      expect(screen.queryByTestId("api-key-extend-button")).not.toBeInTheDocument();

      // Revoke is in the row. There is no menu, since nothing else can be done to it.
      expect(screen.getByTestId("account-token-revoke")).toHaveTextContent("Revoke");
      expect(screen.queryByTestId("account-token-menu-button")).not.toBeInTheDocument();
      expect(screen.queryByTestId("account-token-edit-access")).not.toBeInTheDocument();
      expect(screen.queryByTestId("account-token-regenerate")).not.toBeInTheDocument();
      await user.click(screen.getByTestId("account-token-activity-button"));
    });

    it("revokes after asking, like any token", async () => {
      const user = show(sandbox());
      await user.click(screen.getByTestId("account-token-revoke"));
      await user.click(await screen.findByTestId("account-token-revoke-confirm"));
      expect(revoke).toHaveBeenCalledWith({ id: "t1", name: "refunds task" });
    });

    it("shows Expired once it lapses, with no Extend to bring it back", () => {
      show(sandbox({ status: "expired", expires_at: new Date(Date.now() - HOUR).toISOString() }));
      expect(screen.getByTestId("account-token-row")).toHaveTextContent("Expired");
      expect(screen.queryByTestId("api-key-expired-extend-button")).not.toBeInTheDocument();
      expect(screen.queryByTestId("api-key-extend-button")).not.toBeInTheDocument();
    });
  });

  it("locks a workspace its org removed, instead of offering a tick that would do nothing", async () => {
    const [held] = token().grants;
    const user = show(token({ grants: [{ ...held, revoked_at: "2026-10-02T00:00:00Z" }] }));
    await openMenu(user);
    await user.click(screen.getByTestId("account-token-edit-access"));

    const checkbox = await screen.findByTestId("account-token-workspace-checkbox");
    expect(checkbox).toBeDisabled();
    expect(checkbox).toHaveAttribute("data-state", "unchecked");
    expect(screen.getByTestId("account-token-grant-removed")).toHaveTextContent("removed by Acme");
    // Nothing else is picked, so there is nothing to save.
    expect(screen.getByTestId("account-token-edit-submit")).toBeDisabled();
  });

  it("edits access with PATCH, replacing the grants", async () => {
    const user = show(token());
    await openMenu(user);
    await user.click(screen.getByTestId("account-token-edit-access"));

    // Opens on the token's current grant: Analytics, ticked.
    const checkbox = await screen.findByTestId("account-token-workspace-checkbox");
    expect(checkbox).toHaveAttribute("data-state", "checked");

    // Nothing selected is not saveable; all access is.
    await user.click(checkbox);
    expect(screen.getByTestId("account-token-edit-submit")).toBeDisabled();
    await user.click(screen.getByTestId("account-token-access-all"));
    await user.click(screen.getByTestId("account-token-edit-submit"));

    expect(update.mock.calls[0][0]).toEqual({
      id: "t1",
      request: { all_access: true, platform: false, partner: false, grants: [] }
    });
  });
});
