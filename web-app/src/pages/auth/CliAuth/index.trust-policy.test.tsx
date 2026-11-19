// @vitest-environment jsdom

import { act, cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AxiosError, type AxiosResponse } from "axios";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TrustPolicyMintPreview } from "@/types/apiToken";
import CliAuth from ".";
import { parseCliAuthRequest, returnToUrl } from "./cliAuthRequest";
import type { CliSession } from "./cliSession";

/**
 * `/cli-auth` with `kind=trust_policy`: the approval `oxyc init-ci` opens. What is pinned here:
 * the sheet lays out what the server's preview says will be registered — the organization, the
 * account new or existing, the repository as GitHub names it, the workflow, the environment and
 * the one app it may publish — and never its own reading of the link; a refusal is shown with
 * its reason and never arms; an approval sends the mint and hands oxyc only a code; and the
 * page never says a token was made, because none is.
 */

vi.setConfig({ testTimeout: 20000 });

const CHALLENGE = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
const PKCE_URL = `/cli-auth?port=53124&state=nonce&code_challenge=${CHALLENGE}&hostname=dev-laptop`;
const APP = "0b0e5a10-1111-4222-8333-944455556666";
const ASK = `org=org-1&account=deployer&create=1&app=${APP}&repository=acme-co%2Facme-apps&workflow=.github%2Fworkflows%2Foxy-publish.yml&environment=oxy-publish`;
const policyUrl = (query = ASK) => `${PKCE_URL}&kind=trust_policy&${query}`;

const leaveTo = vi.fn();
const session: CliSession = { token: "session.jwt", email: "ada@acme.test" };
vi.mock("./cliSession", () => ({
  leaveTo: (url: string) => leaveTo(url),
  readCliSession: () => Promise.resolve(session)
}));

type MutateOptions = { onSuccess?: (data: { code: string }) => void; onError?: (e: Error) => void };
const mutate = vi.fn();
vi.mock("@/hooks/api/cliAuth/useAuthorizeCli", () => ({
  default: () => ({ mutate, isPending: false, isError: false, error: null })
}));

