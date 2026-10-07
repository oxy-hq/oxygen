// @vitest-environment jsdom

import { act, cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Table, TableBody } from "@/components/ui/shadcn/table";
import type { Token } from "@/types/apiToken";
import {
  expired,
  inDays,
  inHours,
  partnerToken,
  revoked,
  token,
  workspaceGrant
} from "../testTokens";
import { StandingTokenRow } from "./StandingTokenRow";

vi.setConfig({ testTimeout: 20000 });

const onRevoke = vi.fn();

const show = (shown: Token, props: { revoking?: boolean; trailHref?: string } = {}) => {
  render(
    <MemoryRouter>
      <Table>
        <TableBody>
          <StandingTokenRow
            token={shown}
            revoking={props.revoking ?? false}
            onRevoke={onRevoke}
            trailHref={props.trailHref}
          />
        </TableBody>
      </Table>
    </MemoryRouter>
  );
  return userEvent.setup();
};

const row = () => screen.getByTestId("admin-standing-tokens-row");
const cell = (name: string) => screen.getByTestId(`admin-standing-tokens-${name}`);

afterEach(() => {
  cleanup();
  onRevoke.mockReset();
});

describe("a staff member's oxyc login token", () => {
  it("says its name, owner, standing, reach, how it was made, when it dies and when it was used", () => {
    show(token());
    expect(cell("name")).toHaveTextContent("oxyc on ada-studio");
    expect(cell("owner")).toHaveTextContent("ada@oxy.tech");
    expect(cell("standing")).toHaveTextContent("Staff");
    expect(cell("reach")).toHaveTextContent("All access");
    expect(cell("made-with")).toHaveTextContent("oxyc login");
    expect(cell("expiry")).toHaveTextContent("Active, expires in 88 days");
    expect(cell("last-used")).toHaveTextContent("12m ago");
  });

  it("carries its prefix on the name's hover, which is how an audit row names it", () => {
    show(token());
    expect(cell("name")).toHaveAttribute("title", "oxyc on ada-studio\noxy_pat_Ab3x…wxyz");
  });

  it("names itself and its state in the DOM, for a selector to read", () => {
    show(token());
    expect(row()).toHaveAttribute("data-token-name", "oxyc on ada-studio");
    expect(row()).toHaveAttribute("data-token-id", "t1");
    expect(row()).toHaveAttribute("data-token-status", "active");
    expect(row()).toHaveAttribute("data-token-staff", "true");
    expect(row()).toHaveAttribute("data-token-partner", "false");
    expect(row()).toHaveAttribute("data-token-all-access", "true");
    expect(row()).toHaveAttribute("data-token-oxyc-login", "true");
  });

  it("leaves the standing to its own column: the reach is the reach alone", () => {
    show(token());
    expect(cell("reach")).toHaveTextContent(/^All access$/);
    expect(screen.queryByTestId("account-token-standing-platform")).not.toBeInTheDocument();
  });

  it("sets an all-access reach in weight while the token works", () => {
    show(token());
    expect(cell("reach")).toHaveClass("font-medium");
  });

  it("speaks of the owner's reach on hover, not the reader's", async () => {
    const user = show(token());
    await user.hover(screen.getByTestId("account-token-access-label"));
    const tip = await screen.findByRole("tooltip");
    // The tooltip positions itself in an effect after it mounts.
    await act(async () => {});
    expect(tip).toHaveTextContent("All access, with staff standing");
    expect(tip).toHaveTextContent("Every organization and workspace its owner can reach");
    expect(tip).not.toHaveTextContent("you can reach");
  });

  it("offers Revoke, and hands the token over when it is pressed", async () => {
    const shown = token();
    const user = show(shown);
    await user.click(cell("revoke"));
    expect(onRevoke).toHaveBeenCalledWith(shown);
  });

  it("holds Revoke while its own request is in flight", () => {
    show(token(), { revoking: true });
    expect(cell("revoke")).toBeDisabled();
    expect(cell("revoke")).toHaveTextContent("Revoking…");
  });

  it("says a token made to last has no expiry", () => {
    show(token({ expires_at: null }));
    expect(cell("expiry")).toHaveTextContent("Active, No expiry");
  });

  it("says a token nobody has used yet was never used", () => {
    show(token({ last_used_at: null }));
    expect(cell("last-used")).toHaveTextContent("never");
  });
});

