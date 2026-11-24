// @vitest-environment jsdom

import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AxiosError, type AxiosResponse } from "axios";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Grant, Token } from "@/types/apiToken";
import type {
  InventoryToken,
  IssuerTrustPolicy,
  OidcIssuer,
  ServiceAccount,
  TokenPolicy,
  TrustPolicy
} from "@/types/orgApiAccess";
import type { Organization, OrgRole } from "@/types/organization";
import ApiAccessSection from ".";

vi.setConfig({ testTimeout: 30000 });

const httpError = (status: number, data: unknown = {}) =>
  new AxiosError("Request failed", "ERR", undefined, undefined, {
    status,
    data
  } as AxiosResponse);

const loaded = <T,>(data: T) => ({
  data,
  isPending: false,
  isLoading: false,
  isError: false,
  error: null as unknown,
  refetch: vi.fn()
});
const pending = () => ({ ...loaded(undefined), isPending: true, isLoading: true });
const failed = (status: number) => ({
  ...loaded(undefined),
  isError: true,
  error: httpError(status)
});

type Query<T> = ReturnType<typeof loaded<T | undefined>>;

// The hooks are the seam: this file is about what each server answer renders,
// and which request body each form sends.
const queries: {
  accounts: Query<ServiceAccount[]>;
  tokens: Query<Token[]>;
  policies: Query<TrustPolicy[]>;
  inventory: Query<InventoryToken[]>;
  policy: Query<TokenPolicy>;
  issuers: Query<OidcIssuer[]>;
  issuerPolicies: Query<IssuerTrustPolicy[]>;
} = {
  accounts: pending(),
  tokens: pending(),
  policies: pending(),
  inventory: pending(),
  policy: pending(),
  issuers: pending(),
  issuerPolicies: pending()
};
const reads = { accounts: 0 };
const calls = {
  createAccount: vi.fn(),
  updateAccount: vi.fn(),
  deleteAccount: vi.fn(),
  createToken: vi.fn(),
  extendToken: vi.fn(),
  regenerateToken: vi.fn(),
  revokeToken: vi.fn(),
  createPolicy: vi.fn(),
  updatePolicy: vi.fn(),
  deletePolicy: vi.fn(),
  revokeGrant: vi.fn(),
  savePolicy: vi.fn(),
  createIssuer: vi.fn(),
  deleteIssuer: vi.fn(),
  createIssuerPolicy: vi.fn(),
  updateIssuerPolicy: vi.fn(),
  deleteIssuerPolicy: vi.fn()
};
const mutation = (fn: ReturnType<typeof vi.fn>) => ({ mutateAsync: fn, isPending: false });

vi.mock("@/hooks/api/orgApiAccess", () => ({
  useServiceAccountTokenEndpoints: (orgId: string, saId: string) => ({ orgId, saId }),
  useOrgInventoryEndpoints: (orgId: string) => ({ orgId }),
  useServiceAccounts: () => {
    reads.accounts += 1;
    return queries.accounts;
  },
  useCreateServiceAccount: () => mutation(calls.createAccount),
  useUpdateServiceAccount: () => mutation(calls.updateAccount),
  useDeleteServiceAccount: () => mutation(calls.deleteAccount),
  useServiceAccountTokens: () => queries.tokens,
  useCreateServiceAccountToken: () => mutation(calls.createToken),
  useRegenerateServiceAccountToken: () => mutation(calls.regenerateToken),
  useRevokeServiceAccountToken: () => mutation(calls.revokeToken),
  useTrustPolicies: () => queries.policies,
  useCreateTrustPolicy: () => mutation(calls.createPolicy),
  useUpdateTrustPolicy: () => mutation(calls.updatePolicy),
  useDeleteTrustPolicy: () => mutation(calls.deletePolicy),
  useOrgTokens: () => queries.inventory,
  useRevokeOrgTokenGrant: () => mutation(calls.revokeGrant),
  useTokenPolicy: () => queries.policy,
  useUpdateTokenPolicy: () => mutation(calls.savePolicy),
  useOidcIssuers: () => queries.issuers,
  useCreateOidcIssuer: () => mutation(calls.createIssuer),
  useDeleteOidcIssuer: () => mutation(calls.deleteIssuer),
  useIssuerTrustPolicies: () => queries.issuerPolicies,
  useCreateIssuerTrustPolicy: () => mutation(calls.createIssuerPolicy),
  useUpdateIssuerTrustPolicy: () => mutation(calls.updateIssuerPolicy),
  useDeleteIssuerTrustPolicy: () => mutation(calls.deleteIssuerPolicy)
}));

// Extend and Activity are the shared token components; their hooks are mocked the way the
// personal-tokens row mocks them, so this file stays about what each action sends.
vi.mock("@/hooks/api/tokens/useTokenMutations", () => ({
  useExtendApiKey: (endpoints: unknown) => ({
    mutate: (vars: { token: { id: string }; request: unknown }) =>
      calls.extendToken({ endpoints, tokenId: vars.token.id, request: vars.request }),
    isPending: false
  })
}));
vi.mock("@/hooks/api/tokens/useApiKeyActivity", () => ({
  API_KEY_ACTIVITY_LIMIT: 100,
  default: () => ({ isLoading: true, error: null, refetch: vi.fn() })
}));
vi.mock("@/hooks/api/workspaces/useWorkspaces", () => ({
  useAllWorkspaces: () => loaded([{ id: "ws-1", name: "Analytics" }])
}));
vi.mock("@/hooks/api/appAccess", () => ({
  useOrgAppAccessList: () => loaded([{ id: "app-1", name: "Store Ops" }])
}));
// The personal tokens tab has its own tests; here it only has to be the tab a member gets.
vi.mock("@/hooks/api/oxygenAccess", () => ({
  useOxygenAccessList: () => ({ data: [], isPending: false, error: null, refetch: vi.fn() }),
  useOxygenAccess: () => ({ data: undefined, isPending: true, error: null, refetch: vi.fn() }),
  useApproveOxygenAccess: () => ({ mutate: vi.fn(), isPending: false }),
  useRevokeOxygenAccess: () => ({ mutate: vi.fn(), isPending: false })
}));

