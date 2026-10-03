// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { AxiosError, type AxiosResponse } from "axios";
import { MemoryRouter } from "react-router-dom";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SecretService } from "@/services/secretService";

vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "proj-1" }, branchName: "main" })
}));
vi.mock("@/services/api/axios", () => ({ apiClient: {} }));
vi.mock("@/hooks/api/databases/useDatabases", () => ({
  default: () => ({ data: [{ name: "warehouse", db_type: "postgres" }], isLoading: false })
}));
const files = vi.hoisted(() => ({ create: vi.fn(), save: vi.fn() }));
vi.mock("@/hooks/api/files/useCreateFile", () => ({
  default: () => ({ mutateAsync: files.create })
}));
vi.mock("@/hooks/api/files/useSaveFile", () => ({
  default: () => ({ mutateAsync: files.save })
}));
vi.mock("@/hooks/api/quickbooks/useQuickBooksConnect", () => ({
  useQuickBooksConnect: () => ({ connect: vi.fn(), connecting: false })
}));
vi.mock("@/hooks/api/airway/useAirway", () => ({
  useDiscoverSourceTables: () => ({ mutateAsync: vi.fn(), isPending: false })
}));
vi.mock("@/services/secretService", () => ({
  SecretService: { createSecret: vi.fn() }
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

import NewPipelineDialog from "./NewPipelineDialog";

const createSecret = vi.mocked(SecretService.createSecret);

/** The error axios rejects with when the server answers `status` with `body`. */
const answered = (status: number, body: unknown) =>
  new AxiosError(`Request failed with status code ${status}`, "ERR_BAD_REQUEST", undefined, null, {
    status,
    data: body
  } as AxiosResponse);

/** Every promise callback already queued has run by the time this resolves. */
const settled = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

const type = (label: string, value: string) =>
  fireEvent.change(screen.getByLabelText(label), { target: { value } });

/** A Toast POS pipeline into `warehouse`, its client secret pasted, submitted. */
const createToastPipeline = async () => {
  const onCreated = vi.fn();
  const queryClient = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
  render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter>
        <NewPipelineDialog open onOpenChange={() => {}} existingNames={[]} onCreated={onCreated} />
      </MemoryRouter>
    </QueryClientProvider>
  );

  fireEvent.click(screen.getByTestId("pipeline-source-toast"));
  fireEvent.click(screen.getByTestId("pipeline-dest-warehouse"));
  type("Name", "toast_raw");
  type("Client ID", "client-1");
  type("Client secret", "s3cret");
  type("Secret name", "TOAST_CLIENT_SECRET");
  type("Restaurant GUID(s)", "guid-1");
  fireEvent.click(screen.getByTestId("pipeline-create-button"));

  await waitFor(() =>
    expect(screen.getByTestId("pipeline-create-button").textContent).toBe("Create")
  );
  await settled();
  return { onCreated };
};

beforeEach(() => {
  vi.spyOn(console, "error").mockImplementation(() => {});
  files.create.mockResolvedValue(undefined);
  files.save.mockResolvedValue(undefined);
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  vi.restoreAllMocks();
});

describe("NewPipelineDialog when creating fails", () => {
  it("says once that the secret could not be stored, and why", async () => {
    createSecret.mockRejectedValue(
      answered(409, { error: "Secret with name 'TOAST_CLIENT_SECRET' already exists" })
    );

    const { onCreated } = await createToastPipeline();

    // The secret's own toast says what failed. The dialog used to add a second,
    // "Failed to create pipeline", for the same failure.
    expect(vi.mocked(toast.error).mock.calls).toEqual([
      [
        "Failed to create secret TOAST_CLIENT_SECRET",
        { description: "Secret with name 'TOAST_CLIENT_SECRET' already exists" }
      ]
    ]);
    expect(files.create).not.toHaveBeenCalled();
    expect(onCreated).not.toHaveBeenCalled();
  });

  it("says the pipeline was not created when its file could not be written", async () => {
    createSecret.mockResolvedValue({
      id: "secret-1",
      name: "TOAST_CLIENT_SECRET",
      created_at: "2024-03-05T00:00:00Z",
      updated_at: "2024-03-05T00:00:00Z",
      created_by: "user-1",
      is_active: true
    });
    files.save.mockRejectedValue(answered(500, { error: "No space left on device" }));

    const { onCreated } = await createToastPipeline();

    // In the server's words: axios's own message, "Request failed with status
    // code 500", used to be the whole of it.
    expect(vi.mocked(toast.error).mock.calls).toEqual([
      ["Failed to create pipeline", { description: "No space left on device" }]
    ]);
    expect(onCreated).not.toHaveBeenCalled();
  });
});
