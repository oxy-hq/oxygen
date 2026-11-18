// @vitest-environment jsdom

import { act, cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AxiosError, type AxiosResponse } from "axios";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ServiceAccountMintPreview } from "@/types/apiToken";
import CliAuth from ".";
import { parseCliAuthRequest, returnToUrl } from "./cliAuthRequest";
import type { CliSession } from "./cliSession";

/**
 * `/cli-auth` with `kind=service_account`: the approval `oxyc tokens create
 * --org-service-account` opens. What is pinned here: the sheet lays out what the server's preview
 * says will be created — the organization, the account new or existing with its role, the
 * token's name, its lifetime and its reach — and never its own reading of the link; a request
 * the server refuses is shown refused, with its reason, and cannot be approved; and an approval
 * sends the mint and hands oxyc a code, never the session.
 */

vi.setConfig({ testTimeout: 20000 });

const CHALLENGE = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
const PKCE_URL = `/cli-auth?port=53124&state=nonce&code_challenge=${CHALLENGE}&hostname=ops-laptop`;
const accountUrl = (
  query = "org=poke-house&account=store-ops-tablets&role=admin&days=90&name=tablets"
) => `${PKCE_URL}&kind=service_account&${query}`;

const leaveTo = vi.fn();
const session: CliSession = { token: "session.jwt", email: "luong@example.com" };
vi.mock("./cliSession", () => ({
  leaveTo: (url: string) => leaveTo(url),
  readCliSession: () => Promise.resolve(session)
}));

type MutateOptions = { onSuccess?: (data: { code: string }) => void; onError?: (e: Error) => void };
const mutate = vi.fn();
vi.mock("@/hooks/api/cliAuth/useAuthorizeCli", () => ({
  default: () => ({ mutate, isPending: false, isError: false, error: null })
}));

type PreviewQuery = {
  data?: ServiceAccountMintPreview;
  isError: boolean;
  error: Error | null;
  refetch: () => void;
};
const PREVIEW: ServiceAccountMintPreview = {
  org: { id: "o-1", slug: "poke-house", name: "Poke House" },
  account: { name: "store-ops-tablets", role: "admin", exists: false },
  token: { name: "tablets", expires_in_days: 90, expires_at: "2027-02-16T12:00:00Z" },
  grant: { org_wide: true, role_ceiling: "admin" }
};
let preview: PreviewQuery;
const previewAsked = vi.fn();
vi.mock("@/hooks/api/cliAuth/useServiceAccountPreview", () => ({
  default: (hostname: string, mint: unknown) => {
    previewAsked(hostname, mint);
    return preview;
  }
}));

const pageFocused = vi.spyOn(document, "hasFocus");

beforeEach(() => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  pageFocused.mockReturnValue(true);
  preview = { data: PREVIEW, isError: false, error: null, refetch: vi.fn() };
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  leaveTo.mockReset();
  mutate.mockReset();
  previewAsked.mockReset();
});

const open = (url: string) =>
  render(
    <MemoryRouter initialEntries={[url]}>
      <CliAuth />
    </MemoryRouter>
  );

const httpError = (status: number, data: unknown = {}) =>
  new AxiosError("Request failed", "ERR", undefined, undefined, { status, data } as AxiosResponse);

const attend = async (ms = 1000) => {
  await act(async () => {});
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
};

const sheet = () => screen.getByTestId("cli-auth-card");