vi.mock("../../account/PersonalTokens", () => ({
  default: () => <div data-testid='personal-tokens-tab' />
}));
// Whether the viewer could open Secrets is the nav's decision, tested in nav.test.ts.
const nav = vi.hoisted(() => ({ canOpenSecrets: true }));
vi.mock("../../../useVisibleNavGroups", () => ({
  useCanOpenSection: () => nav.canOpenSecrets
}));
// jsdom has no canvas; the activity chart is Phase 1's and not under test here.
vi.mock("@/components/Echarts/EChart", () => ({ default: () => <div /> }));

// Settings is a route now, so the section reads its tab from the address. These cases are
// about the section, not the router, so the address is faked — live, because a tab click
// only moves the section if it re-renders, the way a real navigation does. A plain object
// froze every tab-switching case.
const route = vi.hoisted(() => {
  let tab: string | null = null;
  const listeners = new Set<() => void>();
  return {
    get tab() {
      return tab;
    },
    set tab(next: string | null) {
      tab = next;
      for (const l of listeners) l();
    },
    subscribe: (l: () => void) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    open: vi.fn(),
    close: vi.fn()
  };
});

vi.mock("@/components/settings/useSettingsRoute", async () => {
  const { useSyncExternalStore } = await vi.importActual<typeof import("react")>("react");
  const useSettingsRoute = () => ({
    tab: useSyncExternalStore(
      route.subscribe,
      () => route.tab,
      () => route.tab
    ),
    section: "organization.api_tokens" as const,
    isOpen: true,
    orgSlug: "acme",
    setTab: (next: string | null) => {
      route.tab = next;
    },
    open: route.open,
    close: route.close
  });
  return { useSettingsRoute, useOpenSettings: () => route.open };
});

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
  token_count: 1,
  trust_policy_count: 1,
  ...over
});

const grant = (over: Partial<Grant> = {}): Grant => ({
  id: "g-1",
  kind: "workspace",
  org_id: "org-1",
  org_name: "Acme",
  workspace_id: null,
  workspace_name: null,
  role_ceiling: "member",
  app_id: null,
  app_name: null,
  revoked_at: null,
  ...over
});

const token = (over: Partial<Token> = {}): Token => ({
  id: "t-1",
  name: "nightly sync",
  kind: "service_account",
  display_prefix: "oxy_sat_ab12",
  last_four: "9f3c",
  all_access: false,
  platform: false,
  partner: false,
  grants: [grant()],
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
  grants: [grant({ workspace_id: "ws-1", workspace_name: "Analytics" })],
  created_by: null,
  created_at: "2026-09-01T00:00:00Z",
  last_used_at: null,
  disabled_at: null,
  ...over
});

const ROLE = "arn:aws:iam::123456789012:role/ferret-task";

const awsIssuer = (over: Partial<OidcIssuer> = {}): OidcIssuer => ({
  id: "iss-1",
  org_id: "org-1",
  name: "oxy-management",
  issuer: "https://abc123-def456.tokens.sts.global.api.aws",
  key_source: "discovery",
  discovery_url: null,
  jwks_uri: "https://abc123-def456.tokens.sts.global.api.aws/.well-known/jwks.json",
  inline_key_ids: [],
  jti_single_use: true,
  preset: "aws",
  policy_count: 0,
  created_by: null,
  created_at: "2026-11-19T00:00:00Z",
  ...over
});

const awsPolicy = (over: Partial<IssuerTrustPolicy> = {}): IssuerTrustPolicy => ({
  id: "tp-1",
  org_id: "org-1",
  service_account: { id: "sa-1", name: "deployer" },
  issuer: { id: "iss-1", name: "oxy-management", issuer: awsIssuer().issuer },
  conditions: [{ claim: "sub", value: ROLE }],
  grants: [grant()],
  created_by: null,
  created_at: "2026-11-19T00:00:00Z",
  last_used_at: null,
  disabled_at: null,
  ...over
});

const inventoryToken = (over: Partial<InventoryToken> = {}): InventoryToken => ({
  ...token(),
  grants_here: [grant()],
  blocked_by_policy: null,
  long_lived_while_trusted_access: false,
  ...over
});

const OPEN_POLICY: TokenPolicy = {
  max_lifetime_days: null,
  allow_all_access_tokens: true,
  require_environment_on_trust_policies: false
};

const show = (role: OrgRole = "owner") => {
  const user = userEvent.setup({ delay: null, pointerEventsCheck: 0 });
  render(<ApiAccessSection org={org} viewerRole={role} />);
  return user;
};

/** Renders the section and opens the one account's page. */
const showAccount = async () => {
  const user = show();
  await user.click(screen.getByTestId("api-access-account-open"));
  return user;
};

beforeEach(() => {
  // Most of this file is about the admin tabs, so it opens on Service accounts the way the old
  // `?settings=organization.api_access` link does; "which tab" has its own cases below.
  route.tab = "service_accounts";
  route.open.mockClear();
  nav.canOpenSecrets = true;
  queries.accounts = loaded([account()]);
  queries.tokens = loaded([token()]);
  queries.policies = loaded([trustPolicy()]);
  queries.inventory = loaded([inventoryToken()]);
  queries.policy = loaded(OPEN_POLICY);
  queries.issuers = loaded([awsIssuer()]);
  queries.issuerPolicies = loaded([]);
  reads.accounts = 0;
});

afterEach(() => {
  cleanup();
  for (const fn of Object.values(calls)) fn.mockReset();
});

