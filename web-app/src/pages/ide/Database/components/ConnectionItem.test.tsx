// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SidebarProvider } from "@/components/ui/shadcn/sidebar";
import type { DatabaseInfo, DatabaseSchema } from "@/types/database";
import { ConnectionItem } from "./ConnectionItem";

// ── Module mocks ──────────────────────────────────────────────────────────────

const mockRefetch = vi.fn();
let mockSchemaState: {
  data: DatabaseSchema | undefined;
  isLoading: boolean;
  isError: boolean;
  isFetching: boolean;
  refetch: typeof mockRefetch;
} = { data: undefined, isLoading: false, isError: false, isFetching: false, refetch: mockRefetch };

vi.mock("@/hooks/api/databases/useDatabaseSchema", () => ({
  default: (_dbName: string, enabled: boolean) => {
    if (!enabled) return { ...mockSchemaState, data: undefined, isLoading: false };
    return mockSchemaState;
  }
}));

vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "proj-1" }, branchName: "main", isMainEditMode: false })
}));

vi.mock("@/stores/useCurrentOrg", () => ({
  default: (selector: (s: { org: { slug: string } }) => unknown) =>
    selector({ org: { slug: "my-org" } })
}));

vi.mock("@/stores/useDatabaseClient", () => ({
  default: () => ({ addTab: vi.fn().mockReturnValue({ success: true }) })
}));

// ── Helpers ───────────────────────────────────────────────────────────────────

const mockDb: DatabaseInfo = {
  name: "my-postgres",
  dialect: "postgres",
  db_type: "postgres",
  datasets: {},
  synced: false
};

const mockSchema: DatabaseSchema = {
  tables: [{ name: "users", columns: [{ name: "id", data_type: "int4" }] }]
};

function renderItem() {
  const client = new QueryClient();
  return render(
    <MemoryRouter>
      <QueryClientProvider client={client}>
        <SidebarProvider>
          <ConnectionItem database={mockDb} />
        </SidebarProvider>
      </QueryClientProvider>
    </MemoryRouter>
  );
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  mockSchemaState = {
    data: undefined,
    isLoading: false,
    isError: false,
    isFetching: false,
    refetch: mockRefetch
  };
});

// ── Tests ─────────────────────────────────────────────────────────────────────

describe("ConnectionItem", () => {
  it("renders the database name", () => {
    renderItem();
    expect(screen.getByText("my-postgres")).toBeInTheDocument();
  });

  it("refresh button is always visible (not hidden)", () => {
    renderItem();
    const btn = screen.getByRole("button", { name: /refresh schema/i });
    expect(btn).toBeInTheDocument();
    // should not have opacity-0 class
    expect(btn.className).not.toContain("opacity-0");
  });

  it("schema content is not rendered when collapsed", () => {
    mockSchemaState = { ...mockSchemaState, data: mockSchema };
    renderItem();
    expect(screen.queryByText("users")).not.toBeInTheDocument();
  });

  it("shows loading state when expanding and schema is loading", async () => {
    mockSchemaState = { ...mockSchemaState, isLoading: true };
    renderItem();
    await userEvent.click(screen.getByText("my-postgres"));
    expect(await screen.findByText(/fetching schema/i)).toBeInTheDocument();
  });

  it("shows error state when schema fetch fails", async () => {
    mockSchemaState = { ...mockSchemaState, isError: true };
    renderItem();
    await userEvent.click(screen.getByText("my-postgres"));
    expect(await screen.findByText(/failed to load schema/i)).toBeInTheDocument();
    expect(screen.getByText("Retry")).toBeInTheDocument();
  });

  it("shows table list after successful fetch", async () => {
    mockSchemaState = { ...mockSchemaState, data: mockSchema };
    renderItem();
    await userEvent.click(screen.getByText("my-postgres"));
    expect(await screen.findByText("users")).toBeInTheDocument();
  });

  it("calls refetch when refresh button is clicked", async () => {
    mockSchemaState = { ...mockSchemaState, data: mockSchema, refetch: mockRefetch };
    renderItem();
    await userEvent.click(screen.getByRole("button", { name: /refresh schema/i }));
    await waitFor(() => expect(mockRefetch).toHaveBeenCalledTimes(1));
  });

  it("shows 'No tables found' when schema is empty", async () => {
    mockSchemaState = { ...mockSchemaState, data: { tables: [] } };
    renderItem();
    await userEvent.click(screen.getByText("my-postgres"));
    expect(await screen.findByText(/no tables found/i)).toBeInTheDocument();
  });
});
