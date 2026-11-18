// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { EditSecretDialog } from "@/components/settings/secrets/SecretTable/Row/EditSecretDialog";
import { ServiceAccountService } from "@/services/api/orgApiAccess";
import useCurrentOrg from "@/stores/useCurrentOrg";
import type { Token } from "@/types/apiToken";
import type { ServiceAccount } from "@/types/orgApiAccess";
import type { Organization, OrgRole } from "@/types/organization";
import type { Secret } from "@/types/secret";

vi.mock("@/contexts/AuthContext", () => ({ useAuth: () => ({ isLocalMode: false }) }));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
vi.mock("@/services/api/orgApiAccess", () => ({
  ServiceAccountService: {
    create: vi.fn(),
    createToken: vi.fn(),
    revokeToken: vi.fn(),
    remove: vi.fn()
  }
}));
const lists = vi.hoisted(() => ({ accounts: [] as ServiceAccount[] }));
vi.mock("@/hooks/api/orgApiAccess", () => ({
  useServiceAccounts: () => ({ data: lists.accounts }),
  useTokenPolicy: () => ({
    data: {
      max_lifetime_days: null,
      allow_all_access_tokens: true,
      require_environment_on_trust_policies: false
    }
  })
}));
const update = vi.hoisted(() => vi.fn());
vi.mock("@/hooks/api/secrets/useSecretMutations", () => ({
  useUpdateSecret: () => ({ mutateAsync: update, isPending: false })
}));
vi.mock("@/hooks/api/secrets/useSharedSecrets", () => ({
  useSharedSecrets: () => ({
    data: {
      shared: [],
      apps: [{ app_id: "app-1", app_name: "Store Ops", app_slug: "store-ops", missing: [] }]
    }
  })
}));

const create = vi.mocked(ServiceAccountService.create);
const createToken = vi.mocked(ServiceAccountService.createToken);
const revokeToken = vi.mocked(ServiceAccountService.revokeToken);
const remove = vi.mocked(ServiceAccountService.remove);

/** The INC-180 secret: Store Ops' own `TABLETS_API_KEY`, which must hold an Oxygen token. */
const secret: Secret = {
  id: "sec-1",
  name: "apps/app-1/TABLETS_API_KEY",
  created_at: "2026-11-01T00:00:00Z",
  updated_at: "2026-11-01T00:00:00Z",
  created_by: "u-1",
  is_active: true
};

const signIn = (role: OrgRole) =>
  useCurrentOrg.setState({
    org: { id: "org-1", name: "Poke House", slug: "poke-house", role } as Organization,
    role
  });

const show = () => {
  const onOpenChange = vi.fn();
  const onSecretUpdated = vi.fn();
  const user = userEvent.setup({ delay: null, pointerEventsCheck: 0 });
  render(
    <QueryClientProvider client={new QueryClient()}>
      <EditSecretDialog
        open
        onOpenChange={onOpenChange}
        secret={secret}
        onSecretUpdated={onSecretUpdated}
      />
    </QueryClientProvider>
  );
  return { user, onOpenChange, onSecretUpdated };
};

beforeEach(() => {
  lists.accounts = [];
  create.mockResolvedValue({
    id: "sa-1",
    org_id: "org-1",
    name: "store-ops",
    description: null,
    org_role: "member",
    created_by: null,
    created_at: "2026-11-18T00:00:00Z",
    disabled_at: null,
    token_count: 0,
    trust_policy_count: 0
  });
  createToken.mockResolvedValue({
    token: { id: "tok-1", expires_at: "2027-02-16T00:00:00Z" } as Token,
    secret: "oxy_sat_minted"
  });
  revokeToken.mockResolvedValue(undefined);
  remove.mockResolvedValue(undefined);
  update.mockResolvedValue({});
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  useCurrentOrg.setState({ org: null, role: null });
});

