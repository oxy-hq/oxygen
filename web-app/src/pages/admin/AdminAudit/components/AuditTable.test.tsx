// @vitest-environment jsdom

import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AuditEvent } from "@/types/audit";
import { AGENT_CLIENT, lifecycleEvent, plainEvent, TOKEN_ID, tokenEvent } from "../auditFixtures";
import AuditTable from "./AuditTable";

afterEach(cleanup);

const show = (
  events: AuditEvent[],
  props: { tokenId?: string; onFilterToken?: (id: string) => void } = {}
) => {
  render(<AuditTable events={events} limit={200} {...props} />);
  return userEvent.setup();
};

const row = (id: string) => screen.getByTestId(`admin-audit-row-${id}`);
const cells = (id: string) =>
  within(row(id))
    .getAllByRole("cell")
    .map((cell) => cell.textContent);

describe("AuditTable", () => {
  it("keeps its six columns, behind one for the chevron", () => {
    show([plainEvent()]);
    expect(screen.getAllByRole("columnheader").map((head) => head.textContent)).toEqual([
      "Details",
      "When",
      "Actor",
      "Action",
      "Target",
      "Scope",
      "Outcome"
    ]);
  });

  describe("a row that names no token, address or client", () => {
    it("shows the cells it always had and nothing else", () => {
      show([plainEvent()]);
      expect(cells("e-plain")).toEqual([
        "",
        "2h",
        "ada@oxy.tech",
        "org.member.added",
        "lin@oxy.tech",
        "org·111111",
        ""
      ]);
    });

    it("has no token line and nothing to open", () => {
      show([plainEvent()]);
      expect(screen.queryByTestId("admin-audit-token")).not.toBeInTheDocument();
      expect(screen.queryByTestId("admin-audit-toggle")).not.toBeInTheDocument();
      expect(within(row("e-plain")).queryByRole("button")).not.toBeInTheDocument();
    });

    it("still marks an actor that is not a person", () => {
      show([plainEvent({ actor_type: "system" })]);
      expect(cells("e-plain")[2]).toBe("ada@oxy.tech(system)");
    });
  });

  describe("a row a token performed", () => {
    it("says which token acted, and its kind, under the actor", () => {
      show([tokenEvent()]);
      const via = within(row("e-token")).getByTestId("admin-audit-token");
      expect(via).toHaveTextContent("via live check · Sandbox agent");
    });

    it("keeps the token's prefix a hover away", () => {
      show([tokenEvent()]);
      expect(within(row("e-token")).getByTestId("admin-audit-token")).toHaveAttribute(
        "title",
        "oxy_sbx_Ab3x…"
      );
    });

    it("says so when the token that acted has no name on the row", () => {
      show([tokenEvent({ token_name: null })]);
      expect(screen.getByTestId("admin-audit-token")).toHaveTextContent(
        "via a token with no name · Sandbox agent"
      );
    });

    it("adds no column for the address or the client", () => {
      show([tokenEvent()]);
      expect(within(row("e-token")).getAllByRole("cell")).toHaveLength(7);
      expect(row("e-token")).not.toHaveTextContent("203.0.113.7");
      expect(row("e-token")).not.toHaveTextContent("oxyc/");
    });

    it("opens to the token, the address and the client, and closes again", async () => {
      const user = show([tokenEvent()]);
      const toggle = within(row("e-token")).getByTestId("admin-audit-toggle");
      expect(toggle).toHaveAttribute("aria-expanded", "false");
      expect(screen.queryByTestId("admin-audit-detail-e-token")).not.toBeInTheDocument();

      await user.click(toggle);
      expect(toggle).toHaveAttribute("aria-expanded", "true");
      const detail = within(screen.getByTestId("admin-audit-detail-e-token"));
      expect(detail.getByTestId("admin-audit-detail-token")).toHaveTextContent(
        "Acted withlive checkSandbox agentoxy_sbx_Ab3x…"
      );
      expect(detail.getByTestId("admin-audit-detail-ip")).toHaveTextContent("Address203.0.113.7");
      expect(detail.getByTestId("admin-audit-detail-client")).toHaveTextContent(
        `Client${AGENT_CLIENT}`
      );
      // The detail is the row its toggle says it controls.
      expect(screen.getByTestId("admin-audit-detail-e-token")).toHaveAttribute(
        "id",
        toggle.getAttribute("aria-controls")
      );

      await user.click(toggle);
      expect(screen.queryByTestId("admin-audit-detail-e-token")).not.toBeInTheDocument();
    });
  });

  describe("a token lifecycle event made in a session", () => {
    it("does not say the person acted through the token", () => {
      show([lifecycleEvent()]);
      expect(screen.queryByTestId("admin-audit-token")).not.toBeInTheDocument();
    });

    it("names the token the event is about in its detail, from the row's target label", async () => {
      // `token_name` is null on a lifecycle row: the name is the target.
      const user = show([lifecycleEvent()]);
      expect(cells("e-lifecycle")[4]).toBe("live check");
      await user.click(screen.getByTestId("admin-audit-toggle"));
      expect(screen.getByTestId("admin-audit-detail-token")).toHaveTextContent(
        "About tokenlive checkSandbox agentoxy_sbx_Ab3x…"
      );
    });

    it("still shows the client that asked, an agent driving oxyc included", async () => {
      const user = show([lifecycleEvent()]);
      await user.click(screen.getByTestId("admin-audit-toggle"));
      expect(screen.getByTestId("admin-audit-detail-client")).toHaveTextContent(AGENT_CLIENT);
    });
  });

  describe("a row made in a session, on something that is not a token", () => {
    it("opens to the address and the client alone", async () => {
      const user = show([
        plainEvent({ ip: "198.51.100.4", user_agent: "Mozilla/5.0 (Macintosh)" })
      ]);
      await user.click(screen.getByTestId("admin-audit-toggle"));
      expect(screen.getByTestId("admin-audit-detail-ip")).toHaveTextContent("198.51.100.4");
      expect(screen.getByTestId("admin-audit-detail-client")).toHaveTextContent(
        "Mozilla/5.0 (Macintosh)"
      );
      expect(screen.queryByTestId("admin-audit-detail-token")).not.toBeInTheDocument();
      expect(screen.queryByTestId("admin-audit-filter-token")).not.toBeInTheDocument();
    });
  });

  describe("narrowing to one token", () => {
    it("hands the row's token id up from its detail", async () => {
      const onFilterToken = vi.fn();
      const user = show([tokenEvent()], { onFilterToken });
      await user.click(screen.getByTestId("admin-audit-toggle"));
      await user.click(screen.getByTestId("admin-audit-filter-token"));
      expect(onFilterToken).toHaveBeenCalledTimes(1);
      expect(onFilterToken).toHaveBeenCalledWith(TOKEN_ID);
    });

    it("is not offered on a row of the token the log is already narrowed to", async () => {
      const user = show([tokenEvent()], { tokenId: TOKEN_ID, onFilterToken: vi.fn() });
      await user.click(screen.getByTestId("admin-audit-toggle"));
      expect(screen.getByTestId("admin-audit-detail-token")).toBeInTheDocument();
      expect(screen.queryByTestId("admin-audit-filter-token")).not.toBeInTheDocument();
    });

    it("is not offered where nothing can narrow the log", async () => {
      const user = show([tokenEvent()]);
      await user.click(screen.getByTestId("admin-audit-toggle"));
      expect(screen.queryByTestId("admin-audit-filter-token")).not.toBeInTheDocument();
    });
  });

  it("says when the stream was cut at the limit", () => {
    render(<AuditTable events={[plainEvent()]} limit={1} />);
    expect(screen.getByText(/Showing the most recent 1\./)).toBeInTheDocument();
  });
});
