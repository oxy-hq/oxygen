// @vitest-environment jsdom

import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Grant, Token } from "@/types/apiToken";
import type { ServiceAccount, TrustPolicy } from "@/types/orgApiAccess";
import type { Organization } from "@/types/organization";
import { ServiceAccountDetail } from ".";

vi.setConfig({ testTimeout: 30000 });

const loaded = <T,>(data: T) => ({
  data,
  isPending: false,
  isLoading: false,
  isError: false,
  error: null,
  refetch: vi.fn()
});

// The hooks are the seam: this file is about which request each row action
// sends, and what its confirmation says first.
const calls = {
  updateAccount: vi.fn(),
  deleteAccount: vi.fn(),
  extendToken: vi.fn(),
  regenerateToken: vi.fn(),
  revokeToken: vi.fn(),
  updatePolicy: vi.fn(),
  deletePolicy: vi.fn()
};
const mutation = (fn: ReturnType<typeof vi.fn>) => ({ mutateAsync: fn, isPending: false });
let tokens: Token[] = [];
let policies: TrustPolicy[] = [];

vi.mock("@/hooks/api/orgApiAccess", () => ({
  // Tagged with what it was asked for, so a test can see which account's routes Extend uses.
  useServiceAccountTokenEndpoints: (orgId: string, saId: string) => ({ orgId, saId }),
  useCreateServiceAccount: () => mutation(vi.fn()),
  useUpdateServiceAccount: () => mutation(calls.updateAccount),
  useDeleteServiceAccount: () => mutation(calls.deleteAccount),
  useServiceAccountTokens: () => loaded(tokens),
  useCreateServiceAccountToken: () => mutation(vi.fn()),
  useRegenerateServiceAccountToken: () => mutation(calls.regenerateToken),
  useRevokeServiceAccountToken: () => mutation(calls.revokeToken),
  useTrustPolicies: () => loaded(policies),
  useCreateTrustPolicy: () => mutation(vi.fn()),
  useUpdateTrustPolicy: () => mutation(calls.updatePolicy),
  useDeleteTrustPolicy: () => mutation(calls.deletePolicy),
  useTokenPolicy: () =>
    loaded({
      max_lifetime_days: null,
      allow_all_access_tokens: true,
      require_environment_on_trust_policies: true
    })
}));
// Extend and Activity are the shared token components; their hooks are mocked the way the
// personal-tokens row mocks them, so this file stays about what each action sends.
vi.mock("@/hooks/api/apiKeys/useApiKeyMutations", () => ({
  useExtendApiKey: (endpoints: unknown) => ({
    mutate: (vars: { token: { id: string }; request: unknown }) =>
      calls.extendToken({ endpoints, tokenId: vars.token.id, request: vars.request }),
    isPending: false
  })
}));
vi.mock("@/hooks/api/apiKeys/useApiKeyActivity", () => ({
  API_KEY_ACTIVITY_LIMIT: 100,
  default: () => ({ isLoading: true, error: null, refetch: vi.fn() })
}));
vi.mock("@/hooks/api/workspaces/useWorkspaces", () => ({
  useAllWorkspaces: () => loaded([{ id: "ws-1", name: "Analytics" }])
}));
vi.mock("@/hooks/api/appAccess", () => ({
  useOrgAppAccessList: () => loaded([{ id: "app-1", name: "Store Ops" }])
}));
vi.mock("@/components/Echarts/EChart", () => ({ default: () => <div /> }));

const org = { id: "org-1", name: "Acme", slug: "acme", role: "owner" } as Organization;
const inDays = (n: number) => new Date(Date.now() + n * 24 * 60 * 60 * 1000).toISOString();

const account = (over: Partial<ServiceAccount> = {}): ServiceAccount => ({
  id: "sa-1",
  org_id: "org-1",
  name: "deployer",
  description: "Ships the storefront",
  org_role: "member",
  created_by: { id: "u-1", label: "Ada Lovelace" },
  created_at: "2026-09-01T00:00:00Z",
  disabled_at: null,
  token_count: 3,
  trust_policy_count: 1,
  ...over
});

const grant: Grant = {
  id: "g-1",
  kind: "workspace",
  org_id: "org-1",
  org_name: "Acme",
  workspace_id: "ws-1",
  workspace_name: "Analytics",
  role_ceiling: "member",
  app_id: null,
  app_name: null,
  revoked_at: null
};

const token = (over: Partial<Token> = {}): Token => ({
  id: "t-1",
  name: "nightly sync",
  kind: "service_account",
  display_prefix: "oxy_sat_ab12",
  last_four: "9f3c",
  all_access: false,
  platform: false,
  partner: false,
  grants: [grant],
  expires_at: inDays(30),
  last_used_at: null,
  created_at: "2026-09-01T00:00:00Z",
  revoked_at: null,
  status: "active",
  source: "ui",
  owner: { type: "service_account", id: "sa-1", label: "deployer" },
  blocked_orgs: [],
  ...over
});

