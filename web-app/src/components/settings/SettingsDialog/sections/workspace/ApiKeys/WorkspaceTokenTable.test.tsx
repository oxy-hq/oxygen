// @vitest-environment jsdom

import { cleanup, render, screen, within } from "@testing-library/react";
import { AxiosError, type AxiosResponse } from "axios";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { WorkspaceTokenListResponse, WorkspaceTokenRow } from "@/types/apiToken";
import WorkspaceTokenTable from "./WorkspaceTokenTable";

vi.setConfig({ testTimeout: 20000 });

type QueryState = {
  data?: WorkspaceTokenListResponse;
  isLoading: boolean;
  error: Error | null;
  refetch: () => void;
};
let query: QueryState = { isLoading: true, error: null, refetch: vi.fn() };

// The query hook is the seam: this file is about what the inventory renders.
vi.mock("@/hooks/api/userTokens/useWorkspaceTokens", () => ({ default: () => query }));

afterEach(cleanup);

const row = (over: Partial<WorkspaceTokenRow>): WorkspaceTokenRow => ({
  id: "t1",
  name: "deploy bot",
  kind: "service_account",
  display_prefix: "oxy_sat_Qr7k",
  all_access: false,
  expires_at: null,
  last_used_at: null,
  status: "active",
  owner: { type: "service_account", id: "sa1", label: "deploy-bot" },
  role_ceiling_here: "member",
  ...over
});

const show = (state: Partial<QueryState>) => {
  query = { isLoading: false, error: null, refetch: vi.fn(), ...state };
  render(<WorkspaceTokenTable />);
};

describe("WorkspaceTokenTable", () => {
  it("lists each token's owner, kind and what it can do here", () => {
    show({
      data: {
        tokens: [
          row({}),
          row({
            id: "t2",
            name: "laptop",
            kind: "personal",
            display_prefix: "oxy_pat_Ab3x",
            all_access: true,
            owner: { type: "user", id: "u1", label: "ana@example.com" },
            role_ceiling_here: "owner"
          })
        ]
      }
    });
    const [bot, personal] = screen.getAllByTestId("workspace-token-row");
    expect(bot).toHaveTextContent("deploy-bot");
    expect(bot).toHaveTextContent("Service account");
    expect(within(bot).getByTestId("workspace-token-ceiling")).toHaveTextContent("Write");
    expect(bot).toHaveTextContent("oxy_sat_Qr7k…");

    expect(personal).toHaveTextContent("ana@example.com");
    expect(personal).toHaveTextContent("Personal");
    expect(within(personal).getByTestId("workspace-token-ceiling")).toHaveTextContent("Full");
  });

  it("is read-only: an expired token shows its status and no way to extend it", () => {
    show({ data: { tokens: [row({ expires_at: "2020-01-01T00:00:00Z", status: "expired" })] } });
    expect(screen.getByTestId("workspace-token-row")).toHaveTextContent("Expired");
    expect(screen.queryByTestId("api-key-expired-extend-button")).not.toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("says the inventory isn't there yet on a server that predates it", () => {
    show({
      error: new AxiosError("Not found", "ERR", undefined, undefined, {
        status: 404
      } as AxiosResponse)
    });
    expect(
      screen.getByText("The token inventory isn't available on this server yet.")
    ).toBeInTheDocument();
  });

  it("says so when no token reaches the workspace", () => {
    show({ data: { tokens: [] } });
    expect(screen.getByText("No tokens reach this workspace")).toBeInTheDocument();
  });
});
