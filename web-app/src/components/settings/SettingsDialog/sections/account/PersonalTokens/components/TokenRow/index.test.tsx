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
    expect(screen.getByTestId("account-token-standing-platform")).toHaveTextContent("Staff");
    expect(screen.getByTestId("account-token-row")).toHaveTextContent("Never");
  });

  it("warns when an org's policy blocks the token", () => {
    show(token({ blocked_orgs: [{ org_id: "o2", org_name: "Globex", reason: "max_lifetime" }] }));
    expect(screen.getByTestId("account-token-blocked-chip")).toHaveTextContent("Blocked in 1 org");
  });

  it("offers Extend, Activity, Edit access, Regenerate and Revoke on a personal token", async () => {
    const user = show(token());
    expect(screen.getByTestId("account-token-activity-button")).toBeInTheDocument();
    expect(screen.getByTestId("api-key-extend-button")).toBeInTheDocument();
    await openMenu(user);
    expect(screen.getByTestId("account-token-edit-access")).toBeInTheDocument();
    expect(screen.getByTestId("account-token-regenerate")).toBeInTheDocument();
    expect(screen.getByTestId("account-token-revoke")).toBeInTheDocument();
  });

  it("offers nothing to change on a revoked token", () => {
    show(token({ status: "revoked", revoked_at: "2026-10-02T00:00:00Z" }));
    expect(screen.getByTestId("account-token-row")).toHaveTextContent("Revoked");
    expect(screen.queryByTestId("account-token-menu-button")).not.toBeInTheDocument();
    expect(screen.queryByTestId("account-token-rename-button")).not.toBeInTheDocument();
    expect(screen.queryByTestId("api-key-extend-button")).not.toBeInTheDocument();
    // What it did is still worth reading.
    expect(screen.getByTestId("account-token-activity-button")).toBeInTheDocument();
  });

  it("asks before revoking", async () => {
    const user = show(token());
    await openMenu(user);
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