const trustPolicy = (over: Partial<TrustPolicy> = {}): TrustPolicy => ({
  id: "tp-1",
  org_id: "org-1",
  service_account_id: "sa-1",
  provider: "github_actions",
  repository: "acme/storefront",
  repository_id: 123,
  repository_owner_id: 456,
  workflow_path: ".github/workflows/release.yml",
  environment: "production",
  ref_pattern: null,
  allow_self_hosted: false,
  grants: [grant],
  created_by: null,
  created_at: "2026-09-01T00:00:00Z",
  last_used_at: null,
  disabled_at: null,
  ...over
});

const onBack = vi.fn();

const show = (shown: ServiceAccount = account()) => {
  tokens = [token()];
  policies = [trustPolicy()];
  const user = userEvent.setup({ delay: null, pointerEventsCheck: 0 });
  render(
    <ServiceAccountDetail org={org} account={shown} takenNames={[shown.name]} onBack={onBack} />
  );
  return user;
};

afterEach(() => {
  cleanup();
  onBack.mockReset();
  for (const fn of Object.values(calls)) fn.mockReset();
});

describe("account actions", () => {
  it("names how many tokens and policies die before deleting, then steps back", async () => {
    calls.deleteAccount.mockResolvedValue(undefined);
    const user = show();
    await user.click(screen.getByTestId("api-access-account-actions"));
    await user.click(await screen.findByTestId("api-access-account-delete"));
    const dialog = screen.getByTestId("api-access-account-delete-dialog");
    expect(within(dialog).getByTestId("api-access-account-delete-impact")).toHaveTextContent(
      "its 3 tokens and 1 trusted-access policy"
    );
    await user.click(within(dialog).getByTestId("api-access-account-delete-dialog-confirm"));
    expect(calls.deleteAccount).toHaveBeenCalledWith({ orgId: "org-1", saId: "sa-1" });
    expect(onBack).toHaveBeenCalled();
  });

  it("says nothing else stops working when the account holds nothing", async () => {
    const user = show(account({ token_count: 0, trust_policy_count: 0 }));
    await user.click(screen.getByTestId("api-access-account-actions"));
    await user.click(await screen.findByTestId("api-access-account-delete"));
    expect(screen.getByTestId("api-access-account-delete-impact")).toHaveTextContent(
      "no tokens and no trusted-access policies"
    );
  });

  it("confirms before disabling, and says it can be undone", async () => {
    calls.updateAccount.mockResolvedValue(undefined);
    const user = show();
    await user.click(screen.getByTestId("api-access-account-actions"));
    await user.click(await screen.findByTestId("api-access-account-disable"));
    const dialog = screen.getByTestId("api-access-account-disable-dialog");
    expect(dialog).toHaveTextContent("enabling the account brings it all back");
    expect(calls.updateAccount).not.toHaveBeenCalled();
    await user.click(within(dialog).getByTestId("api-access-account-disable-dialog-confirm"));
    expect(calls.updateAccount).toHaveBeenCalledWith({
      orgId: "org-1",
      saId: "sa-1",
      request: { disabled: true }
    });
  });

  it("enables a disabled account straight away, and holds its create buttons back until then", async () => {
    calls.updateAccount.mockResolvedValue(undefined);
    const user = show(account({ disabled_at: "2026-09-20T00:00:00Z" }));
    expect(screen.getByTestId("api-access-account-disabled-notice")).toBeInTheDocument();
    expect(screen.getByTestId("api-access-token-create")).toBeDisabled();
    expect(screen.getByTestId("api-access-policy-create")).toBeDisabled();
    await user.click(screen.getByTestId("api-access-account-actions"));
    await user.click(await screen.findByTestId("api-access-account-enable"));
    expect(calls.updateAccount).toHaveBeenCalledWith({
      orgId: "org-1",
      saId: "sa-1",
      request: { disabled: false }
    });
  });

  it("edits the description and role, never the name", async () => {
    calls.updateAccount.mockResolvedValue(undefined);
    const user = show();
    await user.click(screen.getByTestId("api-access-account-actions"));
    await user.click(await screen.findByTestId("api-access-account-edit"));
    expect(screen.queryByTestId("api-access-account-name")).not.toBeInTheDocument();
    const description = screen.getByTestId("api-access-account-description");
    await user.clear(description);
    await user.click(screen.getByTestId("api-access-account-role-admin"));
    await user.click(screen.getByTestId("api-access-account-submit"));
    expect(calls.updateAccount).toHaveBeenCalledWith({
      orgId: "org-1",
      saId: "sa-1",
      // A cleared description is sent as null, not as an empty string.
      request: { description: null, org_role: "admin" }
    });
  });
});

