// @vitest-environment jsdom

import { act, cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AxiosError, type AxiosResponse } from "axios";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Token } from "@/types/apiToken";
import type { PlatformCapability } from "@/types/auth";
import { expired, JO, LIN, partnerToken, revoked, token } from "./testTokens";

vi.setConfig({ testTimeout: 20000 });

const useStandingTokens = vi.fn();
const revoke = vi.fn();
let revoking: { isPending: boolean; variables?: { id: string } } = { isPending: false };

// Who is looking. `manage_platform_grants` is what opens this page; the audit log needs
// `view_audit`.
const GRANTER: PlatformCapability[] = ["manage_platform_grants"];
let viewer: { is_owner: boolean; platform_capabilities: PlatformCapability[] } = {
  is_owner: false,
  platform_capabilities: GRANTER
};
vi.mock("@/hooks/api/users/useCurrentUser", () => ({
  default: () => ({ data: viewer, isPending: false })
}));

// The hooks are the seam: this file is about what the page shows for each answer, and what a
// revoke sends.
vi.mock("@/hooks/api/standingTokens/useStandingTokens", () => ({
  useStandingTokens: () => useStandingTokens(),
  useRevokeStandingToken: () => ({ mutate: revoke, ...revoking })
}));

import AdminStandingTokens from "./index";

const loaded = (tokens: Token[]) => ({
  data: tokens,
  isPending: false,
  isError: false,
  error: null,
  refetch: vi.fn()
});

const failed = (status: number, data?: unknown) => {
  const response = { status, data } as AxiosResponse;
  return {
    data: undefined,
    isPending: false,
    isError: true,
    error: new AxiosError("Request failed", undefined, undefined, undefined, response),
    refetch: vi.fn()
  };
};

const show = (query: unknown) => {
  useStandingTokens.mockReturnValue(query);
  // Routed at the real path: the heading comes from the admin route map.
  render(
    <MemoryRouter initialEntries={["/admin/standing-tokens"]}>
      <AdminStandingTokens />
    </MemoryRouter>
  );
  return userEvent.setup();
};

const rows = () => screen.queryAllByTestId("admin-standing-tokens-row");
const names = () => rows().map((row) => row.getAttribute("data-token-name"));
const rowNamed = (name: string) => {
  const found = rows().find((row) => row.getAttribute("data-token-name") === name);
  if (!found) throw new Error(`no row for ${name}`);
  return found;
};
const summary = () => screen.getByTestId("admin-standing-tokens-summary");
const choice = (standing: "all" | "staff" | "partner") =>
  screen.getByTestId(`admin-standing-tokens-filter-${standing}`);
const hideEnded = () => screen.getByTestId("admin-standing-tokens-hide-ended");

/** One of each: a staff `oxyc login` token, a scoped partner token, a revoked and an expired one. */
const mixed = () => [token(), partnerToken(), revoked(), expired()];

afterEach(() => {
  cleanup();
  useStandingTokens.mockReset();
  revoke.mockReset();
  revoking = { isPending: false };
  viewer = { is_owner: false, platform_capabilities: GRANTER };
});