describe("who can see it", () => {
  it("gives a member their own tokens and the guide, and reads nothing an admin route serves", () => {
    show("member");
    expect(screen.getByTestId("settings-api-tokens")).toBeInTheDocument();
    expect(screen.getByTestId("personal-tokens-tab")).toBeInTheDocument();
    // One tab is not a tab bar: no admin tab to click, even from a link that named one.
    expect(screen.queryByTestId("api-access-tab-service-accounts")).not.toBeInTheDocument();
    expect(screen.queryByTestId("api-access-tab-tokens")).not.toBeInTheDocument();
    expect(screen.queryByTestId("api-access-tab-policy")).not.toBeInTheDocument();
    expect(screen.queryByTestId("api-tokens-tab-apps")).not.toBeInTheDocument();
    expect(reads.accounts).toBe(0);
  });

  it.each(["owner", "admin"] as const)("gives an org %s every tab", (role) => {
    show(role);
    for (const id of [
      "api-tokens-tab-personal",
      "api-access-tab-service-accounts",
      "api-tokens-tab-apps",
      "api-access-tab-tokens",
      "api-access-tab-policy"
    ]) {
      expect(screen.getByTestId(id)).toBeInTheDocument();
    }
  });
});

describe("with no org loaded", () => {
  it("still lists the viewer's own tokens, and nothing that needs an org", () => {
    route.tab = "service_accounts";
    render(<ApiAccessSection org={null} viewerRole={null} />);
    expect(screen.getByTestId("personal-tokens-tab")).toBeInTheDocument();
    expect(screen.queryByTestId("api-access-tab-service-accounts")).not.toBeInTheDocument();
    expect(reads.accounts).toBe(0);
  });
});

describe("which tab", () => {
  it("opens on Personal access tokens when nothing names a tab", () => {
    route.tab = null;
    show();
    expect(screen.getByTestId("api-tokens-tab-personal")).toHaveAttribute("data-state", "active");
    expect(screen.getByTestId("personal-tokens-tab")).toBeInTheDocument();
  });

  it("opens the tab a link named", () => {
    route.tab = "policy";
    show();
    expect(screen.getByTestId("api-access-tab-policy")).toHaveAttribute("data-state", "active");
  });

  it("falls back to the first tab the viewer has when a link names one they don't", () => {
    route.tab = "all";
    show("member");
    expect(screen.getByTestId("personal-tokens-tab")).toBeInTheDocument();
    expect(screen.queryByTestId("api-access-inventory")).not.toBeInTheDocument();
  });

  it("keeps the tab in the dialog's store, so a link and a click agree", async () => {
    const user = show();
    await user.click(screen.getByTestId("api-access-tab-tokens"));
    expect(route.tab).toBe("all");
  });
});

describe("which token do I need", () => {
  const row = (id: string) => within(screen.getByTestId(`api-tokens-guide-${id}`));

  it("takes an admin to the tab that makes each kind", async () => {
    const user = show();
    await user.click(row("service-account").getByRole("button"));
    expect(route.tab).toBe("service_accounts");
    await user.click(row("personal").getByRole("button"));
    expect(route.tab).toBe("personal");
  });

  it("sends an app that manages kiosks or uploads reports to Apps, where no token is made", async () => {
    const user = show();
    await user.click(row("app-approval").getByRole("button"));
    expect(route.tab).toBe("apps");
    expect(
      screen.getByText(/No app of this organization asks to act in Oxygen/)
    ).toBeInTheDocument();
  });

  it("sends a custom app's token to the app's secret, where it can be created in place", async () => {
    const user = show();
    await user.click(row("custom-app").getByRole("button", { name: /Create API token/ }));
    // A different section is a navigation now, not a store write.
    expect(route.open).toHaveBeenCalledWith("organization.secrets", "app");
  });

  it("tells a member who makes the kinds they can't, instead of linking to a tab they lack", () => {
    nav.canOpenSecrets = false;
    show("member");
    expect(row("personal").getByRole("button")).toBeInTheDocument();
    expect(row("service-account").queryByRole("button")).not.toBeInTheDocument();
    expect(row("service-account").getByText(/from an organization admin/)).toBeInTheDocument();
    expect(row("custom-app").queryByRole("button")).not.toBeInTheDocument();
    expect(row("app-approval").queryByRole("button")).not.toBeInTheDocument();
    expect(row("app-approval").getByText(/from an organization admin/)).toBeInTheDocument();
    expect(row("secret").getByText(/set by an organization admin/)).toBeInTheDocument();
  });
});

describe("the service account list before it is a list", () => {
  it("shows a skeleton while loading", () => {
    queries.accounts = pending();
    show();
    expect(screen.getByTestId("api-access-accounts-loading")).toBeInTheDocument();
  });

  it("treats a 404 as 'not available here yet', not as a failure", () => {
    queries.accounts = failed(404);
    show();
    expect(screen.getByTestId("api-access-accounts-unavailable")).toBeInTheDocument();
    expect(screen.queryByTestId("api-access-accounts-error")).not.toBeInTheDocument();
  });

  it("offers a retry on any other failure", async () => {
    queries.accounts = failed(500);
    const user = show();
    await user.click(
      within(screen.getByTestId("api-access-accounts-error")).getByRole("button", {
        name: "Try again"
      })
    );
    expect(queries.accounts.refetch).toHaveBeenCalled();
  });

  it("invites the first account when there are none", () => {
    queries.accounts = loaded([]);
    show();
    const empty = screen.getByTestId("api-access-accounts-empty");
    expect(within(empty).getByTestId("api-access-account-create")).toBeInTheDocument();
  });
});