describe("/cli-auth with kind=service_account", () => {
  it("shows what the server says approving creates: org, account, role, token, lifetime, reach", async () => {
    open(accountUrl());
    expect(await screen.findByTestId("cli-auth-hostname")).toHaveTextContent("ops-laptop");
    expect(within(sheet()).getByRole("heading", { level: 1 })).toHaveTextContent(
      "Approve an organization's service account token?"
    );
    expect(sheet()).toHaveTextContent("the token acts as the account, not as you");
    expect(screen.getByTestId("cli-auth-service-account-notice")).toHaveTextContent(
      "This token belongs to the organization"
    );
    expect(screen.getByTestId("cli-auth-sa-org")).toHaveTextContent("Poke House poke-house");
    expect(screen.getByTestId("cli-auth-sa-account")).toHaveTextContent("store-ops-tablets admin");
    expect(screen.getByTestId("cli-auth-sa-account-state")).toHaveTextContent(
      "A new account. Approving creates it."
    );
    expect(screen.getByTestId("cli-auth-mint-name")).toHaveTextContent("tablets");
    expect(screen.getByTestId("cli-auth-mint-lifetime")).toHaveTextContent(/^90 days/);
    expect(screen.getByTestId("cli-auth-mint-lifetime")).toHaveTextContent(/until \w+ \d+, \d{4}/);
    expect(screen.getByTestId("cli-auth-sa-reach")).toHaveTextContent(
      "All of Poke House, as an admin"
    );
    // An admin's token is said to reach member and invitation management, in those words.
    const [can] = within(screen.getByTestId("cli-auth-mint-powers")).getAllByRole("list");
    expect(
      within(can as HTMLElement)
        .getAllByRole("listitem")
        .map((act) => act.textContent)
    ).toEqual([
      "Do what an admin of this organization can do through the API, over all of it",
      "Change members' roles, remove members and send invitations",
      "Be kept as a custom app's secret and used by its functions"
    ]);
    expect(screen.getByTestId("cli-auth-mint-command")).toHaveTextContent(
      /^oxyc tokens create --org-service-account poke-house\/store-ops-tablets --role admin$/
    );

    // The server is asked about exactly what the link asked for.
    expect(previewAsked).toHaveBeenCalledWith("ops-laptop", {
      kind: "service_account",
      org: "poke-house",
      account: "store-ops-tablets",
      role: "admin",
      name: "tablets",
      expires_in_days: 90
    });
  });

  it("does not list member and invitation management for a member account's token", async () => {
    preview = {
      ...preview,
      data: {
        ...PREVIEW,
        account: { ...PREVIEW.account, role: "member" },
        grant: { org_wide: true, role_ceiling: "member" }
      }
    };
    open(accountUrl("org=poke-house&account=store-ops-tablets"));
    const powers = await screen.findByTestId("cli-auth-mint-powers");
    expect(powers).toHaveTextContent("Do what a member of this organization can do");
    expect(powers).not.toHaveTextContent("remove members");
    expect(screen.getByTestId("cli-auth-sa-reach")).toHaveTextContent(
      "All of Poke House, as a member"
    );
  });

  it("says when the account already exists, and that nothing else about it changes", async () => {
    preview = {
      ...preview,
      data: { ...PREVIEW, account: { ...PREVIEW.account, exists: true } }
    };
    open(accountUrl());
    expect(await screen.findByTestId("cli-auth-sa-account-state")).toHaveTextContent(
      "An existing account. Approving adds a token to it and changes nothing else."
    );
  });

  it("arms Approve after a second, then sends the mint and hands oxyc only a code", async () => {
    open(accountUrl());
    const approve = await screen.findByTestId("cli-auth-confirm");
    expect(approve).toBeDisabled();
    await attend();
    expect(approve).toBeEnabled();

    const user = userEvent.setup({ delay: null });
    await user.click(approve);
    expect(mutate).toHaveBeenCalledTimes(1);
    const [body, options] = mutate.mock.calls[0] as [unknown, MutateOptions];
    expect(body).toEqual({
      code_challenge: CHALLENGE,
      hostname: "ops-laptop",
      mint: {
        kind: "service_account",
        org: "poke-house",
        account: "store-ops-tablets",
        role: "admin",
        name: "tablets",
        expires_in_days: 90
      }
    });

    act(() => options.onSuccess?.({ code: "single-use" }));
    expect(leaveTo).toHaveBeenCalledWith(
      "http://127.0.0.1:53124/callback?code=single-use&state=nonce"
    );
    expect(leaveTo.mock.calls.flat().join(" ")).not.toContain("session.jwt");
    expect(sheet()).toHaveAttribute("data-status", "done");
  });

  it("shows a refusal from the server with its reason, and never arms Approve", async () => {
    preview = {
      data: undefined,
      isError: true,
      error: httpError(409, {
        code: "role_mismatch",
        error:
          "the service account 'store-ops-tablets' already exists as member: ask for that role, or change the account's role under Settings → Organization → API tokens → Service accounts"
      }),
      refetch: vi.fn()
    };
    open(accountUrl());
    const problem = await screen.findByTestId("cli-auth-sa-problem");
    // The server's reason, as a sentence, and what fixes it: running oxyc again.
    expect(problem).toHaveTextContent(
      /^The service account 'store-ops-tablets' already exists as member/
    );
    expect(problem).toHaveTextContent("Run oxyc again once it is put right.");
    // What was asked stays on the page, as oxyc sent it.
    expect(screen.getByTestId("cli-auth-sa-asked")).toHaveTextContent(
      "Organizationpoke-houseAccountstore-ops-tablets admin"
    );
    expect(within(sheet()).getByRole("heading", { level: 1 })).toHaveTextContent(
      "This request can't be approved as it stands"
    );
    await attend(3000);
    expect(screen.getByTestId("cli-auth-confirm")).toBeDisabled();
    expect(screen.queryByTestId("cli-auth-service-account")).not.toBeInTheDocument();
    expect(mutate).not.toHaveBeenCalled();
  });

  it("refuses a member who may not manage the organization's service accounts", async () => {
    preview = {
      data: undefined,
      isError: true,
      error: httpError(403, {
        code: "org_admin_required",
        error:
          "only an owner or admin of the organization may create its service accounts and their tokens"
      }),
      refetch: vi.fn()
    };
    open(accountUrl());
    const problem = await screen.findByTestId("cli-auth-sa-problem");
    expect(problem).toHaveTextContent(
      "Only an owner or admin of the organization may create its service accounts and their tokens."
    );
    // Running oxyc again gives nobody a role: it says who can approve instead.
    expect(problem).toHaveTextContent("Ask one of them to approve it.");
    expect(problem).not.toHaveTextContent("Run oxyc again");
    await attend(3000);
    expect(screen.getByTestId("cli-auth-confirm")).toBeDisabled();
  });

  it("refuses an owner role or a lifetime that is no number without asking the server", async () => {
    open(accountUrl("org=poke-house&account=store-ops-tablets&role=owner"));
    expect(await screen.findByTestId("cli-auth-sa-problem")).toHaveTextContent('never "owner"');
    expect(previewAsked).toHaveBeenLastCalledWith("ops-laptop", null);
    cleanup();
    open(accountUrl("org=poke-house&account=store-ops-tablets&days=soon"));
    expect(await screen.findByTestId("cli-auth-sa-problem")).toHaveTextContent(
      "not a whole number of days"
    );
    await attend(3000);
    expect(screen.getByTestId("cli-auth-confirm")).toBeDisabled();
  });

  it("refuses a link that mixes in another kind's params", () => {
    expect(parseCliAuthRequest(new URLSearchParams(accountUrl().split("?")[1])).kind).toBe(
      "service_account_mint"
    );
    for (const extra of ["apps=acme/store", "standing=1", "hours=8"]) {
      const params = new URLSearchParams(`${accountUrl().split("?")[1]}&${extra}`);
      expect(parseCliAuthRequest(params).kind, extra).toBe("invalid");
    }
    const agentWithAccount = new URLSearchParams(
      `${PKCE_URL.split("?")[1]}&kind=agent&hours=8&account=store-ops`
    );
    expect(parseCliAuthRequest(agentWithAccount).kind).toBe("invalid");
    const noAccount = new URLSearchParams(`${PKCE_URL.split("?")[1]}&kind=service_account&org=x`);
    expect(parseCliAuthRequest(noAccount).kind).toBe("invalid");
  });

  it("comes back to the same request after signing in", () => {
    const request = parseCliAuthRequest(new URLSearchParams(accountUrl().split("?")[1]));
    if (request.kind !== "service_account_mint") throw new Error(request.kind);
    const back = new URL(returnToUrl("https://app.example", request));
    const again = parseCliAuthRequest(back.searchParams);
    expect(again).toEqual(request);
  });
});