describe("Admin → Staff & partner tokens", () => {
  it("is named by the route map, like every admin page", () => {
    show(loaded([token()]));
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Staff & partner tokens");
  });

  it("lists the tokens in the order the server sent them, ended ones included", () => {
    show(loaded(mixed()));
    expect(names()).toEqual([
      "oxyc on ada-studio",
      "client onboarding",
      "oxyc on lost-laptop",
      "oxyc on old-laptop"
    ]);
    expect(rows().map((row) => row.getAttribute("data-token-status"))).toEqual([
      "active",
      "active",
      "revoked",
      "expired"
    ]);
  });

  describe("the summary", () => {
    it("says how many are live, for how many people, and how many of those are all-access", () => {
      show(loaded([token(), token({ id: "t2", owner: LIN }), partnerToken(), revoked()]));
      expect(summary()).toHaveTextContent(
        "3 tokens are live, held by 3 people. 2 of them are all-access. 1 more has expired or been revoked."
      );
    });

    it("says nothing works when every listed token has ended, and still lists them", () => {
      show(loaded([revoked(), expired()]));
      expect(summary()).toHaveTextContent(
        "No token with staff or partner standing works right now. The 2 below have expired or been revoked."
      );
      expect(rows()).toHaveLength(2);
    });

    it("is about the whole list, whatever the filter shows", async () => {
      const user = show(loaded(mixed()));
      const whole = summary().textContent;
      await user.click(choice("partner"));
      await user.click(hideEnded());
      expect(rows()).toHaveLength(1);
      expect(summary()).toHaveTextContent(whole ?? "");
    });
  });

  describe("the filter", () => {
    it("starts on every token, with the count each choice would show", () => {
      show(loaded(mixed()));
      expect(choice("all")).toHaveAttribute("data-state", "on");
      expect(choice("all")).toHaveTextContent("All4");
      expect(choice("staff")).toHaveTextContent("Staff standing3");
      expect(choice("partner")).toHaveTextContent("Partner standing1");
      expect(hideEnded()).toHaveAttribute("data-state", "unchecked");
    });

    it("narrows to the tokens carrying staff standing", async () => {
      const user = show(loaded(mixed()));
      await user.click(choice("staff"));
      expect(names()).toEqual(["oxyc on ada-studio", "oxyc on lost-laptop", "oxyc on old-laptop"]);
      expect(choice("staff")).toHaveAttribute("data-state", "on");
    });

    it("narrows to the tokens carrying partner standing", async () => {
      const user = show(loaded(mixed()));
      await user.click(choice("partner"));
      expect(names()).toEqual(["client onboarding"]);
    });

    it("lists a token carrying both under each standing", async () => {
      const both = token({ id: "t-both", name: "support console", partner: true });
      const user = show(loaded([both, partnerToken()]));
      await user.click(choice("staff"));
      expect(names()).toEqual(["support console"]);
      await user.click(choice("partner"));
      expect(names()).toEqual(["support console", "client onboarding"]);
    });

    it("stays on a choice that is pressed again", async () => {
      const user = show(loaded(mixed()));
      await user.click(choice("partner"));
      await user.click(choice("partner"));
      expect(choice("partner")).toHaveAttribute("data-state", "on");
      expect(names()).toEqual(["client onboarding"]);
    });

    it("hides the tokens that have ended, and recounts each choice", async () => {
      const user = show(loaded(mixed()));
      expect(screen.getByText("Hide expired and revoked (2)")).toBeInTheDocument();
      await user.click(hideEnded());
      expect(names()).toEqual(["oxyc on ada-studio", "client onboarding"]);
      expect(choice("all")).toHaveTextContent("All2");
      expect(choice("staff")).toHaveTextContent("Staff standing1");
    });

    it("hides a token that lapsed after the list was fetched", async () => {
      const lapsed = token({
        id: "t-lapsed",
        name: "just lapsed",
        expires_at: "2020-01-01T00:00:00Z"
      });
      const user = show(loaded([token(), lapsed]));
      await user.click(hideEnded());
      expect(names()).toEqual(["oxyc on ada-studio"]);
    });

    it("says so when a choice leaves no row, and offers the way back", async () => {
      const user = show(loaded([token(), revoked()]));
      await user.click(choice("partner"));
      expect(rows()).toHaveLength(0);
      expect(screen.getByTestId("admin-standing-tokens-no-match")).toHaveTextContent(
        "No token carries partner standing."
      );
      // The frame stays: it is a filter with no match, not a list with nothing in it.
      expect(screen.getByTestId("admin-standing-tokens-table")).toBeInTheDocument();
      expect(screen.queryByTestId("admin-standing-tokens-empty")).not.toBeInTheDocument();

      await user.click(screen.getByTestId("admin-standing-tokens-show-all"));
      expect(rows()).toHaveLength(2);
      expect(choice("all")).toHaveAttribute("data-state", "on");
    });

    it("says every token has ended when hiding them leaves none", async () => {
      const user = show(loaded([revoked(), expired()]));
      await user.click(hideEnded());
      expect(screen.getByTestId("admin-standing-tokens-no-match")).toHaveTextContent(
        "Every token listed has expired or been revoked."
      );
      await user.click(screen.getByTestId("admin-standing-tokens-show-all"));
      expect(hideEnded()).toHaveAttribute("data-state", "unchecked");
      expect(rows()).toHaveLength(2);
    });
  });

  describe("revoking", () => {
    it("is offered on a token that works, and not on one that has ended", () => {
      show(loaded(mixed()));
      for (const name of ["oxyc on ada-studio", "client onboarding"]) {
        expect(within(rowNamed(name)).getByTestId("admin-standing-tokens-revoke")).toBeEnabled();
      }
      for (const name of ["oxyc on lost-laptop", "oxyc on old-laptop"]) {
        expect(within(rowNamed(name)).queryByRole("button")).not.toBeInTheDocument();
      }
    });

    it("asks first, naming the token and its owner and saying what happens next", async () => {
      const user = show(loaded([token()]));
      await user.click(screen.getByTestId("admin-standing-tokens-revoke"));
      expect(revoke).not.toHaveBeenCalled();

      const dialog = await screen.findByRole("alertdialog");
      // The dialog's focus handling runs in an effect after it mounts.
      await act(async () => {});
      expect(dialog).toHaveTextContent("Revoke oxyc on ada-studio?");
      expect(dialog).toHaveTextContent(
        "ada@oxy.tech owns it. It stops working at once, and they sign in or run oxyc login again to get a new one."
      );

      await user.click(within(dialog).getByTestId("admin-standing-tokens-revoke-confirm"));
      expect(revoke).toHaveBeenCalledTimes(1);
      expect(revoke).toHaveBeenCalledWith({ id: "t1", name: "oxyc on ada-studio" });
    });

    it("still says what happens next for a token whose owner the server did not name", async () => {
      const user = show(loaded([token({ owner: { type: "user", id: "u-gone", label: "" } })]));
      await user.click(screen.getByTestId("admin-standing-tokens-revoke"));
      const dialog = await screen.findByRole("alertdialog");
      await act(async () => {});
      expect(dialog).toHaveTextContent(
        "It stops working at once, and its owner signs in or runs oxyc login again to get a new one."
      );
      expect(dialog).not.toHaveTextContent("owns it");
    });

    it("revokes nothing when the confirmation is cancelled", async () => {
      const user = show(loaded([token()]));
      await user.click(screen.getByTestId("admin-standing-tokens-revoke"));
      const dialog = await screen.findByRole("alertdialog");
      await act(async () => {});
      await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
      expect(revoke).not.toHaveBeenCalled();
    });

    it("holds the one row's Revoke while its request is in flight", () => {
      revoking = { isPending: true, variables: { id: "t1" } };
      show(loaded([token(), partnerToken()]));
      const busy = within(rowNamed("oxyc on ada-studio")).getByTestId(
        "admin-standing-tokens-revoke"
      );
      expect(busy).toBeDisabled();
      expect(busy).toHaveTextContent("Revoking…");
      expect(
        within(rowNamed("client onboarding")).getByTestId("admin-standing-tokens-revoke")
      ).toBeEnabled();
    });
  });

  describe("the states around the list", () => {
    it("shows placeholders while the list loads", () => {
      show({ data: undefined, isPending: true, isError: false, error: null });
      expect(screen.getByTestId("admin-async-loading")).toBeInTheDocument();
      expect(screen.queryByTestId("admin-standing-tokens-table")).not.toBeInTheDocument();
      expect(screen.queryByTestId("admin-standing-tokens-empty")).not.toBeInTheDocument();
    });

    it("says so when no token carries standing, and offers no filter over nothing", () => {
      show(loaded([]));
      expect(screen.getByTestId("admin-standing-tokens-empty")).toHaveTextContent(
        "No token carries staff or partner standing."
      );
      expect(screen.queryByTestId("admin-standing-tokens-table")).not.toBeInTheDocument();
      expect(screen.queryByTestId("admin-standing-tokens-filters")).not.toBeInTheDocument();
      expect(screen.queryByTestId("admin-standing-tokens-summary")).not.toBeInTheDocument();
    });

    it("tells a viewer without the capability so, with nothing to retry", () => {
      show(failed(403));
      const refused = screen.getByTestId("admin-standing-tokens-refused");
      expect(refused).toHaveTextContent("Your staff access doesn't include this list.");
      expect(refused).toHaveTextContent("manage_platform_grants");
      expect(screen.queryByTestId("admin-async-error")).not.toBeInTheDocument();
      expect(screen.queryByTestId("admin-async-retry")).not.toBeInTheDocument();
      expect(screen.queryByTestId("admin-standing-tokens-table")).not.toBeInTheDocument();
      expect(screen.queryByTestId("admin-standing-tokens-empty")).not.toBeInTheDocument();
    });

    it("tells a viewer whose access is limited to some organizations why, in those words", () => {
      show(
        failed(403, {
          code: "unbounded_grant_required",
          error:
            "your staff access is limited to some organizations; this needs access to all of them"
        })
      );
      const refused = screen.getByTestId("admin-standing-tokens-refused");
      expect(refused).toHaveTextContent("limited to some organizations");
      expect(refused).toHaveTextContent("needs access to every organization");
      expect(refused).not.toHaveTextContent("manage_platform_grants");
      expect(screen.queryByRole("button", { name: /retry/i })).not.toBeInTheDocument();
    });

    it("tells a session opened from a token to sign in, not that a capability is missing", () => {
      show(failed(403, { code: "session_required", error: "sign in with a browser session" }));
      const refused = screen.getByTestId("admin-standing-tokens-refused");
      expect(refused).toHaveTextContent("needs you to sign in in the browser");
      expect(refused).toHaveTextContent("oxyc login-link");
      expect(refused).not.toHaveTextContent("manage_platform_grants");
      expect(screen.queryByTestId("admin-async-retry")).not.toBeInTheDocument();
    });

    it("reports any other failure as a failure, with a way to try again", async () => {
      const query = failed(500);
      const user = show(query);
      expect(screen.getByTestId("admin-async-error")).toHaveTextContent(
        "Couldn’t load staff and partner tokens."
      );
      expect(screen.queryByTestId("admin-standing-tokens-refused")).not.toBeInTheDocument();
      // Not the empty state: a server that is down has not said "no tokens".
      expect(screen.queryByTestId("admin-standing-tokens-empty")).not.toBeInTheDocument();
      await user.click(screen.getByTestId("admin-async-retry"));
      expect(query.refetch).toHaveBeenCalledTimes(1);
    });

    it("says when the list was cut at the server's limit, whatever the filter shows", async () => {
      const many = Array.from({ length: 500 }, (_, index) =>
        expired({ id: `t-${index}`, name: `token ${index}` })
      );
      const user = show(loaded(many));
      const note = () => screen.getByTestId("admin-standing-tokens-truncated");
      expect(note()).toHaveTextContent("Showing the newest 500. Older tokens are not listed.");
      await user.click(hideEnded());
      expect(rows()).toHaveLength(0);
      expect(note()).toBeInTheDocument();
    });

    it("says nothing about a limit for a list under it", () => {
      show(loaded(mixed()));
      expect(screen.queryByTestId("admin-standing-tokens-truncated")).not.toBeInTheDocument();
    });
  });

  describe("a token's audit trail", () => {
    it("is one click from its name for a viewer the audit log admits", () => {
      viewer = { is_owner: false, platform_capabilities: [...GRANTER, "view_audit"] };
      show(loaded([token(), revoked()]));
      const trail = within(rowNamed("oxyc on ada-studio")).getByTestId(
        "admin-standing-tokens-trail"
      );
      expect(trail).toHaveTextContent("oxyc on ada-studio");
      expect(trail).toHaveAttribute("href", "/admin/audit?token_id=t1");
      // What was done with a token matters after it ended, so that row links too.
      expect(
        within(rowNamed("oxyc on lost-laptop")).getByTestId("admin-standing-tokens-trail")
      ).toHaveAttribute("href", "/admin/audit?token_id=t-revoked");
    });

    it("is there for an owner, who holds every capability", () => {
      viewer = { is_owner: true, platform_capabilities: [] };
      show(loaded([token()]));
      expect(screen.getByTestId("admin-standing-tokens-trail")).toBeInTheDocument();
    });

    it("is not offered to a viewer the audit log would turn away", () => {
      show(loaded([token()]));
      expect(screen.queryByTestId("admin-standing-tokens-trail")).not.toBeInTheDocument();
      // The name is still there, as text.
      expect(screen.getByTestId("admin-standing-tokens-name")).toHaveTextContent(
        "oxyc on ada-studio"
      );
      expect(within(rowNamed("oxyc on ada-studio")).queryByRole("link")).not.toBeInTheDocument();
    });
  });

  it("shows a partner's token beside a staff member's, each with its owner", () => {
    show(loaded([token(), partnerToken({ owner: JO })]));
    expect(
      within(rowNamed("client onboarding")).getByTestId("admin-standing-tokens-owner")
    ).toHaveTextContent("jo@harbor.example");
    expect(
      within(rowNamed("oxyc on ada-studio")).getByTestId("admin-standing-tokens-owner")
    ).toHaveTextContent("ada@oxy.tech");
  });
});