describe("creating a service account", () => {
  it("normalizes the name to a slug as it is typed and sends the chosen role", async () => {
    queries.accounts = loaded([]);
    calls.createAccount.mockResolvedValue(account({ id: "sa-2", name: "release-bot" }));
    const user = show();
    await user.click(screen.getByTestId("api-access-account-create"));
    await user.type(screen.getByTestId("api-access-account-name"), "Release Bot");
    expect(screen.getByTestId("api-access-account-name")).toHaveValue("release-bot");
    // The hint shows the handle a workflow will use.
    expect(screen.getByText("acme/release-bot")).toBeInTheDocument();
    await user.click(screen.getByTestId("api-access-account-role-admin"));
    await user.click(screen.getByTestId("api-access-account-submit"));
    expect(calls.createAccount).toHaveBeenCalledWith({
      orgId: "org-1",
      request: { name: "release-bot", org_role: "admin" }
    });
  });

  it("refuses a name the org already uses without a round trip", async () => {
    const user = show();
    await user.click(screen.getByTestId("api-access-account-create"));
    await user.type(screen.getByTestId("api-access-account-name"), "deployer");
    await user.click(screen.getByTestId("api-access-account-submit"));
    expect(screen.getByRole("alert")).toHaveTextContent(/already exists/);
    expect(calls.createAccount).not.toHaveBeenCalled();
  });

  it("puts the server's name_taken on the name field", async () => {
    queries.accounts = loaded([]);
    calls.createAccount.mockRejectedValue(httpError(409, { error: "taken", code: "name_taken" }));
    const user = show();
    await user.click(screen.getByTestId("api-access-account-create"));
    await user.type(screen.getByTestId("api-access-account-name"), "deployer");
    await user.click(screen.getByTestId("api-access-account-submit"));
    expect(await screen.findByRole("alert")).toHaveTextContent(/already exists/);
  });
});

describe("a service account's page", () => {
  it("shows its handle, the ID workflows name it by, its tokens and its policies", async () => {
    await showAccount();
    expect(screen.getByTestId("api-access-account-handle")).toHaveTextContent("acme/deployer");
    // The id is what a workflow carries, so it is shown and can be copied.
    expect(screen.getByTestId("api-access-account-id")).toHaveTextContent("sa-1");
    expect(screen.getByTestId("api-access-account-id-copy")).toBeInTheDocument();
    const tokenRow = screen.getByTestId("api-access-token-row");
    expect(tokenRow).toHaveTextContent("nightly sync");
    expect(tokenRow).toHaveTextContent("oxy_sat_ab12…9f3c");
    expect(tokenRow).toHaveTextContent("Every workspace, as Member");
    const policyRow = screen.getByTestId("api-access-policy-row");
    expect(policyRow).toHaveTextContent("acme/storefront");
    expect(policyRow).toHaveTextContent("Analytics as Member");
  });

  it("flags a policy that names no environment", async () => {
    queries.policies = loaded([trustPolicy({ environment: null })]);
    await showAccount();
    expect(screen.getByTestId("api-access-policy-no-environment")).toBeInTheDocument();
  });

  it("offers no Extend on a token that never expires, and none of the three on a revoked one", async () => {
    queries.tokens = loaded([
      token({ id: "t-never", name: "forever", expires_at: null }),
      token({ id: "t-dead", name: "dead", status: "revoked", revoked_at: inDays(-1) })
    ]);
    await showAccount();
    const [forever, dead] = screen.getAllByTestId("api-access-token-row");
    expect(within(forever).queryByTestId("api-access-token-extend")).not.toBeInTheDocument();
    expect(within(forever).getByTestId("api-access-token-regenerate")).toBeInTheDocument();
    expect(within(dead).queryByTestId("api-access-token-extend")).not.toBeInTheDocument();
    expect(within(dead).queryByTestId("api-access-token-regenerate")).not.toBeInTheDocument();
    expect(within(dead).queryByTestId("api-access-token-revoke")).not.toBeInTheDocument();
    expect(within(dead).getByTestId("api-access-token-activity")).toBeInTheDocument();
  });

  it("steps back to the list", async () => {
    const user = await showAccount();
    await user.click(screen.getByTestId("api-access-account-back"));
    expect(screen.getByTestId("api-access-account-table")).toBeInTheDocument();
  });
});

describe("creating a token", () => {
  const createToken = async (user: ReturnType<typeof userEvent.setup>, name: string) => {
    await user.click(screen.getByTestId("api-access-token-create"));
    await user.type(screen.getByTestId("api-access-token-name"), name);
    await user.click(screen.getByTestId("api-access-token-submit"));
  };

  it("defaults to 90 days and the whole org, then shows the secret once", async () => {
    calls.createToken.mockResolvedValue({ token: token({ name: "ci" }), secret: "oxy_sat_SECRET" });
    const user = await showAccount();
    await createToken(user, "ci");
    // No `grants`: the server grants the whole org at the account's role.
    expect(calls.createToken).toHaveBeenCalledWith({
      orgId: "org-1",
      saId: "sa-1",
      request: { name: "ci", expires_in_days: 90 }
    });

    const dialog = await screen.findByTestId("api-access-secret-dialog");
    const snippet = within(dialog).getByTestId("api-access-secret-export");
    expect(snippet).toHaveTextContent("export OXY_TOKEN=");
    expect(dialog).not.toHaveTextContent("oxy_sat_SECRET");
    await user.click(within(dialog).getByTestId("api-access-secret-reveal"));
    expect(snippet).toHaveTextContent("export OXY_TOKEN=oxy_sat_SECRET");

    await user.click(within(dialog).getByTestId("api-access-secret-done"));
    expect(screen.queryByTestId("api-access-secret-dialog")).not.toBeInTheDocument();
  });

  it("starts at the org's lifetime cap and disables what the cap rules out", async () => {
    queries.policy = loaded({ ...OPEN_POLICY, max_lifetime_days: 30 });
    calls.createToken.mockResolvedValue({ token: token(), secret: "s" });
    const user = await showAccount();
    await user.click(screen.getByTestId("api-access-token-create"));
    expect(
      within(screen.getByTestId("api-access-token-expiry-90")).getByRole("radio")
    ).toBeDisabled();
    expect(
      within(screen.getByTestId("api-access-token-expiry-never")).getByRole("radio")
    ).toBeDisabled();
    expect(
      within(screen.getByTestId("api-access-token-expiry-30")).getByRole("radio")
    ).toBeChecked();
    expect(screen.getByTestId("api-access-token-expiry-outcome")).toHaveTextContent(
      "This organization limits tokens to 30 days."
    );
    await user.type(screen.getByTestId("api-access-token-name"), "ci");
    await user.click(screen.getByTestId("api-access-token-submit"));
    expect(calls.createToken.mock.calls[0][0].request).toEqual({ name: "ci", expires_in_days: 30 });
  });

  it("sends a ceiling per picked workspace, and refuses an empty selection", async () => {
    calls.createToken.mockResolvedValue({ token: token(), secret: "s" });
    const user = await showAccount();
    await user.click(screen.getByTestId("api-access-token-create"));
    await user.type(screen.getByTestId("api-access-token-name"), "ci");
    await user.click(
      within(screen.getByTestId("api-access-token-access-scope-selected")).getByRole("radio")
    );
    await user.click(screen.getByTestId("api-access-token-submit"));
    expect(screen.getByText("Pick at least one workspace.")).toBeInTheDocument();
    expect(calls.createToken).not.toHaveBeenCalled();

    await user.click(
      within(screen.getByTestId("api-access-token-access-workspace")).getByRole("checkbox")
    );
    await user.click(screen.getByTestId("api-access-token-submit"));
    expect(calls.createToken.mock.calls[0][0].request).toEqual({
      name: "ci",
      expires_in_days: 90,
      grants: [{ kind: "workspace", workspace_id: "ws-1", role_ceiling: "member" }]
    });
  });

  it("asks for a name before sending anything", async () => {
    const user = await showAccount();
    await user.click(screen.getByTestId("api-access-token-create"));
    await user.click(screen.getByTestId("api-access-token-submit"));
    expect(screen.getByRole("alert")).toHaveTextContent(/Give the token a name/);
    expect(calls.createToken).not.toHaveBeenCalled();
  });
});

