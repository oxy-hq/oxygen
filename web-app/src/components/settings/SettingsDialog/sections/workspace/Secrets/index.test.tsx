// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import queryKeys from "@/hooks/api/queryKey";
import { CustomAppsService } from "@/services/api/customApps";
import { SecretService } from "@/services/secretService";
import type { Secret, SecretListResponse } from "@/types/secret";

vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "proj-1" }, branchName: "main" })
}));
vi.mock("@/hooks/api/customApps/useCustomApps", () => ({
  useCustomApps: () => ({ data: [{ id: "app-1", name: "Storefront" }] })
}));
vi.mock("@/hooks/api/secrets/useSecrets", () => ({
  default: () => ({ data: { secrets: [] } })
}));
vi.mock("@/services/secretService", () => ({ SecretService: { createSecret: vi.fn() } }));
vi.mock("@/services/api/customApps", () => ({
  CustomAppsService: { setWorkspaceAppSecret: vi.fn() }
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
// Not under test: the role gate, the table, and Radix's select, which jsdom cannot open.
vi.mock("@/components/auth/Can", () => ({
  CanWorkspaceAdmin: ({ children }: { children: ReactNode }) => <>{children}</>
}));
vi.mock("@/components/settings/secrets/UnifiedSecretsTable", () => ({
  UnifiedSecretsTable: () => null
}));
vi.mock("@/components/ui/shadcn/select", () => ({
  Select: ({
    value,
    onValueChange,
    children
  }: {
    value: string;
    onValueChange: (value: string) => void;
    children: ReactNode;
  }) => (
    <select aria-label='Available to' value={value} onChange={(e) => onValueChange(e.target.value)}>
      {children}
    </select>
  ),
  SelectTrigger: () => null,
  SelectValue: () => null,
  SelectContent: ({ children }: { children: ReactNode }) => <>{children}</>,
  SelectItem: ({ value, children }: { value: string; children: ReactNode }) => (
    <option value={value}>{children}</option>
  )
}));

import Secrets from "./index";

const createSecret = vi.mocked(SecretService.createSecret);
const setWorkspaceAppSecret = vi.mocked(CustomAppsService.setWorkspaceAppSecret);

/** Every promise callback already queued has run by the time this resolves. */
const settled = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

/** A stored secret as the workspace's list holds it. */
const stored = (name: string): Secret => ({
  id: `id-of-${name}`,
  name,
  created_at: "2024-03-05T00:00:00Z",
  updated_at: "2024-03-05T00:00:00Z",
  created_by: "user-1",
  is_active: true
});

/**
 * Opens the create dialog and fills it in for a secret named API_KEY. `existing`
 * is the workspace's secrets list as the section has it loaded; `null` leaves it
 * not loaded at all.
 */
const fillInNewSecret = (existing: Secret[] | null = []) => {
  const queryClient = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
  if (existing) {
    const list: SecretListResponse = { secrets: existing, total: existing.length };
    queryClient.setQueryData(queryKeys.secret.list("proj-1"), list);
  }
  render(
    <QueryClientProvider client={queryClient}>
      <Secrets />
    </QueryClientProvider>
  );
  fireEvent.click(screen.getByRole("button", { name: "Create" }));
  fireEvent.change(screen.getByLabelText("Name *"), { target: { value: "API_KEY" } });
  fireEvent.change(screen.getByLabelText("Value *"), { target: { value: "s3cret" } });
};

/** Submits the dialog and waits for it to close. */
const submit = async () => {
  fireEvent.click(screen.getByRole("button", { name: "Create" }));
  await waitFor(() => expect(screen.queryByLabelText("Name *")).toBeNull());
  await settled();
};

beforeEach(() => {
  vi.spyOn(console, "error").mockImplementation(() => {});
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  vi.restoreAllMocks();
});

describe("Secrets section create", () => {
  it("says a workspace secret was created once", async () => {
    createSecret.mockResolvedValue({
      id: "secret-1",
      name: "API_KEY",
      created_at: "2024-03-05T00:00:00Z",
      updated_at: "2024-03-05T00:00:00Z",
      created_by: "user-1",
      is_active: true
    });

    fillInNewSecret();
    await submit();

    expect(createSecret).toHaveBeenCalledTimes(1);
    expect(vi.mocked(toast.success).mock.calls).toEqual([["Secret created successfully"]]);
  });

  it("says an app secret was created once", async () => {
    setWorkspaceAppSecret.mockResolvedValue(undefined);

    fillInNewSecret();
    fireEvent.change(screen.getByLabelText("Available to"), { target: { value: "app-1" } });
    await submit();

    expect(setWorkspaceAppSecret).toHaveBeenCalledWith("proj-1", "app-1", "API_KEY", "s3cret");
    expect(createSecret).not.toHaveBeenCalled();
    expect(vi.mocked(toast.success).mock.calls).toEqual([["Secret created successfully"]]);
  });

  /** Submits the dialog for app-1's API_KEY, given what the workspace already stores. */
  const saveAppSecret = async (existing: Secret[] | null) => {
    setWorkspaceAppSecret.mockResolvedValue(undefined);
    fillInNewSecret(existing);
    fireEvent.change(screen.getByLabelText("Available to"), { target: { value: "app-1" } });
    await submit();
    return vi.mocked(toast.success).mock.calls;
  };

  it("says an app secret was updated when the app already had that key", async () => {
    // The same endpoint rotates an existing key: nothing was created.
    expect(await saveAppSecret([stored("apps/app-1/API_KEY")])).toEqual([
      ["Secret updated successfully"]
    ]);
  });

  it("says created when only another app, or the workspace, has a key of that name", async () => {
    expect(await saveAppSecret([stored("apps/app-2/API_KEY"), stored("API_KEY")])).toEqual([
      ["Secret created successfully"]
    ]);
  });

  it("claims neither when the secrets list is not loaded to tell which it was", async () => {
    expect(await saveAppSecret(null)).toEqual([["Secret saved successfully"]]);
  });
});
