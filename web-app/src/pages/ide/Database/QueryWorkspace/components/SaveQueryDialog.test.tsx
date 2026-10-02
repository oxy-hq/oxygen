// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { QueryTab } from "@/stores/useDatabaseClient";
import SaveQueryDialog from "./SaveQueryDialog";

// The Save button is disabled while a save is in flight; Enter in the name field has to
// be held to the same rule, or a double Enter saves the file twice.

vi.setConfig({ testTimeout: 20000 });

const { saveFile } = vi.hoisted(() => ({ saveFile: vi.fn() }));

vi.mock("@/services/api", () => ({ FileService: { saveFile } }));
vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "p1" }, branchName: "main" })
}));
vi.mock("@/stores/useDatabaseClient", () => ({ default: () => ({ updateTab: vi.fn() }) }));
vi.mock("sonner", () => ({ toast: { error: vi.fn(), success: vi.fn() } }));

const tab = { id: "t1", name: "orders", content: "select 1" } as QueryTab;

const renderDialog = () =>
  render(
    <QueryClientProvider client={new QueryClient()}>
      <SaveQueryDialog open onOpenChange={vi.fn()} tab={tab} />
    </QueryClientProvider>
  );

afterEach(() => cleanup());
beforeEach(() => {
  saveFile.mockReset();
});

describe("SaveQueryDialog", () => {
  it("saves once when Enter is pressed again while the save is in flight", async () => {
    // Never settles, so the first save is still in flight for the second Enter.
    saveFile.mockReturnValue(new Promise(() => {}));
    renderDialog();

    await userEvent.type(screen.getByLabelText("File Name"), "{Enter}{Enter}");

    expect(saveFile).toHaveBeenCalledTimes(1);
  });

  it("saves again on Enter once a failed save has settled", async () => {
    saveFile.mockRejectedValueOnce(new Error("disk full")).mockResolvedValueOnce(undefined);
    renderDialog();
    const input = screen.getByLabelText("File Name");

    await userEvent.type(input, "{Enter}");
    expect(await screen.findByText("disk full")).toBeTruthy();
    await userEvent.type(input, "{Enter}");

    expect(saveFile).toHaveBeenCalledTimes(2);
  });
});