describe("adding a trusted-access policy", () => {
  const fill = async (user: ReturnType<typeof userEvent.setup>) => {
    await user.click(screen.getByTestId("api-access-policy-create"));
    await user.type(screen.getByTestId("api-access-policy-repository"), "acme/storefront");
    await user.type(screen.getByTestId("api-access-policy-workflow"), "release.yml");
    await user.click(
      within(screen.getByTestId("api-access-policy-access-workspace")).getByRole("checkbox")
    );
  };
  const submit = (user: ReturnType<typeof userEvent.setup>) =>
    user.click(screen.getByTestId("api-access-policy-submit"));

  it("warns, while the environment is empty, that anyone who can push could get a token", async () => {
    const user = await showAccount();
    await fill(user);
    expect(screen.getByTestId("api-access-policy-environment-warning")).toHaveTextContent(
      /anyone who can push to this repository/
    );
    await user.type(screen.getByTestId("api-access-policy-environment"), "production");
    expect(screen.queryByTestId("api-access-policy-environment-warning")).not.toBeInTheDocument();
  });

  it("refuses an empty environment up front when the org's policy requires one", async () => {
    queries.policy = loaded({ ...OPEN_POLICY, require_environment_on_trust_policies: true });
    const user = await showAccount();
    await fill(user);
    await submit(user);
    expect(
      screen.getByText(/requires an environment on every trusted-access policy/)
    ).toBeInTheDocument();
    expect(calls.createPolicy).not.toHaveBeenCalled();
  });

  it("keeps self-hosted runners off and sends null for what was left blank", async () => {
    calls.createPolicy.mockResolvedValue(trustPolicy({ environment: null }));
    const user = await showAccount();
    await fill(user);
    await submit(user);
    expect(calls.createPolicy).toHaveBeenCalledWith({
      orgId: "org-1",
      saId: "sa-1",
      request: {
        repository: "acme/storefront",
        workflow_path: ".github/workflows/release.yml",
        environment: null,
        ref_pattern: null,
        allow_self_hosted: false,
        grants: [{ kind: "workspace", workspace_id: "ws-1", role_ceiling: "member" }]
      }
    });
  });

  it("can grant publishing an app instead of a workspace", async () => {
    calls.createPolicy.mockResolvedValue(trustPolicy());
    const user = await showAccount();
    await user.click(screen.getByTestId("api-access-policy-create"));
    await user.type(screen.getByTestId("api-access-policy-repository"), "acme/storefront");
    await user.type(screen.getByTestId("api-access-policy-workflow"), "release.yml");
    await user.click(
      within(screen.getByTestId("api-access-policy-access-app")).getByRole("checkbox")
    );
    await submit(user);
    expect(calls.createPolicy.mock.calls[0][0].request.grants).toEqual([
      { kind: "app_publish", app_id: "app-1" }
    ]);
  });

  it("asks for the numeric ids only after the server couldn't resolve the repository", async () => {
    calls.createPolicy
      .mockRejectedValueOnce(httpError(422, { error: "unresolved", code: "repository_unresolved" }))
      .mockResolvedValueOnce(trustPolicy());
    const user = await showAccount();
    await fill(user);
    expect(screen.queryByTestId("api-access-policy-ids")).not.toBeInTheDocument();
    await submit(user);

    const ids = await screen.findByTestId("api-access-policy-ids");
    // The hint says where the ids come from, for this very repository.
    expect(ids).toHaveTextContent("gh api repos/acme/storefront --jq '.id, .owner.id'");
    await user.type(screen.getByTestId("api-access-policy-repo-id"), "123");
    await user.type(screen.getByTestId("api-access-policy-owner-id"), "456");
    await submit(user);
    expect(calls.createPolicy.mock.calls[1][0].request).toMatchObject({
      repository: "acme/storefront",
      repository_id: 123,
      repository_owner_id: 456
    });
  });

  it("shows the server's environment_required on the environment field", async () => {
    calls.createPolicy.mockRejectedValueOnce(
      httpError(400, { error: "environment is required", code: "environment_required" })
    );
    const user = await showAccount();
    await fill(user);
    await submit(user);
    expect(
      await screen.findByText(/requires an environment on every trusted-access policy/)
    ).toBeInTheDocument();
    // The warning gives way to the refusal: the field is now required, not advised.
    expect(screen.queryByTestId("api-access-policy-environment-warning")).not.toBeInTheDocument();
  });

  it("hands over a workflow pre-filled with the account's handle and the environment", async () => {
    calls.createPolicy.mockResolvedValue(trustPolicy());
    const user = await showAccount();
    await fill(user);
    await user.type(screen.getByTestId("api-access-policy-environment"), "production");
    await submit(user);
    const snippet = await screen.findByTestId("api-access-snippet");
    // Named by id; the handle rides along as a comment.
    expect(snippet).toHaveTextContent("OXY_SERVICE_ACCOUNT: sa-1 # acme/deployer");
    expect(snippet).toHaveTextContent("environment: production");
    expect(snippet).toHaveTextContent("id-token: write");
  });
});