const PREVIEW: TrustPolicyMintPreview = {
  org: { id: "org-1", slug: "acme", name: "Acme" },
  account: { name: "deployer", role: "member", exists: false },
  app: { id: APP, slug: "sales", name: "Sales" },
  policy: {
    repository: "acme-co/acme-apps",
    workflow_path: ".github/workflows/oxy-publish.yml",
    environment: "oxy-publish",
    exists: false
  }
};
type PreviewQuery = {
  data?: TrustPolicyMintPreview;
  isError: boolean;
  error: Error | null;
  refetch: () => void;
};
let preview: PreviewQuery;
const previewAsked = vi.fn();
vi.mock("@/hooks/api/cliAuth/useTrustPolicyPreview", () => ({
  default: (mint: unknown) => {
    previewAsked(mint);
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

const MINT = {
  kind: "trust_policy",
  org: "org-1",
  account: "deployer",
  create_account: true,
  app: APP,
  repository: "acme-co/acme-apps",
  workflow_path: ".github/workflows/oxy-publish.yml",
  environment: "oxy-publish"
};

describe("/cli-auth with kind=trust_policy", () => {
  it("shows what the server says approving registers: org, account, run and the one grant", async () => {
    open(policyUrl());
    expect(await screen.findByTestId("cli-auth-hostname")).toHaveTextContent("dev-laptop");
    expect(within(sheet()).getByRole("heading", { level: 1 })).toHaveTextContent(
      "Let a GitHub Actions workflow publish this app?"
    );
    expect(sheet()).toHaveTextContent("No token is created.");
    expect(screen.getByTestId("cli-auth-trust-policy-notice")).toHaveTextContent(
      "Runs of this GitHub Actions workflow will publish as the account"
    );
    expect(screen.getByTestId("cli-auth-tp-org")).toHaveTextContent("Acme acme");
    expect(screen.getByTestId("cli-auth-tp-account")).toHaveTextContent("deployer member");
    expect(screen.getByTestId("cli-auth-tp-account-state")).toHaveTextContent(
      "A new account. Approving creates it."
    );
    expect(screen.getByTestId("cli-auth-tp-repository")).toHaveTextContent("acme-co/acme-apps");
    expect(screen.getByTestId("cli-auth-tp-workflow")).toHaveTextContent(
      ".github/workflows/oxy-publish.yml"
    );
    expect(screen.getByTestId("cli-auth-tp-environment")).toHaveTextContent("oxy-publish");
    expect(screen.getByTestId("cli-auth-tp-grant")).toHaveTextContent("Publish Sales acme/sales");
    expect(screen.getByTestId("cli-auth-tp-grant")).toHaveTextContent("And nothing else.");
    // The app the server resolved: whoever ran it may be a member asking an admin.
    expect(screen.getByTestId("cli-auth-mint-command")).toHaveTextContent(
      /^oxyc init-ci --app acme\/sales$/
    );
    expect(previewAsked).toHaveBeenCalledWith(MINT);
  });

  it("says when the account and the policy are already there", async () => {
    preview = {
      ...preview,
      data: {
        ...PREVIEW,
        account: { ...PREVIEW.account, exists: true },
        policy: { ...PREVIEW.policy, exists: true }
      }
    };
    open(policyUrl());
    expect(await screen.findByTestId("cli-auth-tp-account-state")).toHaveTextContent(
      "An existing account. Approving changes nothing about it."
    );
    expect(screen.getByTestId("cli-auth-tp-policy-state")).toHaveTextContent(
      "Already trusted: approving writes nothing."
    );
  });

  it("arms after a second, sends the mint, and hands oxyc only a code — never a token", async () => {
    open(policyUrl());
    const approve = await screen.findByTestId("cli-auth-confirm");
    expect(approve).toBeDisabled();
    await attend();
    expect(approve).toBeEnabled();
    await userEvent.setup({ delay: null }).click(approve);
    expect(mutate).toHaveBeenCalledTimes(1);
    const [body, options] = mutate.mock.calls[0] as [unknown, MutateOptions];
    expect(body).toEqual({ code_challenge: CHALLENGE, hostname: "dev-laptop", mint: MINT });

    act(() => options.onSuccess?.({ code: "single-use" }));
    expect(leaveTo).toHaveBeenCalledWith(
      "http://127.0.0.1:53124/callback?code=single-use&state=nonce"
    );
    expect(sheet()).toHaveAttribute("data-status", "done");
    expect(sheet()).toHaveTextContent("oxyc finishes the workflow");
    expect(sheet()).not.toHaveTextContent("token");
  });

  it("refuses for a plain member, saying who can approve, and never arms", async () => {
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
    open(policyUrl());
    const problem = await screen.findByTestId("cli-auth-tp-problem");
    expect(problem).toHaveTextContent(/^Only an owner or admin of the organization/);
    expect(problem).toHaveTextContent("Ask one of them to approve it.");
    expect(screen.getByTestId("cli-auth-tp-asked")).toHaveTextContent("acme-co/acme-apps");
    await attend(3000);
    expect(screen.getByTestId("cli-auth-confirm")).toBeDisabled();
    expect(mutate).not.toHaveBeenCalled();
  });

  it("points the approver at Trusted access when GitHub won't show the repository", async () => {
    preview = {
      data: undefined,
      isError: true,
      error: httpError(422, {
        code: "repository_unresolved",
        error: "GitHub would not show 'acme/private' to this deployment"
      }),
      refetch: vi.fn()
    };
    open(policyUrl());
    const problem = await screen.findByTestId("cli-auth-tp-problem");
    expect(problem).toHaveTextContent("GitHub would not show 'acme/private'");
    expect(problem).toHaveTextContent("Trusted access");
    expect(problem).toHaveTextContent("enter the repository's ids yourself");
    await attend(3000);
    expect(screen.getByTestId("cli-auth-confirm")).toBeDisabled();
  });

  // A run is matched on the ids and this page shows only the name: ids in the link are never sent.
  it("never forwards repository ids a link carries", async () => {
    open(policyUrl(`${ASK}&repository_id=987&repository_owner_id=42`));
    await screen.findByTestId("cli-auth-tp-org");
    expect(previewAsked).toHaveBeenCalledWith(MINT);
  });

  it("refuses a link that mixes in a token's params, and survives a sign-in round trip", () => {
    const params = (query: string) => new URLSearchParams(policyUrl(query).split("?")[1]);
    expect(parseCliAuthRequest(params(ASK)).kind).toBe("trust_policy_mint");
    for (const extra of ["role=admin", "hours=8", "apps=acme/store", "standing=1", "name=x"]) {
      expect(parseCliAuthRequest(params(`${ASK}&${extra}`)).kind, extra).toBe("invalid");
    }
    const missing = new URLSearchParams(`${PKCE_URL.split("?")[1]}&kind=trust_policy&org=org-1`);
    expect(parseCliAuthRequest(missing).kind).toBe("invalid");

    const request = parseCliAuthRequest(params(ASK));
    if (request.kind !== "trust_policy_mint") throw new Error(request.kind);
    const back = new URL(returnToUrl("https://app.example", request));
    expect(parseCliAuthRequest(back.searchParams)).toEqual(request);
  });
});
