// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SecretService } from "@/services/secretService";
import type { Secret } from "@/types/secret";
import type { UnifiedRow } from "./types";

const secret: Secret = {
  id: "secret-1",
  name: "API_KEY",
  created_at: "2024-03-05T00:00:00Z",
  updated_at: "2024-03-05T00:00:00Z",
  created_by: "user-1",
  is_active: true
};

vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "proj-1" }, branchName: "main" })
}));
vi.mock("@/hooks/api/secrets/useSecrets", () => ({
  default: () => ({ data: { secrets: [secret] }, isLoading: false, error: null })
}));
vi.mock("@/hooks/api/secrets/useEnvSecrets", () => ({
  default: () => ({ data: [], isLoading: false, error: null })
}));
vi.mock("@/hooks/api/customApps/useCustomApps", () => ({
  useCustomApps: () => ({ data: [] })
}));
vi.mock("@/services/secretService", () => ({
  SecretService: { createSecret: vi.fn(), updateSecret: vi.fn(), deleteSecret: vi.fn() }
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
// Not under test: the detail dialog is reduced to its actions.
vi.mock("./components/SecretDetailDialog", () => ({
  SecretDetailDialog: ({
    row,
    onEdit,
    onDelete,
    onAddOverride
  }: {
    row: UnifiedRow | null;
    onEdit: (secret: Secret) => void;
    onDelete: (secret: Secret) => void;
    onAddOverride: (name: string) => void;
  }) =>
    row?.secretInfo ? (
      <>
        <button type='button' onClick={() => row.secretInfo && onEdit(row.secretInfo)}>
          Edit from detail
        </button>
        <button type='button' onClick={() => row.secretInfo && onDelete(row.secretInfo)}>
          Delete from detail
        </button>
        <button type='button' onClick={() => onAddOverride("NEW_KEY")}>
          Override from detail
        </button>
      </>
    ) : null
}));

import { UnifiedSecretsTable } from "./index";

const createSecret = vi.mocked(SecretService.createSecret);
const updateSecret = vi.mocked(SecretService.updateSecret);
const deleteSecret = vi.mocked(SecretService.deleteSecret);
const unhandledRejection = vi.fn();

/** Every promise callback already queued has run, and Node has reported any unhandled rejection. */
const settled = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

/** Opens the one secret's detail and takes the named action from it. */
const openFromDetail = (action: string) => {
  const queryClient = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
  render(
    <QueryClientProvider client={queryClient}>
      <UnifiedSecretsTable />
    </QueryClientProvider>
  );
  fireEvent.click(screen.getByText("API_KEY"));
  fireEvent.click(screen.getByRole("button", { name: action }));
};

/** Opens the table's delete confirmation for the one secret and confirms it. */
const confirmDelete = () => {
  openFromDetail("Delete from detail");
  fireEvent.click(screen.getByRole("button", { name: "Delete Secret" }));
};

beforeEach(() => {
  vi.spyOn(console, "error").mockImplementation(() => {});
  process.on("unhandledRejection", unhandledRejection);
});

afterEach(() => {
  process.off("unhandledRejection", unhandledRejection);
  cleanup();
  vi.clearAllMocks();
  vi.restoreAllMocks();
});

describe("UnifiedSecretsTable delete", () => {
  it("handles a failed delete: one toast, no unhandled rejection, dialog left open", async () => {
    deleteSecret.mockRejectedValue(new Error("403"));

    confirmDelete();

    await waitFor(() => expect(toast.error).toHaveBeenCalledWith("Failed to delete secret"));
    await settled();
    expect(toast.error).toHaveBeenCalledTimes(1);
    expect(unhandledRejection).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Delete Secret" })).toBeTruthy();
  });

  it("closes the dialog once the secret is deleted", async () => {
    deleteSecret.mockResolvedValue(undefined);

    confirmDelete();

    await waitFor(() => expect(screen.queryByRole("button", { name: "Delete Secret" })).toBeNull());
    expect(deleteSecret).toHaveBeenCalledWith("proj-1", "secret-1");
    expect(toast.success).toHaveBeenCalledWith("Secret deleted successfully");
  });

  it("sends one delete for a double click", async () => {
    let finish: () => void = () => {};
    deleteSecret.mockReturnValue(new Promise<void>((resolve) => (finish = resolve)));

    confirmDelete();
    const button = screen.getByRole<HTMLButtonElement>("button", { name: "Delete Secret" });
    await waitFor(() => expect(button).toBeDisabled());
    fireEvent.click(button);
    expect(deleteSecret).toHaveBeenCalledTimes(1);

    finish();
    await waitFor(() => expect(screen.queryByRole("button", { name: "Delete Secret" })).toBeNull());
  });
});

describe("UnifiedSecretsTable success toasts", () => {
  it("says a secret was created once", async () => {
    createSecret.mockResolvedValue({ ...secret, name: "NEW_KEY" });

    openFromDetail("Override from detail");
    fireEvent.change(screen.getByLabelText("Value *"), { target: { value: "s3cret" } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));

    await waitFor(() => expect(screen.queryByRole("button", { name: "Create" })).toBeNull());
    await settled();
    expect(createSecret).toHaveBeenCalledWith("proj-1", {
      name: "NEW_KEY",
      value: "s3cret",
      description: undefined
    });
    expect(vi.mocked(toast.success).mock.calls).toEqual([["Secret created successfully"]]);
  });

  it("says a secret was updated once", async () => {
    updateSecret.mockResolvedValue(secret);

    openFromDetail("Edit from detail");
    fireEvent.change(screen.getByLabelText("New Value *"), { target: { value: "rotated" } });
    fireEvent.click(screen.getByRole("button", { name: "Update Secret" }));

    await waitFor(() => expect(screen.queryByRole("button", { name: "Update Secret" })).toBeNull());
    await settled();
    expect(updateSecret).toHaveBeenCalledWith("proj-1", "secret-1", {
      value: "rotated",
      description: undefined
    });
    expect(vi.mocked(toast.success).mock.calls).toEqual([["Secret updated successfully"]]);
  });
});