describe("trusted access from an OIDC issuer", () => {
  it("adds a policy for an AWS role, then shows what the workload runs", async () => {
    calls.createIssuerPolicy.mockResolvedValue(awsPolicy());
    const user = show();
    expect(screen.getByTestId("api-access-issuer-no-policies")).toBeInTheDocument();
    await user.click(screen.getByTestId("api-access-trust-policy-create"));
    // No single workspace on offer: the whole organization, or apps only.
    expect(screen.queryByTestId("api-access-trust-access-workspaces")).not.toBeInTheDocument();
    await user.type(screen.getByTestId("api-access-trust-role"), "ferret-task");
    await user.click(screen.getByTestId("api-access-trust-submit"));
    expect(calls.createIssuerPolicy).not.toHaveBeenCalled();
    await user.clear(screen.getByTestId("api-access-trust-role"));
    await user.type(screen.getByTestId("api-access-trust-role"), ROLE);
    await user.click(screen.getByTestId("api-access-trust-submit"));
    expect(calls.createIssuerPolicy).toHaveBeenCalledWith({
      orgId: "org-1",
      saId: "sa-1",
      request: {
        issuer_id: "iss-1",
        conditions: [{ claim: "sub", value: ROLE }],
        grants: [{ kind: "workspace", workspace_id: null, role_ceiling: "member" }]
      }
    });
    // A policy alone does nothing: the workload's commands come next, naming the account by id.
    const snippet = await screen.findByTestId("api-access-client-setup");
    expect(snippet).toHaveTextContent("aws sts get-web-identity-token");
    expect(snippet).toHaveTextContent("--signing-algorithm RS256");
    expect(snippet).toHaveTextContent("OXY_SERVICE_ACCOUNT=sa-1");
    expect(snippet).toHaveTextContent("acme/deployer");
  });

  it("asks STS for the deployment's audience, not an org subdomain's", async () => {
    // On acme.<host> the browser's origin is the subdomain; the exchange takes only the
    // audience of the deployment's own address, which the server injects as appBaseUrl.
    window.__OXY_ORG__ = {
      orgId: "org-1",
      orgSlug: "acme",
      subdomain: "acme",
      appBaseUrl: "https://app.example.com"
    };
    try {
      calls.createIssuerPolicy.mockResolvedValue(awsPolicy());
      const user = show();
      await user.click(screen.getByTestId("api-access-trust-policy-create"));
      await user.type(screen.getByTestId("api-access-trust-role"), ROLE);
      await user.click(screen.getByTestId("api-access-trust-submit"));
      const snippet = await screen.findByTestId("api-access-client-setup");
      expect(snippet).toHaveTextContent("--audience 'oxy:app.example.com'");
      expect(snippet).toHaveTextContent("--target https://app.example.com");
    } finally {
      delete window.__OXY_ORG__;
    }
  });

  it("takes claim conditions for a custom issuer, refusing a bare '*'", async () => {
    queries.issuers = loaded([awsIssuer({ preset: "custom", name: "gitlab" })]);
    calls.createIssuerPolicy.mockResolvedValue(
      awsPolicy({ conditions: [{ claim: "sub", value: "project_path:acme/app:*" }] })
    );
    const user = show();
    await user.click(screen.getByTestId("api-access-trust-policy-create"));
    const value = screen.getByTestId("api-access-trust-condition-value");
    await user.type(value, "*");
    await user.click(screen.getByTestId("api-access-trust-submit"));
    expect(calls.createIssuerPolicy).not.toHaveBeenCalled();
    await user.clear(value);
    await user.type(value, "project_path:acme/app:*");
    await user.click(screen.getByTestId("api-access-trust-submit"));
    expect(calls.createIssuerPolicy).toHaveBeenCalledWith(
      expect.objectContaining({
        request: expect.objectContaining({
          conditions: [{ claim: "sub", value: "project_path:acme/app:*" }]
        })
      })
    );
    // No AWS role, so no workload commands.
    expect(screen.queryByTestId("api-access-client-setup")).not.toBeInTheDocument();
  });

  it("lists each policy under its issuer, by the account and the role", async () => {
    queries.issuerPolicies = loaded([awsPolicy()]);
    show();
    const row = screen.getByTestId("api-access-trust-policy-row");
    expect(row).toHaveTextContent("deployer");
    expect(row).toHaveTextContent(ROLE);
  });

  it("registers an AWS account's issuer", async () => {
    queries.issuers = loaded([]);
    calls.createIssuer.mockResolvedValue(awsIssuer());
    const user = show();
    await user.click(screen.getByTestId("api-access-issuer-create"));
    await user.type(screen.getByTestId("api-access-issuer-name"), "oxy-management");
    await user.type(screen.getByTestId("api-access-issuer-url"), "http://not-https.example");
    await user.click(screen.getByTestId("api-access-issuer-submit"));
    expect(calls.createIssuer).not.toHaveBeenCalled();
    await user.clear(screen.getByTestId("api-access-issuer-url"));
    await user.type(screen.getByTestId("api-access-issuer-url"), awsIssuer().issuer);
    await user.click(screen.getByTestId("api-access-issuer-submit"));
    expect(calls.createIssuer).toHaveBeenCalledWith({
      orgId: "org-1",
      request: {
        name: "oxy-management",
        issuer: awsIssuer().issuer,
        key_source: "discovery",
        jti_single_use: true,
        preset: "aws"
      }
    });
  });

  it("says what removing an issuer ends", async () => {
    queries.issuers = loaded([awsIssuer({ policy_count: 2 })]);
    const user = show();
    await user.click(screen.getByTestId("api-access-issuer-delete"));
    expect(screen.getByTestId("api-access-issuer-delete-dialog")).toHaveTextContent(
      /Its 2 trust policies are deleted with it, and every token they minted stops working now/
    );
  });
});