describe("token actions", () => {
  it("extends by the picked preset and keeps the secret", async () => {
    const user = show();
    await user.click(screen.getByTestId("api-access-token-extend"));
    const popover = await screen.findByTestId("api-key-extend-popover");
    expect(popover).toHaveTextContent("The secret stays the same");
    await user.click(
      within(within(popover).getByTestId("api-key-extend-option-90")).getByRole("radio")
    );
    await user.click(within(popover).getByTestId("api-key-extend-submit"));
    // Through this account's own routes, not the workspace or personal ones.
    expect(calls.extendToken).toHaveBeenCalledWith({
      endpoints: { orgId: "org-1", saId: "sa-1" },
      tokenId: "t-1",
      request: { days: 90 }
    });
  });

  it("warns that regenerating kills the current secret, then shows the new one once", async () => {
    calls.regenerateToken.mockResolvedValue({ token: token(), secret: "oxy_sat_NEW" });
    const user = show();
    await user.click(screen.getByTestId("api-access-token-regenerate"));
    const confirm = screen.getByTestId("api-access-token-regenerate-dialog");
    expect(confirm).toHaveTextContent("The current secret stops working at once");
    await user.click(within(confirm).getByTestId("api-access-token-regenerate-dialog-confirm"));
    expect(calls.regenerateToken).toHaveBeenCalledWith({
      orgId: "org-1",
      saId: "sa-1",
      tokenId: "t-1"
    });
    const secret = await screen.findByTestId("api-access-secret-dialog");
    expect(secret).toHaveTextContent("nightly sync regenerated");
  });

  it("revokes only after confirming", async () => {
    calls.revokeToken.mockResolvedValue(undefined);
    const user = show();
    await user.click(screen.getByTestId("api-access-token-revoke"));
    expect(calls.revokeToken).not.toHaveBeenCalled();
    await user.click(screen.getByTestId("api-access-token-revoke-dialog-confirm"));
    expect(calls.revokeToken).toHaveBeenCalledWith({
      orgId: "org-1",
      saId: "sa-1",
      tokenId: "t-1"
    });
  });

  it("opens the activity drawer for the token", async () => {
    const user = show();
    await user.click(screen.getByTestId("api-access-token-activity"));
    const drawer = await screen.findByTestId("api-key-activity-drawer");
    expect(drawer).toHaveTextContent("nightly sync");
    expect(drawer).toHaveTextContent("oxy_sat_ab12…9f3c");
    expect(within(drawer).getByTestId("api-key-activity-loading")).toBeInTheDocument();
  });
});

describe("trusted-access policy actions", () => {
  const openMenu = async (user: ReturnType<typeof userEvent.setup>, item: string) => {
    await user.click(screen.getByTestId("api-access-policy-actions"));
    await user.click(await screen.findByTestId(item));
  };

  it("edits everything but the repository", async () => {
    calls.updatePolicy.mockResolvedValue(trustPolicy());
    const user = show();
    await openMenu(user, "api-access-policy-edit");
    expect(screen.getByTestId("api-access-policy-repository")).toBeDisabled();
    const environment = screen.getByTestId("api-access-policy-environment");
    expect(environment).toHaveValue("production");
    await user.clear(environment);
    await user.type(environment, "staging");
    await user.click(screen.getByTestId("api-access-policy-self-hosted"));
    await user.click(screen.getByTestId("api-access-policy-submit"));
    expect(calls.updatePolicy).toHaveBeenCalledWith({
      orgId: "org-1",
      saId: "sa-1",
      policyId: "tp-1",
      request: {
        workflow_path: ".github/workflows/release.yml",
        environment: "staging",
        ref_pattern: null,
        allow_self_hosted: true,
        grants: [{ kind: "workspace", workspace_id: "ws-1", role_ceiling: "member" }]
      }
    });
  });

  it("disables a policy from its menu", async () => {
    calls.updatePolicy.mockResolvedValue(trustPolicy());
    const user = show();
    await openMenu(user, "api-access-policy-disable");
    expect(calls.updatePolicy).toHaveBeenCalledWith({
      orgId: "org-1",
      saId: "sa-1",
      policyId: "tp-1",
      request: { disabled: true }
    });
  });

  it("deletes only after confirming", async () => {
    calls.deletePolicy.mockResolvedValue(undefined);
    const user = show();
    await openMenu(user, "api-access-policy-delete");
    expect(calls.deletePolicy).not.toHaveBeenCalled();
    await user.click(screen.getByTestId("api-access-policy-delete-dialog-confirm"));
    expect(calls.deletePolicy).toHaveBeenCalledWith({
      orgId: "org-1",
      saId: "sa-1",
      policyId: "tp-1"
    });
  });

  it("shows the matching workflow again on request", async () => {
    const user = show();
    await openMenu(user, "api-access-policy-show-workflow");
    const snippet = await screen.findByTestId("api-access-snippet");
    expect(snippet).toHaveTextContent("# .github/workflows/release.yml");
    // Named by id; the handle rides along as a comment.
    expect(snippet).toHaveTextContent("OXY_SERVICE_ACCOUNT: sa-1 # acme/deployer");
  });
});