describe("a partner's token made in Settings", () => {
  it("says partner standing, the workspaces it reaches and that Settings made it", () => {
    show(partnerToken());
    expect(cell("standing")).toHaveTextContent(/^Partner$/);
    expect(cell("reach")).toHaveTextContent("All workspaces in Rivermark");
    expect(cell("made-with")).toHaveTextContent("Settings");
    expect(row()).toHaveAttribute("data-token-staff", "false");
    expect(row()).toHaveAttribute("data-token-partner", "true");
    expect(row()).toHaveAttribute("data-token-all-access", "false");
    expect(row()).toHaveAttribute("data-token-oxyc-login", "false");
  });

  it("counts named workspaces and their orgs, the way the account list does", () => {
    show(
      partnerToken({
        grants: [
          workspaceGrant("Rivermark", "Ops"),
          workspaceGrant("Rivermark", "Labor"),
          workspaceGrant("Poke House", "Analytics")
        ]
      })
    );
    expect(cell("reach")).toHaveTextContent("3 workspaces in 2 orgs");
  });

  it("does not set a scoped reach in weight", () => {
    show(partnerToken());
    expect(cell("reach")).not.toHaveClass("font-medium");
  });
});

describe("a token carrying both standings", () => {
  it("says both, and is marked as each", () => {
    show(token({ partner: true }));
    expect(cell("standing")).toHaveTextContent("Staff and partner");
    expect(row()).toHaveAttribute("data-token-staff", "true");
    expect(row()).toHaveAttribute("data-token-partner", "true");
  });
});

describe("a token that has ended", () => {
  it("says it was revoked and how long ago, and offers no action", () => {
    show(revoked());
    expect(row()).toHaveAttribute("data-token-status", "revoked");
    expect(cell("expiry")).toHaveTextContent("Revoked3h ago");
    expect(screen.queryByTestId("admin-standing-tokens-revoke")).not.toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("says it expired and how long ago, and offers no action", () => {
    show(expired());
    expect(row()).toHaveAttribute("data-token-status", "expired");
    expect(cell("expiry")).toHaveTextContent("Expired2d ago");
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("is set back, its all-access reach with it", () => {
    show(revoked());
    expect(row()).toHaveClass("text-muted-foreground");
    expect(cell("reach")).not.toHaveClass("font-medium");
  });

  it("reads as expired once its expiry has passed, whatever the fetched status says", () => {
    show(token({ status: "active", expires_at: inHours(-0.1) }));
    expect(row()).toHaveAttribute("data-token-status", "expired");
    expect(screen.queryByTestId("admin-standing-tokens-revoke")).not.toBeInTheDocument();
  });

  it("stays revoked however its expiry reads", () => {
    show(revoked({ expires_at: inDays(40) }));
    expect(row()).toHaveAttribute("data-token-status", "revoked");
    expect(cell("expiry")).toHaveTextContent("Revoked");
  });

  it("still leads to its audit trail from its name", () => {
    show(revoked(), { trailHref: "/admin/audit?token_id=t-revoked" });
    expect(cell("trail")).toHaveAttribute("href", "/admin/audit?token_id=t-revoked");
  });
});

describe("a token the server describes oddly", () => {
  it("names a personal token's own spelling of oxyc login the same way", () => {
    show(token({ source: "oxyc_login" }));
    expect(cell("made-with")).toHaveTextContent("oxyc login");
    expect(row()).toHaveAttribute("data-token-oxyc-login", "true");
  });

  it("shows a source it does not know as the server spelled it", () => {
    show(token({ source: "oidc" }));
    expect(cell("made-with")).toHaveTextContent("oidc");
    expect(row()).toHaveAttribute("data-token-oxyc-login", "false");
  });

  it("does not claim a standing the token does not carry", () => {
    show(token({ platform: false, partner: false }));
    expect(cell("standing")).toHaveTextContent("None");
  });

  it("says Unknown for an owner the server did not name", () => {
    show(token({ owner: { type: "user", id: "u-gone", label: "" } }));
    expect(cell("owner")).toHaveTextContent("Unknown");
  });
});