describe("the token inventory", () => {
  const openInventory = async () => {
    const user = show();
    await user.click(screen.getByTestId("api-access-tab-tokens"));
    return user;
  };
  const rowNamed = (name: string) =>
    screen
      .getAllByTestId("api-access-inventory-row")
      .find((row) => row.getAttribute("data-token-name") === name) as HTMLElement;
  const legacyRowNamed = (name: string) =>
    screen
      .getAllByTestId("api-access-legacy-key-row")
      .find((row) => row.getAttribute("data-token-name") === name) as HTMLElement;

  beforeEach(() => {
    queries.inventory = loaded([
      inventoryToken({
        id: "t-pat",
        name: "laptop",
        kind: "personal",
        owner: { type: "user", id: "u-1", label: "Ada Lovelace" },
        blocked_by_policy: "max_lifetime",
        long_lived_while_trusted_access: true
      }),
      inventoryToken({
        id: "t-legacy",
        name: "old key",
        kind: "legacy_key",
        all_access: true,
        grants_here: [],
        owner: { type: "user", id: "u-2", label: "Grace Hopper" }
      }),
      inventoryToken({ id: "t-sat", name: "nightly sync" })
    ]);
  });

  it("shows who owns each token, what it reaches here, and what blocks it", async () => {
    await openInventory();
    const personal = rowNamed("laptop");
    expect(personal).toHaveTextContent("Ada Lovelace");
    expect(personal).toHaveTextContent("Personal");
    expect(within(personal).getByTestId("api-access-inventory-blocked")).toHaveTextContent(
      /Blocked.*expiry is further out than this organization allows/
    );
    expect(within(personal).getByTestId("api-access-inventory-long-lived")).toHaveTextContent(
      "Long-lived"
    );
    expect(rowNamed("nightly sync")).toHaveTextContent("Service account");
  });

  it("lists removed legacy API keys in a group of their own, never among the tokens", async () => {
    await openInventory();
    // The token list holds the two tokens and nothing else.
    const tokenTable = screen.getByTestId("api-access-inventory-table");
    expect(
      within(tokenTable)
        .getAllByTestId("api-access-inventory-row")
        .map((row) => row.getAttribute("data-token-name"))
    ).toEqual(["laptop", "nightly sync"]);
    expect(tokenTable).not.toHaveTextContent("old key");
    expect(tokenTable).not.toHaveTextContent(/legacy/i);

    // The legacy API key sits below, under its own heading.
    const group = screen.getByTestId("api-access-legacy-keys");
    expect(
      within(group).getByRole("heading", { name: "Legacy API keys (removed)" })
    ).toBeInTheDocument();
    expect(group).toHaveTextContent("None of them authenticates anything any more");
    expect(within(group).queryByTestId("api-access-inventory-row")).not.toBeInTheDocument();
    expect(
      tokenTable.compareDocumentPosition(group) & Node.DOCUMENT_POSITION_FOLLOWING
    ).toBeTruthy();
  });

  it("badges every legacy API key Legacy, and no token", async () => {
    queries.inventory = loaded([
      inventoryToken({ id: "t-sat", name: "nightly sync" }),
      ...["old key", "older key"].map((name, i) =>
        inventoryToken({
          id: `t-legacy-${i}`,
          name,
          kind: "legacy_key",
          all_access: true,
          grants_here: [],
          owner: { type: "user", id: "u-2", label: "Grace Hopper" }
        })
      )
    ]);
    await openInventory();
    for (const name of ["old key", "older key"]) {
      const row = legacyRowNamed(name);
      expect(within(row).getByTestId("legacy-badge")).toHaveTextContent("Legacy");
      expect(row).toHaveTextContent("Grace Hopper");
    }
    expect(within(rowNamed("nightly sync")).queryByTestId("legacy-badge")).not.toBeInTheDocument();
  });

  it("still opens a legacy API key's activity, and calls it what it is", async () => {
    const user = await openInventory();
    await user.click(
      within(legacyRowNamed("old key")).getByTestId("api-access-legacy-key-activity")
    );
    const drawer = await screen.findByTestId("api-key-activity-drawer");
    expect(drawer).toHaveTextContent("old key");
    expect(within(drawer).getByTestId("api-key-activity-scope")).toHaveTextContent(
      "Only what this legacy API key did in Acme, while legacy keys still worked."
    );
  });

  it("says the token list is empty when only legacy API keys reach the org", async () => {
    queries.inventory = loaded([
      inventoryToken({
        id: "t-legacy",
        name: "old key",
        kind: "legacy_key",
        all_access: true,
        grants_here: [],
        owner: { type: "user", id: "u-2", label: "Grace Hopper" }
      })
    ]);
    await openInventory();
    // The token list says it is empty, rather than leaving a hole above the group.
    expect(screen.getByTestId("api-access-inventory-no-tokens")).toHaveTextContent(
      "No API tokens reach Acme yet."
    );
    expect(screen.getByTestId("api-access-legacy-keys")).toBeInTheDocument();
    expect(screen.queryByTestId("api-access-inventory-table")).not.toBeInTheDocument();
  });

  it("leaves the legacy group out when no legacy API key reaches the org", async () => {
    queries.inventory = loaded([inventoryToken({ id: "t-sat", name: "nightly sync" })]);
    await openInventory();
    expect(screen.getByTestId("api-access-inventory-table")).toBeInTheDocument();
    expect(screen.queryByTestId("api-access-legacy-keys")).not.toBeInTheDocument();
  });

  it("explains that revoking only ends the token's reach here and that its owner is emailed", async () => {
    calls.revokeGrant.mockResolvedValue(undefined);
    const user = await openInventory();
    await user.click(within(rowNamed("laptop")).getByTestId("api-access-inventory-revoke"));
    const dialog = screen.getByTestId("api-access-inventory-revoke-dialog");
    expect(dialog).toHaveTextContent("This ends the token's reach into Acme");
    expect(dialog).toHaveTextContent("The token itself isn't revoked");
    expect(dialog).toHaveTextContent("can't be given access here again");
    expect(dialog).toHaveTextContent("keeps working in every other organization");
    expect(dialog).toHaveTextContent("Ada Lovelace is emailed");
    await user.click(within(dialog).getByTestId("api-access-inventory-revoke-dialog-confirm"));
    expect(calls.revokeGrant).toHaveBeenCalledWith({ orgId: "org-1", tokenId: "t-pat" });
  });

  it("says an all-access token keeps working in every other org too", async () => {
    queries.inventory = loaded([
      inventoryToken({
        id: "t-all",
        name: "everything",
        kind: "personal",
        all_access: true,
        grants_here: [],
        owner: { type: "user", id: "u-1", label: "Ada Lovelace" }
      })
    ]);
    const user = await openInventory();
    await user.click(within(rowNamed("everything")).getByTestId("api-access-inventory-revoke"));
    // A block costs the token this org and nothing else: there is no all-access penalty to warn of.
    const dialog = screen.getByTestId("api-access-inventory-revoke-dialog");
    expect(dialog).toHaveTextContent("This ends the token's reach into Acme");
    expect(dialog).toHaveTextContent("keeps working in every other organization");
    expect(dialog).not.toHaveTextContent("chat, work, notifications");
  });

  it("offers nothing on an all-access token the org has already blocked", async () => {
    queries.inventory = loaded([
      inventoryToken({
        id: "t-blocked",
        name: "blocked",
        kind: "personal",
        all_access: true,
        owner: { type: "user", id: "u-1", label: "Ada Lovelace" },
        grants_here: [grant({ workspace_id: null, revoked_at: "2026-09-01T00:00:00Z" })]
      })
    ]);
    await openInventory();
    expect(
      within(rowNamed("blocked")).queryByTestId("api-access-inventory-revoke")
    ).not.toBeInTheDocument();
  });

  /// A disabled Revoke sat here while only the key's owner could end one. The
  /// removal ended every key, so there is nothing to offer and nothing to
  /// explain — Activity is the one thing left to do with the row.
  it("offers nothing but Activity on a removed legacy API key", async () => {
    await openInventory();
    const row = legacyRowNamed("old key");
    expect(within(row).getByTestId("api-access-legacy-key-activity")).toBeEnabled();
    expect(within(row).queryByTestId("api-access-inventory-revoke-legacy")).not.toBeInTheDocument();
    expect(within(row).queryByTestId("api-access-inventory-revoke")).not.toBeInTheDocument();
  });

  it("opens a service-account token's account instead of revoking it here", async () => {
    const user = await openInventory();
    await user.click(
      within(rowNamed("nightly sync")).getByTestId("api-access-inventory-open-account")
    );
    expect(screen.getByTestId("api-access-account-handle")).toHaveTextContent("acme/deployer");
  });

  it("says nothing reaches the org when the list is empty, and 'not available' on a 404", async () => {
    queries.inventory = loaded([]);
    await openInventory();
    expect(screen.getByTestId("api-access-inventory-empty")).toBeInTheDocument();
    cleanup();
    queries.inventory = failed(404);
    await openInventory();
    expect(screen.getByTestId("api-access-inventory-unavailable")).toBeInTheDocument();
  });
});

