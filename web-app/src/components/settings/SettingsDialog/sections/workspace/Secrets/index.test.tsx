// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import queryKeys from "@/hooks/api/queryKey";
import { type AppSecretWrite, CustomAppsService } from "@/services/api/customApps";
import { SecretService } from "@/services/secretService";
import type { Secret, SecretListResponse } from "@/types/secret";
import type { SharedSecretsResponse } from "@/types/sharedSecrets";

vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "proj-1" }, branchName: "main" })
}));
const listed = vi.hoisted(() => ({ secrets: [] as Secret[] }));
vi.mock("@/hooks/api/secrets/useSecrets", () => ({
  default: () => ({ data: { secrets: listed.secrets } })
}));
vi.mock("@/services/secretService", () => ({
  SecretService: { createSecret: vi.fn(), listShared: vi.fn(), shareWithApps: vi.fn() }
}));
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
    <select aria-label='App' value={value} onChange={(e) => onValueChange(e.target.value)}>
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
const listShared = vi.mocked(SecretService.listShared);
const shareWithApps = vi.mocked(SecretService.shareWithApps);

/** The workspace has one app, Storefront, whose build declares API_KEY. */
const sharing: SharedSecretsResponse = {
  shared: [],
  apps: [
    {
      app_id: "app-1",
      app_name: "Storefront",
      app_slug: "storefront",
      missing: [{ key: "API_KEY", required: true }]
    }
  ]
};
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
const fillInNewSecret = async (existing: Secret[] | null = []) => {
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
  // The apps arrive with the sharing view.
  await screen.findByTestId("secret-scope-app");
};

/** Stores the secret as Storefront's own rather than the workspace's. */
const chooseOneApp = () => fireEvent.click(screen.getByTestId("secret-scope-app"));

/** Submits the dialog and waits for it to close. */
const submit = async () => {
  fireEvent.click(screen.getByTestId("secret-create-submit"));
  await waitFor(() => expect(screen.queryByLabelText("Name *")).toBeNull());
  await settled();
};

beforeEach(() => {
  vi.spyOn(console, "error").mockImplementation(() => {});
  listShared.mockResolvedValue(sharing);
  listed.secrets = [];
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

    await fillInNewSecret();
    await submit();

    expect(createSecret).toHaveBeenCalledTimes(1);
    expect(shareWithApps).not.toHaveBeenCalled();
    expect(vi.mocked(toast.success).mock.calls).toEqual([["Secret created successfully"]]);
  });

  it("says an app secret was created once", async () => {
    setWorkspaceAppSecret.mockResolvedValue("created");

    await fillInNewSecret();
    chooseOneApp();
    fireEvent.change(screen.getByLabelText("App"), { target: { value: "app-1" } });
    await submit();

    expect(setWorkspaceAppSecret).toHaveBeenCalledWith("proj-1", "app-1", "API_KEY", "s3cret");
    expect(createSecret).not.toHaveBeenCalled();
    expect(vi.mocked(toast.success).mock.calls).toEqual([["Secret created successfully"]]);
  });

  /**
   * Submits the dialog for app-1's API_KEY. `existing` is the workspace's list
   * as loaded; `write` is what the server answers the endpoint did.
   */
  const saveAppSecret = async (existing: Secret[] | null, write: AppSecretWrite) => {
    setWorkspaceAppSecret.mockResolvedValue(write);
    await fillInNewSecret(existing);
    chooseOneApp();
    await submit();
    return vi.mocked(toast.success).mock.calls;
  };

  it("says an app secret was updated when the server replaced an existing key", async () => {
    // The same endpoint rotates an existing key: nothing was created.
    expect(await saveAppSecret([stored("apps/app-1/API_KEY")], "updated")).toEqual([
      ["Secret updated successfully"]
    ]);
  });

  // The loaded list can be stale (another tab deleted or added the key) or not
  // loaded at all; only the server's answer says what this write did.
  it("goes by what the server did, not by the loaded list", async () => {
    expect(await saveAppSecret([stored("apps/app-1/API_KEY")], "created")).toEqual([
      ["Secret created successfully"]
    ]);
    cleanup();
    vi.mocked(toast.success).mockClear();
    expect(await saveAppSecret(null, "updated")).toEqual([["Secret updated successfully"]]);
  });

  it("shares a new workspace secret with the apps ticked, in one step and one toast", async () => {
    createSecret.mockResolvedValue({
      id: "secret-1",
      name: "API_KEY",
      created_at: "2024-03-05T00:00:00Z",
      updated_at: "2024-03-05T00:00:00Z",
      created_by: "user-1",
      is_active: true
    });
    shareWithApps.mockResolvedValue({ secret_id: "secret-1", app_ids: ["app-1"] });

    await fillInNewSecret();
    fireEvent.click(screen.getByLabelText("Storefront"));
    await submit();

    expect(shareWithApps).toHaveBeenCalledWith("proj-1", "secret-1", ["app-1"]);
    expect(setWorkspaceAppSecret).not.toHaveBeenCalled();
    expect(vi.mocked(toast.success).mock.calls).toEqual([["Secret created and shared with 1 app"]]);
  });
});

describe("Secrets section: apps missing a key", () => {
  const renderSection = () =>
    render(
      <QueryClientProvider
        client={new QueryClient({ defaultOptions: { mutations: { retry: false } } })}
      >
        <Secrets />
      </QueryClientProvider>
    );

  it("opens create with the name filled in and the app ticked", async () => {
    renderSection();
    fireEvent.click(await screen.findByTestId("secrets-missing-create-API_KEY"));

    expect(screen.getByLabelText("Name *")).toHaveValue("API_KEY");
    expect(await screen.findByLabelText("Storefront")).toBeChecked();
  });

  it("shares the workspace secret of that name in one click", async () => {
    listed.secrets = [stored("API_KEY"), stored("apps/app-2/API_KEY")];
    shareWithApps.mockResolvedValue({ secret_id: "id-of-API_KEY", app_ids: ["app-1"] });
    renderSection();

    fireEvent.click(await screen.findByTestId("secrets-missing-share-API_KEY"));

    await waitFor(() =>
      expect(shareWithApps).toHaveBeenCalledWith("proj-1", "id-of-API_KEY", ["app-1"])
    );
  });
});
