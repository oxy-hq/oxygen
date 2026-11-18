// @vitest-environment jsdom

import { act, cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AxiosError, type AxiosResponse } from "axios";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CliDeviceRequest, SandboxApp, TokenOptions } from "@/types/apiToken";
import type { CliSession } from "../cliSession";
import CliAuthDevice from ".";

/**
 * `/cli-auth/device`: approving `oxyc … --no-browser` from any device. The sheet is the one a
 * link opens (its rules are pinned in `../index.test.tsx`); here, what a device-code request
 * adds: the code to compare before anything is approved, the decision recorded on the request
 * instead of a redirect, and a decline that tells oxyc.
 */

vi.setConfig({ testTimeout: 20000 });

const leaveTo = vi.fn();
let session: CliSession | null = { token: "session.jwt", email: "luong@example.com" };
vi.mock("../cliSession", () => ({
  leaveTo: (url: string) => leaveTo(url),
  readCliSession: () => Promise.resolve(session)
}));

type RequestQuery = { data?: CliDeviceRequest; error: Error | null; isPending: boolean };
let deviceRequest: RequestQuery;
const refetch = vi.fn();
vi.mock("@/hooks/api/cliAuth/useCliDeviceRequest", () => ({
  default: (code: string | null) =>
    code === null
      ? { data: undefined, error: null, isPending: true, refetch }
      : { ...deviceRequest, refetch }
}));

type MutateOptions = { onSuccess?: () => void; onError?: (e: Error) => void };
const approve = vi.fn();
vi.mock("@/hooks/api/cliAuth/useApproveCliDevice", () => ({
  default: () => ({ mutate: approve, isPending: false, isError: false, error: null })
}));
const deny = vi.fn();
vi.mock("@/hooks/api/cliAuth/useDenyCliDevice", () => ({
  default: () => ({ mutate: deny, isError: false })
}));

const APP: SandboxApp = {
  id: "a1",
  org_id: "org-acme",
  org_slug: "acme",
  org_name: "Acme",
  slug: "store-ops",
  name: "Store Ops"
};
const OPTIONS: TokenOptions = {
  orgs: [],
  can_platform: true,
  can_partner: false,
  sandbox_agent: { default_hours: 8, max_hours: 168, max_apps: 5 },
  agent: { default_hours: 8, max_hours: 168 },
  sandbox_apps: [APP]
};
const previewAsked = vi.fn();
// One object, as react-query hands back: Approve arms against what it shows, by identity.
const SA_PREVIEW = {
  data: {
    org: { id: "org-acme", slug: "acme", name: "Acme" },
    account: { name: "store-ops-tablets", role: "admin", exists: false },
    token: { name: "tablets", expires_in_days: 90, expires_at: "2027-02-16T12:00:00Z" },
    grant: { org_wide: true, role_ceiling: "admin" }
  },
  isError: false,
  error: null,
  refetch: vi.fn()
};
vi.mock("@/hooks/api/cliAuth/useServiceAccountPreview", () => ({
  default: (hostname: string, mint: unknown) => {
    previewAsked(hostname, mint);
    return SA_PREVIEW;
  }
}));
vi.mock("@/hooks/api/userTokens/useUserTokens", () => ({
  useTokenOptions: () => ({ data: OPTIONS, isLoading: false, isError: false, refetch: vi.fn() })
}));

const pending = (ask: Record<string, string>): RequestQuery => ({
  data: {
    user_code: "BCDF-GHJK",
    hostname: "cloud-box",
    ask,
    created_at: "2026-10-21T00:00:00Z",
    expires_at: "2026-10-21T00:10:00Z"
  },
  error: null,
  isPending: false
});
const SANDBOX_ASK = { kind: "sandbox_agent", apps: "acme/store-ops", hours: "8" };

beforeEach(() => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  vi.spyOn(document, "hasFocus").mockReturnValue(true);
  deviceRequest = pending(SANDBOX_ASK);
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  leaveTo.mockReset();
  approve.mockReset();
  deny.mockReset();
  session = { token: "session.jwt", email: "luong@example.com" };
});

const open = (url: string) =>
  render(
    <MemoryRouter initialEntries={[url]}>
      <Routes>
        <Route path='/cli-auth/device' element={<CliAuthDevice />} />
      </Routes>
    </MemoryRouter>
  );

/** Let the sheet count the second it waits before Approve arms. */
const attend = async () => {
  await act(async () => {});
  await act(async () => {
    await vi.advanceTimersByTimeAsync(1000);
  });
};

const user = () => userEvent.setup({ advanceTimers: vi.advanceTimersByTime });

