// @vitest-environment jsdom

import { act, cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AuditEvent, AuditSearchParams } from "@/types/audit";
import { plainEvent, TOKEN_ID, tokenEvent } from "./auditFixtures";

vi.setConfig({ testTimeout: 20000 });

const useAuditSearch = vi.fn();
vi.mock("@/hooks/api/audit", () => ({
  useAuditSearch: (params: AuditSearchParams) => useAuditSearch(params)
}));

import AdminAudit from "./index";

const show = (events: AuditEvent[], search = "") => {
  useAuditSearch.mockReturnValue({
    data: events,
    isPending: false,
    isError: false,
    error: null,
    refetch: vi.fn()
  });
  render(
    <MemoryRouter initialEntries={[`/admin/audit${search}`]}>
      <AdminAudit />
    </MemoryRouter>
  );
  return userEvent.setup();
};

/** What the page last asked the server for. */
const asked = (): AuditSearchParams => useAuditSearch.mock.calls.at(-1)?.[0];

afterEach(() => {
  cleanup();
  useAuditSearch.mockReset();
});

describe("Admin → Audit, narrowed to one token", () => {
  it("asks for every token when the link names none", () => {
    show([plainEvent()]);
    expect(asked().token_id).toBeUndefined();
    expect(screen.queryByTestId("admin-audit-token-filter")).not.toBeInTheDocument();
    // Nothing is filtered, so there is nothing to clear.
    expect(screen.queryByRole("button", { name: "Clear" })).not.toBeInTheDocument();
  });

  it("narrows to the token a link names, and says which by its name", () => {
    show([tokenEvent()], `?token_id=${TOKEN_ID}`);
    expect(asked().token_id).toBe(TOKEN_ID);
    expect(screen.getByTestId("admin-audit-token-filter")).toHaveTextContent("Tokenlive check");
  });

  it("shows the start of the id while no loaded row names the token", () => {
    show([plainEvent()], `?token_id=${TOKEN_ID}`);
    expect(screen.getByTestId("admin-audit-token-filter")).toHaveTextContent("Token0b5c1d6e");
    // The whole id is a hover away.
    expect(screen.getByTestId("admin-audit-token-filter")).toHaveAttribute("title", TOKEN_ID);
  });

  it("reads a token_id that is not a UUID as no filter", () => {
    show([plainEvent()], "?token_id=refunds");
    expect(asked().token_id).toBeUndefined();
    expect(screen.queryByTestId("admin-audit-token-filter")).not.toBeInTheDocument();
  });

  it("narrows from a row's detail, then widens again from the chip", async () => {
    const user = show([tokenEvent()]);
    await user.click(screen.getByTestId("admin-audit-toggle"));
    await user.click(await screen.findByTestId("admin-audit-filter-token"));
    // The filter is set through the router: let its update land before reading it back.
    await act(async () => {});

    expect(asked().token_id).toBe(TOKEN_ID);
    expect(screen.getByTestId("admin-audit-token-filter")).toHaveTextContent("live check");
    // The row is now one of the token's own, so it no longer offers to narrow to it.
    expect(screen.queryByTestId("admin-audit-filter-token")).not.toBeInTheDocument();

    await user.click(screen.getByTestId("admin-audit-token-filter-clear"));
    await act(async () => {});
    expect(asked().token_id).toBeUndefined();
    expect(screen.queryByTestId("admin-audit-token-filter")).not.toBeInTheDocument();
  });

  it("drops the token with every other filter on Clear", async () => {
    const user = show([tokenEvent()], `?token_id=${TOKEN_ID}`);
    await user.click(screen.getByRole("button", { name: "Clear" }));
    await act(async () => {});
    expect(asked().token_id).toBeUndefined();
    expect(screen.queryByTestId("admin-audit-token-filter")).not.toBeInTheDocument();
  });

  it("keeps the other filters in the request beside the token", () => {
    show([tokenEvent()], `?token_id=${TOKEN_ID}`);
    expect(asked()).toEqual({
      q: undefined,
      action: undefined,
      outcome: undefined,
      token_id: TOKEN_ID,
      limit: 200
    });
  });
});