describe("the token policy", () => {
  const openPolicy = async () => {
    const user = show();
    await user.click(screen.getByTestId("api-access-tab-policy"));
    return user;
  };

  it("says a violating token is blocked for this org, not revoked, and that legacy API keys are exempt", async () => {
    await openPolicy();
    const panel = screen.getByTestId("api-access-policy");
    expect(panel).toHaveTextContent("blocked for Acme, not revoked");
    expect(panel).toHaveTextContent("Legacy API keys are exempt from every rule on this page");
  });

  it("reflects the saved policy and has nothing to save until something changes", async () => {
    queries.policy = loaded({
      max_lifetime_days: 90,
      allow_all_access_tokens: true,
      require_environment_on_trust_policies: true
    });
    await openPolicy();
    expect(screen.getByTestId("api-access-policy-limit-lifetime")).toBeChecked();
    expect(screen.getByTestId("api-access-policy-max-days")).toHaveValue("90");
    expect(screen.getByTestId("api-access-policy-allow-all-access")).toBeChecked();
    expect(screen.getByTestId("api-access-policy-require-environment")).toBeChecked();
    expect(screen.getByTestId("api-access-policy-save")).toBeDisabled();
  });

  it("spells out what a tighter rule does before saving it, then saves the whole policy", async () => {
    calls.savePolicy.mockResolvedValue(undefined);
    const user = await openPolicy();
    await user.click(screen.getByTestId("api-access-policy-allow-all-access"));
    expect(screen.getByTestId("api-access-policy-notes")).toHaveTextContent(
      /All-access personal tokens stop working in this organization/
    );
    await user.click(screen.getByTestId("api-access-policy-save"));
    expect(calls.savePolicy).toHaveBeenCalledWith({
      orgId: "org-1",
      policy: {
        max_lifetime_days: null,
        allow_all_access_tokens: false,
        require_environment_on_trust_policies: false
      }
    });
  });

  it("won't save a lifetime that isn't a whole number of days", async () => {
    const user = await openPolicy();
    await user.click(screen.getByTestId("api-access-policy-limit-lifetime"));
    const days = screen.getByTestId("api-access-policy-max-days");
    await user.clear(days);
    await user.type(days, "1.5");
    expect(screen.getByRole("alert")).toHaveTextContent("Use a whole number of days.");
    expect(screen.getByTestId("api-access-policy-save")).toBeDisabled();
  });

  it("is calm about a server that has no policy endpoint yet", async () => {
    queries.policy = failed(404);
    await openPolicy();
    expect(screen.getByTestId("api-access-policy-unavailable")).toBeInTheDocument();
    // The explanation still stands, even with nothing to edit.
    expect(screen.getByTestId("api-access-policy")).toHaveTextContent("not revoked");
  });
});