describe("/cli-auth/device", () => {
  it("asks for the code when the link carries none", async () => {
    open("/cli-auth/device");
    const input = await screen.findByTestId("cli-auth-device-code-input");
    await user().type(input, "nope");
    await user().click(screen.getByTestId("cli-auth-device-code-continue"));
    expect(screen.getByRole("alert")).toHaveTextContent("eight letters");
    await user().clear(input);
    await user().type(input, "bcdfghjk");
    await user().click(screen.getByTestId("cli-auth-device-code-continue"));
    // On to the request: the code is typed again there, never shown.
    expect(await screen.findByTestId("cli-auth-device-code-typed")).toHaveValue("");
  });

  it("sends a person with no session through login and back to this code", async () => {
    session = null;
    open("/cli-auth/device?code=bcdf-ghjk");
    await act(async () => {});
    const back = `${window.location.origin}/cli-auth/device?code=BCDF-GHJK`;
    expect(leaveTo).toHaveBeenCalledWith(`/login?return_to=${encodeURIComponent(back)}`);
  });

  it("approves nothing until the person types the code from their terminal", async () => {
    open("/cli-auth/device?code=BCDF-GHJK");
    expect(await screen.findByTestId("cli-auth-hostname")).toHaveTextContent("cloud-box");
    expect(screen.getByTestId("cli-auth-mint-command")).toHaveTextContent(
      "oxyc tokens create --sandbox-agent --no-browser"
    );
    // The page never shows the code it was opened with: a forwarded link carries no answer.
    expect(screen.queryByText("BCDF-GHJK")).not.toBeInTheDocument();
    await attend();
    const confirm = screen.getByTestId("cli-auth-confirm");
    expect(confirm).toBeDisabled();

    const typed = screen.getByTestId("cli-auth-device-code-typed");
    await user().type(typed, "bcdf-ghjx");
    expect(confirm).toBeDisabled();
    await user().clear(typed);
    await user().type(typed, "bcdf ghjk");
    expect(confirm).toBeEnabled();
    await user().click(confirm);

    expect(approve).toHaveBeenCalledTimes(1);
    const [body, options] = approve.mock.calls[0] as [unknown, MutateOptions];
    expect(body).toEqual({
      mint: {
        kind: "sandbox_agent",
        apps: ["a1"],
        expires_in_hours: 8,
        name: "Sandbox agent on cloud-box"
      }
    });
    // Nothing leaves for oxyc: it collects the token by polling.
    act(() => options.onSuccess?.());
    expect(await screen.findByText("Approved")).toBeInTheDocument();
    expect(leaveTo).not.toHaveBeenCalled();
  });

  it("approves oxyc login once the code matches", async () => {
    deviceRequest = pending({});
    open("/cli-auth/device?code=BCDF-GHJK");
    const confirm = await screen.findByTestId("cli-auth-confirm");
    expect(confirm).toHaveTextContent("Authorize oxyc");
    expect(screen.getByTestId("cli-auth-card")).toHaveTextContent(
      "never carries staff or partner access"
    );
    expect(confirm).toBeDisabled();
    await user().type(screen.getByTestId("cli-auth-device-code-typed"), "BCDFGHJK");
    await user().click(confirm);
    expect((approve.mock.calls[0] as [unknown])[0]).toEqual({});
  });

  it("never offers staff access on an agent token, even when asked", async () => {
    deviceRequest = pending({ kind: "agent", hours: "4", standing: "1" });
    open("/cli-auth/device?code=BCDF-GHJK");
    expect(await screen.findByTestId("cli-auth-agent-standing-out")).toHaveTextContent(
      "not included"
    );
    expect(screen.queryByTestId("cli-auth-agent-standing")).not.toBeInTheDocument();
    await attend();
    await user().type(screen.getByTestId("cli-auth-device-code-typed"), "BCDF-GHJK");
    await user().click(screen.getByTestId("cli-auth-confirm"));
    expect((approve.mock.calls[0] as [{ mint: { standing: boolean } }])[0].mint.standing).toBe(
      false
    );
  });

  it("approves an organization's service account token once the code matches", async () => {
    deviceRequest = pending({
      kind: "service_account",
      org: "acme",
      account: "store-ops-tablets",
      role: "admin",
      days: "90",
      name: "tablets"
    });
    open("/cli-auth/device?code=BCDF-GHJK");
    expect(await screen.findByTestId("cli-auth-sa-account")).toHaveTextContent(
      "store-ops-tablets admin"
    );
    expect(screen.getByTestId("cli-auth-mint-command")).toHaveTextContent(
      "oxyc tokens create --org-service-account acme/store-ops-tablets --role admin --no-browser"
    );
    expect(previewAsked).toHaveBeenCalledWith(
      "cloud-box",
      expect.objectContaining({ org: "acme" })
    );
    await attend();
    const confirm = screen.getByTestId("cli-auth-confirm");
    expect(confirm).toBeDisabled();
    await user().type(screen.getByTestId("cli-auth-device-code-typed"), "BCDF-GHJK");
    await user().click(confirm);
    expect((approve.mock.calls[0] as [unknown])[0]).toEqual({
      mint: {
        kind: "service_account",
        org: "acme",
        account: "store-ops-tablets",
        role: "admin",
        name: "tablets",
        expires_in_days: 90
      }
    });
  });

  it("tells oxyc when the person declines", async () => {
    open("/cli-auth/device?code=BCDF-GHJK");
    await user().click(await screen.findByTestId("cli-auth-cancel"));
    expect(deny).toHaveBeenCalledTimes(1);
    expect(approve).not.toHaveBeenCalled();
    expect(screen.getByText("Request declined")).toBeInTheDocument();
  });

  it("says when no request waits for the code", async () => {
    const notFound = new AxiosError("not found", "ERR_BAD_REQUEST", undefined, undefined, {
      status: 404,
      data: { error: "not found" }
    } as AxiosResponse);
    deviceRequest = { data: undefined, error: notFound, isPending: false };
    open("/cli-auth/device?code=BCDF-GHJK");
    expect(await screen.findByText("No request is waiting for this code")).toBeInTheDocument();
    expect(screen.queryByTestId("cli-auth-confirm")).not.toBeInTheDocument();
  });

  it("refuses a request no link could carry", async () => {
    deviceRequest = pending({ kind: "personal" });
    open("/cli-auth/device?code=BCDF-GHJK");
    expect(await screen.findByText("Token request failed")).toBeInTheDocument();
    expect(screen.queryByTestId("cli-auth-confirm")).not.toBeInTheDocument();
  });
});
