// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Token } from "@/types/apiToken";
import TokenTable from "./TokenTable";

const useTokenOptions = vi.fn();
let tokens: Partial<Token>[] = [];
vi.mock("@/hooks/api/userTokens/useUserTokens", () => ({
  useTokenOptions: (enabled?: boolean) => useTokenOptions(enabled),
  useUserTokens: () => ({ data: { tokens }, isLoading: false, error: null, refetch: vi.fn() })
}));

// The row has its own tests. Here it only says which token the table handed it.
vi.mock("./TokenRow", () => ({
  default: ({ token }: { token: Token }) => (
    <tr data-testid='row' data-kind={token.kind}>
      <td>{token.name}</td>
    </tr>
  )
}));

afterEach(() => {
  cleanup();
  useTokenOptions.mockReset();
  tokens = [];
});

describe("TokenTable", () => {
  it("has a column for the kind, between the name and the token", () => {
    tokens = [{ id: "t1", name: "laptop", kind: "personal" }];
    render(<TokenTable onRegenerated={vi.fn()} />);
    expect(screen.getAllByRole("columnheader").map((head) => head.textContent)).toEqual([
      "Name",
      "Kind",
      "Token",
      "Access",
      "Expiry",
      "Last used",
      "Actions"
    ]);
  });

  it("measures its own box, and gives way there by column, the Token column first", () => {
    tokens = [{ id: "t1", name: "laptop", kind: "personal" }];
    render(<TokenTable onRegenerated={vi.fn()} />);
    const table = screen.getByTestId("account-token-table");
    // A container query, not a viewport one: Settings shows the list in a pane of one width.
    expect(table.parentElement).toHaveClass("@container");
    expect(table).toHaveClass("table-fixed");

    const head = (name: string) => screen.getByRole("columnheader", { name });
    expect(head("Token")).toHaveClass("hidden", "@5xl:table-cell");
    expect(head("Last used")).toHaveClass("hidden", "@2xl:table-cell");
    expect(head("Kind")).toHaveClass("hidden", "@xl:table-cell");
    // What a row is read for never gives way.
    for (const name of ["Name", "Access", "Expiry", "Actions"]) {
      expect(head(name)).not.toHaveClass("hidden");
    }
  });

  it("shows a row for every token, a sandbox agent token among them", () => {
    tokens = [
      { id: "t1", name: "laptop", kind: "personal" },
      { id: "t2", name: "refunds task", kind: "sandbox_agent" }
    ];
    render(<TokenTable onRegenerated={vi.fn()} />);
    expect(screen.getAllByTestId("row").map((row) => row.getAttribute("data-kind"))).toEqual([
      "personal",
      "sandbox_agent"
    ]);
  });

  it("never reads the token options: a grant names its own app, so the list is one request", () => {
    tokens = [
      { id: "t1", name: "laptop", kind: "personal" },
      { id: "t2", name: "refunds task", kind: "sandbox_agent" }
    ];
    render(<TokenTable onRegenerated={vi.fn()} />);
    expect(useTokenOptions).not.toHaveBeenCalled();
  });
});