describe("Create API token on a secret", () => {
  it("creates a token for the app's own account and saves it as the value, never shown", async () => {
    signIn("admin");
    const { user, onSecretUpdated } = show();

    await user.click(screen.getByTestId("secret-create-api-token"));
    // Suggested after the app, the way the Store Ops fix named it.
    expect(screen.getByTestId("secret-api-token-account-name")).toHaveValue("store-ops");
    await user.click(screen.getByTestId("secret-api-token-submit"));

    expect(create).toHaveBeenCalledWith("org-1", { name: "store-ops", org_role: "member" });
    expect(createToken).toHaveBeenCalledWith("org-1", "sa-1", {
      name: "Secret TABLETS_API_KEY of Store Ops",
      expires_in_days: 90
    });
    // The value field gives way to a note; the token itself is on no screen.
    expect(await screen.findByTestId("secret-minted-token")).toHaveTextContent("store-ops");
    expect(screen.queryByDisplayValue("oxy_sat_minted")).not.toBeInTheDocument();
    expect(document.body).not.toHaveTextContent("oxy_sat_minted");

    await user.click(screen.getByRole("button", { name: "Update Secret" }));
    await waitFor(() =>
      expect(update).toHaveBeenCalledWith({
        id: "sec-1",
        request: { value: "oxy_sat_minted", description: undefined }
      })
    );
    expect(onSecretUpdated).toHaveBeenCalled();
    // Saved, so it is the secret's now: nothing is taken back.
    expect(revokeToken).not.toHaveBeenCalled();
  });

  it("reuses an account already named after the app instead of making a second one", async () => {
    signIn("owner");
    lists.accounts = [
      {
        id: "sa-9",
        org_id: "org-1",
        name: "store-ops",
        description: null,
        org_role: "admin",
        created_by: null,
        created_at: "2026-11-18T00:00:00Z",
        disabled_at: null,
        token_count: 1,
        trust_policy_count: 0
      }
    ];
    const { user } = show();
    await user.click(screen.getByTestId("secret-create-api-token"));
    expect(screen.queryByTestId("secret-api-token-account-name")).not.toBeInTheDocument();
    await user.click(screen.getByTestId("secret-api-token-submit"));
    expect(create).not.toHaveBeenCalled();
    expect(createToken).toHaveBeenCalledWith("org-1", "sa-9", expect.any(Object));
  });

  it("revokes the token, and the account made for it, when the dialog closes unsaved", async () => {
    signIn("admin");
    const { user, onOpenChange } = show();
    await user.click(screen.getByTestId("secret-create-api-token"));
    await user.click(screen.getByTestId("secret-api-token-submit"));
    await screen.findByTestId("secret-minted-token");

    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
    await waitFor(() => expect(revokeToken).toHaveBeenCalledWith("org-1", "sa-1", "tok-1"));
    await waitFor(() => expect(remove).toHaveBeenCalledWith("org-1", "sa-1"));
    expect(update).not.toHaveBeenCalled();
  });

  it("revokes a token that arrives after the dialog closed, and never offers it again", async () => {
    signIn("admin");
    // The mint is still in flight when the person gives up on the dialog.
    let finish: (value: Awaited<ReturnType<typeof createToken>>) => void = () => {};
    createToken.mockReturnValue(
      new Promise((resolve) => {
        finish = resolve;
      })
    );
    const { user, onOpenChange } = show();
    await user.click(screen.getByTestId("secret-create-api-token"));
    await user.click(screen.getByTestId("secret-api-token-submit"));
    // The dialog's own Cancel, the last one: the token form above has a Cancel too.
    const cancels = screen.getAllByRole("button", { name: "Cancel" });
    await user.click(cancels[cancels.length - 1]);
    expect(onOpenChange).toHaveBeenCalledWith(false);

    finish({
      token: { id: "tok-late", expires_at: null } as Token,
      secret: "oxy_sat_late"
    });
    await waitFor(() => expect(revokeToken).toHaveBeenCalledWith("org-1", "sa-1", "tok-late"));
    await waitFor(() => expect(remove).toHaveBeenCalledWith("org-1", "sa-1"));
    // Not kept for the next secret this dialog opens on.
    expect(screen.queryByTestId("secret-minted-token")).not.toBeInTheDocument();
    expect(update).not.toHaveBeenCalled();
  });

  it("revokes an unsaved token when the dialog unmounts", async () => {
    signIn("admin");
    const { user } = show();
    await user.click(screen.getByTestId("secret-create-api-token"));
    await user.click(screen.getByTestId("secret-api-token-submit"));
    await screen.findByTestId("secret-minted-token");

    cleanup();
    await waitFor(() => expect(revokeToken).toHaveBeenCalledWith("org-1", "sa-1", "tok-1"));
  });

  it("takes a token back with Undo and offers the value field again", async () => {
    signIn("admin");
    const { user } = show();
    await user.click(screen.getByTestId("secret-create-api-token"));
    await user.click(screen.getByTestId("secret-api-token-submit"));
    await user.click(await screen.findByTestId("secret-minted-token-undo"));

    await waitFor(() => expect(revokeToken).toHaveBeenCalledWith("org-1", "sa-1", "tok-1"));
    expect(screen.queryByTestId("secret-minted-token")).not.toBeInTheDocument();
    expect(screen.getByLabelText("New Value *")).toBeInTheDocument();
  });

  it("tells someone who is not an org admin who can, instead of a button the server refuses", () => {
    signIn("member");
    show();
    expect(screen.queryByTestId("secret-create-api-token")).not.toBeInTheDocument();
    expect(screen.getByTestId("secret-api-token-admin-only")).toHaveTextContent(
      "An organization admin can create one here"
    );
  });
});
