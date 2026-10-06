// @vitest-environment jsdom

import { act, cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AxiosError, type AxiosResponse } from "axios";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { SandboxApp, TokenOptions, TokenWithSecret } from "@/types/apiToken";
import CreateTokenDialog from "./CreateTokenDialog";
import TokenSecretDialog, { exportSnippet } from "./TokenSecretDialog";

vi.setConfig({ testTimeout: 20000 });

const mutate = vi.fn();
const mint = vi.fn();
let options: TokenOptions;

// The hooks are the seam: this file is about which body the dialog sends.
vi.mock("@/hooks/api/userTokens/useUserTokenMutations", () => ({
  useCreateUserToken: () => ({ mutate, isPending: false }),
  useCreateSandboxAgentToken: () => ({ mutate: mint, isPending: false })
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
  mint.mockReset();
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

const sandboxApp = (over: Partial<SandboxApp>): SandboxApp => ({
  id: "a1",
  org_id: "o1",
  org_slug: "acme",
  org_name: "Acme",
  slug: "store-ops",
  name: "Store Ops",
  ...over
});

const GLOBEX = { org_id: "o2", org_slug: "globex", org_name: "Globex" };

const SANDBOX_APPS: SandboxApp[] = [
  sandboxApp({}),
  sandboxApp({ id: "a2", slug: "refunds", name: "Refunds" }),
  sandboxApp({ id: "a3", slug: "labor", name: "Labor" }),
  sandboxApp({ id: "a4", ...GLOBEX, slug: "pos", name: "POS" }),
  sandboxApp({ id: "a5", ...GLOBEX, slug: "inventory", name: "Inventory" }),
  sandboxApp({ id: "a6", ...GLOBEX, slug: "payroll", name: "Payroll" })
];

const SANDBOX: Partial<TokenOptions> = {
  sandbox_agent: { default_hours: 8, max_hours: 168, max_apps: 5 },
  sandbox_apps: SANDBOX_APPS
};

/** The dialog with the sandbox agent type already chosen, and "laptop" typed as the name. */
const openSandbox = async (over: Partial<TokenOptions> = {}) => {
  const user = await open({ ...SANDBOX, ...over });
  await user.click(screen.getByTestId("account-token-access-sandbox"));
  return user;
};

const appRow = (name: string) =>
  within(
    screen
      .getAllByTestId("account-token-sandbox-app")
      .find((row) => row.getAttribute("data-app-name") === name) as HTMLElement
  );

const tick = (user: ReturnType<typeof userEvent.setup>, name: string) =>
  user.click(appRow(name).getByTestId("account-token-sandbox-app-checkbox"));

const refuse = (status: number, data: unknown) => {
  const [, handlers] = mint.mock.calls[0] as [unknown, { onError: (error: Error) => void }];
  act(() =>
    handlers.onError(
      new AxiosError("Request failed", "ERR", undefined, undefined, {
        status,
        data
      } as AxiosResponse)
    )
  );
};

describe("CreateTokenDialog, the sandbox agent type", () => {
  it("isn't offered to someone with no app to mint for, or by a server that predates it", async () => {
    await open();
    expect(screen.queryByTestId("account-token-access-sandbox")).not.toBeInTheDocument();
    cleanup();

    await open({ ...SANDBOX, sandbox_apps: [] });
    expect(screen.queryByTestId("account-token-access-sandbox")).not.toBeInTheDocument();
  });

  it("is a third type beside the two access modes, and changes nothing until it is chosen", async () => {
    await open(SANDBOX);
    expect(screen.getByTestId("account-token-access-sandbox")).toHaveTextContent("Sandbox agent");
    expect(within(screen.getByTestId("account-token-access-all")).getByRole("radio")).toBeChecked();
    expect(screen.getByTestId("account-token-expiry-90")).toBeInTheDocument();
    expect(screen.queryByTestId("account-token-sandbox-fields")).not.toBeInTheDocument();
  });

  it("says what the token can and can't do, and swaps days for hours and standing for apps", async () => {
    await openSandbox({ can_platform: true, can_partner: true });
    // Two sentences, each beside its own label.
    const [can, cannot] = within(screen.getByTestId("account-token-sandbox-summary")).getAllByRole(
      "definition"
    );
    expect(can).toHaveTextContent(/^Create up to three dev sandboxes of the apps you pick, /);
    expect(cannot).toHaveTextContent(/^Reach production or staging, .* or touch any other app\.$/);
    expect(screen.getByTestId("account-token-sandbox-summary")).toHaveTextContent("Cannot");

    expect(screen.queryByTestId("account-token-expiry-90")).not.toBeInTheDocument();
    expect(within(screen.getByTestId("account-token-lifetime-8")).getByRole("radio")).toBeChecked();
    expect(screen.getByTestId("account-token-lifetime-outcome")).toHaveTextContent(
      "It can't be extended"
    );
    // A sandbox agent token carries no standing, whatever the caller holds.
    expect(screen.queryByTestId("account-token-platform")).not.toBeInTheDocument();
    expect(screen.queryByTestId("account-token-partner")).not.toBeInTheDocument();
  });

  it("needs an app, then sends the pinned body with the default 8 hours", async () => {
    const user = await openSandbox();
    expect(screen.getByTestId("account-token-submit")).toBeDisabled();

    await tick(user, "Store Ops");
    await tick(user, "POS");
    await user.click(screen.getByTestId("account-token-submit"));
    expect(mint.mock.calls[0][0]).toEqual({
      name: "laptop",
      kind: "sandbox_agent",
      apps: ["a1", "a4"],
      expires_in_hours: 8
    });
    // The personal-token create is a different request, and is not made.
    expect(mutate).not.toHaveBeenCalled();
  });

  it("groups the apps by organization and narrows them by search", async () => {
    const user = await openSandbox();
    expect(
      screen.getAllByTestId("account-token-sandbox-org").map((org) => org.dataset.orgName)
    ).toEqual(["Acme", "Globex"]);
    expect(appRow("Store Ops").getByText("acme/store-ops")).toBeInTheDocument();

    const search = screen.getByTestId("account-token-sandbox-search");
    await user.type(search, "globex/p");
    expect(
      screen.getAllByTestId("account-token-sandbox-app").map((row) => row.dataset.appName)
    ).toEqual(["Payroll", "POS"]);

    await user.clear(search);
    await user.type(search, "nothing like it");
    expect(screen.getByTestId("account-token-sandbox-no-match")).toHaveTextContent(
      'No app or organization matches "nothing like it".'
    );
  });

  it("takes at most five apps", async () => {
    const user = await openSandbox();
    for (const name of ["Store Ops", "Refunds", "Labor", "POS", "Inventory"]) {
      await tick(user, name);
    }
    expect(appRow("Payroll").getByTestId("account-token-sandbox-app-checkbox")).toBeDisabled();
    expect(screen.getByTestId("account-token-sandbox-count")).toHaveTextContent(
      "5 apps picked, the most one token covers."
    );

    // Unticking one frees the place.
    await tick(user, "Labor");
    expect(appRow("Payroll").getByTestId("account-token-sandbox-app-checkbox")).toBeEnabled();
  });

  it("names an app by its reference, the org set back once the app is picked", async () => {
    const user = await openSandbox();
    expect(appRow("Store Ops").getByText("acme/store-ops")).toBeInTheDocument();
    await tick(user, "Store Ops");
    expect(appRow("Store Ops").getByText("acme/")).toBeInTheDocument();
    expect(appRow("Store Ops").getByText("store-ops")).toBeInTheDocument();
    expect(screen.getByTestId("account-token-sandbox-count")).toHaveTextContent(
      "1 app picked, of up to 5."
    );
  });
});

const activeApp = () =>
  screen
    .queryAllByTestId("account-token-sandbox-app")
    .filter((row) => row.dataset.active === "true")
    .map((row) => row.dataset.appName);

describe("CreateTokenDialog, from the keyboard", () => {
  afterEach(() => {
    // Back to jsdom's own platform, which is no Apple one.
    Reflect.deleteProperty(window.navigator, "platform");
  });
  const onPlatform = (platform: string) =>
    Object.defineProperty(window.navigator, "platform", { value: platform, configurable: true });

  it("moves a highlight down the apps from the search field, and picks with Enter", async () => {
    const user = await openSandbox();
    // No row is marked until the search field has the keyboard.
    expect(activeApp()).toEqual([]);

    await user.click(screen.getByTestId("account-token-sandbox-search"));
    expect(activeApp()).toEqual(["Labor"]);
    await user.keyboard("{ArrowDown}{ArrowDown}");
    expect(activeApp()).toEqual(["Store Ops"]);
    await user.keyboard("{ArrowUp}");
    expect(activeApp()).toEqual(["Refunds"]);

    await user.keyboard("{Enter}");
    expect(appRow("Refunds").getByTestId("account-token-sandbox-app-checkbox")).toBeChecked();
    // Enter picked an app: it did not send the form.
    expect(mint).not.toHaveBeenCalled();
    expect(mutate).not.toHaveBeenCalled();

    // Enter again unpicks it.
    await user.keyboard("{Enter}");
    expect(appRow("Refunds").getByTestId("account-token-sandbox-app-checkbox")).not.toBeChecked();
  });

  it("stops at the ends of the list, and starts over from the top match as the search changes", async () => {
    const user = await openSandbox();
    const search = screen.getByTestId("account-token-sandbox-search");
    await user.click(search);
    await user.keyboard("{ArrowUp}");
    expect(activeApp()).toEqual(["Labor"]);
    await user.keyboard(
      "{ArrowDown}{ArrowDown}{ArrowDown}{ArrowDown}{ArrowDown}{ArrowDown}{ArrowDown}"
    );
    expect(activeApp()).toEqual(["POS"]);

    await user.type(search, "globex");
    expect(activeApp()).toEqual(["Inventory"]);
    await user.keyboard("{Enter}");
    expect(appRow("Inventory").getByTestId("account-token-sandbox-app-checkbox")).toBeChecked();

    // Nothing matches, so there is nothing for Enter to pick, and still nothing is sent.
    await user.type(search, " nothing");
    expect(activeApp()).toEqual([]);
    await user.keyboard("{Enter}");
    expect(mint).not.toHaveBeenCalled();
  });

  it("won't pick past the limit with Enter, as a click won't", async () => {
    const user = await openSandbox();
    for (const name of ["Store Ops", "Refunds", "Labor", "POS", "Inventory"]) {
      await tick(user, name);
    }
    const search = screen.getByTestId("account-token-sandbox-search");
    await user.type(search, "payroll");
    expect(activeApp()).toEqual(["Payroll"]);
    await user.keyboard("{Enter}");
    expect(appRow("Payroll").getByTestId("account-token-sandbox-app-checkbox")).not.toBeChecked();
  });

  it("creates on Command+Enter on a Mac, from any field, and shows that chord", async () => {
    onPlatform("MacIntel");
    const user = await openSandbox();
    const submit = screen.getByTestId("account-token-submit");
    expect(submit).toHaveAttribute("aria-keyshortcuts", "Meta+Enter");
    // Nothing to create yet: no chord is offered, and the chord does nothing.
    expect(within(submit).queryByTestId("submit-chord-hint")).not.toBeInTheDocument();
    await user.keyboard("{Meta>}{Enter}{/Meta}");
    expect(mint).not.toHaveBeenCalled();

    await tick(user, "Store Ops");
    expect(within(submit).getByTestId("submit-chord-hint")).not.toHaveTextContent("Ctrl");
    // The hint is decoration: the button is still named "Create token".
    expect(screen.getByRole("button", { name: "Create token" })).toBe(submit);

    await user.click(screen.getByTestId("account-token-sandbox-search"));
    // Control is another platform's chord. In the search field it is not Enter's pick either.
    await user.keyboard("{Control>}{Enter}{/Control}");
    expect(mint).not.toHaveBeenCalled();

    await user.keyboard("{Meta>}{Enter}{/Meta}");
    expect(mint).toHaveBeenCalledTimes(1);
    expect(mint.mock.calls[0][0]).toEqual({
      name: "laptop",
      kind: "sandbox_agent",
      apps: ["a1"],
      expires_in_hours: 8
    });
    // The chord sent the form: it did not also pick the highlighted app.
    expect(appRow("Labor").getByTestId("account-token-sandbox-app-checkbox")).not.toBeChecked();
  });

  it("creates a personal token on Control+Enter off a Mac, and shows Ctrl", async () => {
    onPlatform("Win32");
    const user = await open();
    const submit = screen.getByTestId("account-token-submit");
    expect(submit).toHaveAttribute("aria-keyshortcuts", "Control+Enter");
    expect(within(submit).getByTestId("submit-chord-hint")).toHaveTextContent("Ctrl");

    // From the name field, where a bare Enter already sends the form: the chord sends it once,
    // not once for the chord and once more for the field's own Enter.
    await user.keyboard("{Control>}{Enter}{/Control}");
    expect(mutate).toHaveBeenCalledTimes(1);
    expect(mutate.mock.calls[0][0]).toMatchObject({ name: "laptop", all_access: true });
    expect(mint).not.toHaveBeenCalled();
  });

  it("keeps its buttons outside the part that scrolls, so a short window never hides them", async () => {
    await openSandbox();
    const fields = screen.getByTestId("account-token-fields");
    expect(fields).toContainElement(screen.getByTestId("account-token-name"));
    expect(fields).toContainElement(screen.getByTestId("account-token-sandbox-picker"));
    expect(fields).toContainElement(screen.getByTestId("account-token-lifetime-8"));
    expect(fields).not.toContainElement(screen.getByTestId("account-token-submit"));
    expect(fields).not.toContainElement(screen.getByTestId("account-token-cancel"));
  });

  it("shows Esc on Cancel, which closes the dialog", async () => {
    await open();
    const cancel = screen.getByTestId("account-token-cancel");
    expect(cancel).toHaveAttribute("aria-keyshortcuts", "Escape");
    expect(cancel).toHaveTextContent("Esc");
    expect(screen.getByRole("button", { name: "Cancel" })).toBe(cancel);
  });

  it("sends the preset picked, or the hours typed", async () => {
    const user = await openSandbox();
    await tick(user, "Store Ops");
    await user.click(screen.getByTestId("account-token-lifetime-72"));
    await user.click(screen.getByTestId("account-token-submit"));
    expect(mint.mock.calls[0][0]).toMatchObject({ expires_in_hours: 72 });

    // Custom opens on the span already picked, so the box is never empty.
    await user.click(screen.getByTestId("account-token-lifetime-custom"));
    const hours = screen.getByTestId("account-token-lifetime-custom-input");
    expect(hours).toHaveValue("72");
    await user.clear(hours);
    await user.type(hours, "12");
    await user.click(screen.getByTestId("account-token-submit"));
    expect(mint.mock.calls[1][0]).toMatchObject({ expires_in_hours: 12 });
  });

  it("won't send a lifetime the server would refuse", async () => {
    const user = await openSandbox();
    await tick(user, "Store Ops");
    await user.click(screen.getByTestId("account-token-lifetime-custom"));
    const hours = screen.getByTestId("account-token-lifetime-custom-input");
    await user.clear(hours);
    await user.type(hours, "500");
    expect(screen.getByTestId("account-token-lifetime-problem")).toHaveTextContent(
      "A sandbox agent token lasts at most 168 hours (7 days)."
    );
    expect(screen.getByTestId("account-token-submit")).toBeDisabled();
  });

  it("names the app a 404 refuses, beside the picker, until the picks change", async () => {
    const user = await openSandbox();
    await tick(user, "Store Ops");
    await user.click(screen.getByTestId("account-token-submit"));
    refuse(404, { code: "app_not_found", app_id: "a1" });
    expect(screen.getByTestId("account-token-sandbox-error")).toHaveTextContent(
      "Oxygen couldn't find Store Ops in Acme for you."
    );
    expect(screen.getByTestId("account-token-dialog")).toBeInTheDocument();

    await tick(user, "Refunds");
    expect(screen.queryByTestId("account-token-sandbox-error")).not.toBeInTheDocument();
  });

  it("words a 400 by what a sandbox agent token may ask for", async () => {
    const user = await openSandbox();
    await tick(user, "Store Ops");
    await user.click(screen.getByTestId("account-token-submit"));
    refuse(400, { code: "invalid_sandbox_token", message: "apps: duplicate id" });
    expect(screen.getByTestId("account-token-sandbox-error")).toHaveTextContent(
      "A sandbox agent token names 1 to 5 apps and lasts 1 to 168 hours."
    );
  });

  it("says in the server's words when an organization's lifetime cap refuses the mint", async () => {
    const user = await openSandbox();
    await tick(user, "Store Ops");
    await user.click(screen.getByTestId("account-token-submit"));
    // The body `TokenError::ExceedsPolicy` answers: the sentence is `error`, and there is no `message`.
    refuse(400, {
      error:
        "the expiry is past the 3-day token lifetime an organization this token reaches allows",
      code: "exceeds_policy",
      max_lifetime_days: 3
    });
    expect(screen.getByTestId("account-token-sandbox-error")).toHaveTextContent(
      "the expiry is past the 3-day token lifetime an organization this token reaches allows"
    );
    expect(screen.getByTestId("account-token-dialog")).toBeInTheDocument();
  });

  it("stops the name at the server's 100 characters, for either type", async () => {
    await openSandbox();
    // Past it a mint answers 400 `invalid_sandbox_token`, which the dialog words by apps and hours.
    expect(screen.getByTestId("account-token-name")).toHaveAttribute("maxlength", "100");
  });

  it("closes and hands the secret up once the token is minted", async () => {
    const onOpenChange = vi.fn();
    const onCreated = vi.fn();
    options = { ...baseOptions(), ...SANDBOX };
    const user = userEvent.setup({ delay: null });
    render(<CreateTokenDialog open onOpenChange={onOpenChange} onCreated={onCreated} />);
    await user.type(screen.getByTestId("account-token-name"), "refunds task");
    await user.click(screen.getByTestId("account-token-access-sandbox"));
    await tick(user, "Refunds");
    await user.click(screen.getByTestId("account-token-submit"));

    const minted = { secret: "oxy_sbx_SECRET", token: { name: "refunds task" } } as TokenWithSecret;
    const [, handlers] = mint.mock.calls[0] as [
      unknown,
      { onSuccess: (m: TokenWithSecret) => void }
    ];
    act(() => handlers.onSuccess(minted));
    expect(onOpenChange).toHaveBeenCalledWith(false);
    expect(onCreated).toHaveBeenCalledWith(minted);
  });

  it("sends a personal token again once an access mode is picked back", async () => {
    const user = await openSandbox();
    await tick(user, "Store Ops");
    await user.click(screen.getByTestId("account-token-access-all"));
    expect(screen.queryByTestId("account-token-sandbox-fields")).not.toBeInTheDocument();
    await user.click(screen.getByTestId("account-token-submit"));

    expect(mint).not.toHaveBeenCalled();
    expect(sentBody()).toEqual({
      name: "laptop",
      all_access: true,
      platform: false,
      partner: false,
      grants: [],
      expires_in_days: 90
    });
  });
});

describe("TokenSecretDialog", () => {
  it("shows a sandbox agent token's secret with the export line, and no talk of regenerating", () => {
    render(
      <TokenSecretDialog
        onDone={() => {}}
        reveal={{
          reason: "created",
          secret: "oxy_sbx_SECRET",
          token: { name: "refunds task", kind: "sandbox_agent" } as never
        }}
      />
    );
    expect(screen.getByTestId("account-token-secret")).toHaveTextContent("oxy_sbx_SECRET");
    expect(screen.getByTestId("account-token-export")).toHaveTextContent(
      "export OXY_TOKEN=oxy_sbx_SECRET"
    );
    const dialog = screen.getByTestId("account-token-secret-dialog");
    expect(dialog).toHaveTextContent("revoke this token and create another");
    expect(dialog).not.toHaveTextContent("regenerate");
    expect(dialog).toHaveTextContent("Or set it in the agent's environment");
  });

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
