// @vitest-environment jsdom

import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { TokenOptions } from "@/types/apiToken";
import CreateTokenDialog from "./CreateTokenDialog";
import TokenSecretDialog, { exportSnippet } from "./TokenSecretDialog";

vi.setConfig({ testTimeout: 20000 });

const mutate = vi.fn();
let options: TokenOptions;

// The hooks are the seam: this file is about which body the dialog sends.
vi.mock("@/hooks/api/userTokens/useUserTokenMutations", () => ({
  useCreateUserToken: () => ({ mutate, isPending: false })
}));
vi.mock("@/hooks/api/userTokens/useUserTokens", () => ({
  useTokenOptions: () => ({ data: options, isLoading: false, isError: false, refetch: vi.fn() })
}));

const baseOptions = (): TokenOptions => ({
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
      workspaces: [
        { workspace_id: "w1", name: "Analytics", role: "admin" },
        { workspace_id: "w2", name: "Finance", role: "viewer" }
      ]
    },
    {
      org_id: "o2",
      org_name: "Globex",
      org_slug: "globex",
      role: "member",
      via: "member",
      policy: { max_lifetime_days: 30, allow_all_access_tokens: true },
      workspaces: [{ workspace_id: "w9", name: "Ops", role: "member" }]
    }
  ]
});

afterEach(() => {
  cleanup();
  mutate.mockReset();
});

const open = async (over: Partial<TokenOptions> = {}) => {
  options = { ...baseOptions(), ...over };
  const user = userEvent.setup({ delay: null });
  render(<CreateTokenDialog open onOpenChange={() => {}} onCreated={() => {}} />);
  await user.type(screen.getByTestId("account-token-name"), "laptop");
  return user;
};

const workspace = (name: string) =>
  within(
    screen
      .getAllByTestId("account-token-workspace")
      .find((row) => row.getAttribute("data-workspace-name") === name) as HTMLElement
  );

const sentBody = () => mutate.mock.calls[0][0];

describe("CreateTokenDialog", () => {
  it("defaults to all access for 90 days", async () => {
    const user = await open();
    await user.click(screen.getByTestId("account-token-submit"));
    expect(sentBody()).toEqual({
      name: "laptop",
      all_access: true,
      platform: false,
      partner: false,
      grants: [],
      expires_in_days: 90
    });
  });

  it("can't be submitted without a name", async () => {
    options = baseOptions();
    render(<CreateTokenDialog open onOpenChange={() => {}} onCreated={() => {}} />);
    expect(screen.getByTestId("account-token-submit")).toBeDisabled();
  });

  it("needs at least one workspace once Selected is chosen, then sends it as a grant", async () => {
    const user = await open();
    await user.click(screen.getByTestId("account-token-access-selected"));
    expect(screen.getByTestId("account-token-submit")).toBeDisabled();

    await user.click(workspace("Analytics").getByTestId("account-token-workspace-checkbox"));
    await user.click(screen.getByTestId("account-token-submit"));
    expect(sentBody()).toMatchObject({
      all_access: false,
      grants: [{ kind: "workspace", org_id: "o1", workspace_id: "w1", role_ceiling: "owner" }],
      expires_in_days: 90
    });
  });

  it("says when the caller's own role caps a grant", async () => {
    const user = await open();
    await user.click(screen.getByTestId("account-token-access-selected"));
    await user.click(workspace("Finance").getByTestId("account-token-workspace-checkbox"));
    expect(workspace("Finance").getByText(/capped at Read, your role here/)).toBeInTheDocument();
  });

  it("sends one org-wide grant for 'all workspaces, including future ones'", async () => {
    const user = await open();
    await user.click(screen.getByTestId("account-token-access-selected"));
    await user.click(screen.getAllByTestId("account-token-org-wide-checkbox")[0]);
    await user.click(screen.getByTestId("account-token-submit"));
    expect(sentBody().grants).toEqual([
      { kind: "workspace", org_id: "o1", workspace_id: null, role_ceiling: "owner" }
    ]);
  });

  it("moves the expiry inside an org's max lifetime when the token is narrowed to it", async () => {
    const user = await open();
    await user.click(screen.getByTestId("account-token-access-selected"));
    await user.click(workspace("Ops").getByTestId("account-token-workspace-checkbox"));

    expect(within(screen.getByTestId("account-token-expiry-90")).getByRole("radio")).toBeDisabled();
    expect(
      within(screen.getByTestId("account-token-expiry-never")).getByRole("radio")
    ).toBeDisabled();
    expect(screen.getByTestId("account-token-expiry-outcome")).toHaveTextContent(
      "Globex limits tokens to 30 days."
    );

    await user.click(screen.getByTestId("account-token-submit"));
    expect(sentBody()).toMatchObject({ expires_in_days: 30 });
  });

  it("offers staff and partner access only to someone who holds them", async () => {
    await open();
    expect(screen.queryByTestId("account-token-platform")).not.toBeInTheDocument();
    expect(screen.queryByTestId("account-token-partner")).not.toBeInTheDocument();
    cleanup();

    const user = await open({ can_platform: true, can_partner: true });
    await user.click(screen.getByTestId("account-token-platform"));
    await user.click(screen.getByTestId("account-token-submit"));
    expect(sentBody()).toMatchObject({ platform: true, partner: false });
  });

  it("warns that an org refusing all-access tokens won't accept this one", async () => {
    const base = baseOptions();
    base.orgs[1].policy.allow_all_access_tokens = false;
    await open({ orgs: base.orgs });
    expect(screen.getByTestId("account-token-all-access-caution")).toHaveTextContent(
      "Globex doesn't accept all-access tokens."
    );
  });
});

describe("TokenSecretDialog", () => {
  it("shows the secret once, with an export line, and says it won't be shown again", () => {
    render(
      <TokenSecretDialog
        onDone={() => {}}
        reveal={{
          reason: "created",
          secret: "oxy_pat_SECRET",
          token: { name: "laptop" } as never
        }}
      />
    );
    expect(screen.getByTestId("account-token-secret")).toHaveTextContent("oxy_pat_SECRET");
    expect(screen.getByTestId("account-token-export")).toHaveTextContent(
      "export OXY_TOKEN=oxy_pat_SECRET"
    );
    expect(screen.getByTestId("account-token-secret-dialog")).toHaveTextContent(
      "You won't see this again."
    );
  });

  it("builds the shell line oxyc reads the token from", () => {
    expect(exportSnippet("oxy_pat_x")).toBe("export OXY_TOKEN=oxy_pat_x");
  });
});
