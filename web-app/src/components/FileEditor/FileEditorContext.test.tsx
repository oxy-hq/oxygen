// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AxiosError } from "axios";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { encodeBase64 } from "@/libs/encoding";
import { FileEditorProvider } from "./FileEditorContext";
import { useFileEditorContext } from "./useFileEditorContext";

// A save that fails used to do nothing but put the file back to "modified" — and from
// the "save and navigate" dialog, which has already closed by then, that reads as the
// button doing nothing at all. These pin that both save paths say so.

type SaveCallbacks = { onSuccess?: () => void; onError?: (error: unknown) => void };
const saveFile = vi.fn<(vars: unknown, callbacks: SaveCallbacks) => void>();

vi.mock("@/hooks/api/files/useFile", () => ({
  default: () => ({ data: "select 1", isPending: false, isSuccess: true })
}));
vi.mock("@/hooks/api/files/useFileGit", () => ({ default: () => ({ data: undefined }) }));
vi.mock("@/hooks/api/files/useSaveFile", () => ({ default: () => ({ mutate: saveFile }) }));
vi.mock("sonner", () => ({ toast: { error: vi.fn(), success: vi.fn() } }));

const PATH_B64 = encodeBase64("queries/orders.sql");

const Consumer = ({ onSuccess }: { onSuccess: () => void }) => {
  const { state, actions } = useFileEditorContext();
  return (
    <>
      <span data-testid='file-state'>{state.fileState}</span>
      <button type='button' onClick={() => actions.setContent("select 2")}>
        edit
      </button>
      <button type='button' onClick={() => void actions.save(onSuccess)}>
        save
      </button>
    </>
  );
};

afterEach(() => cleanup());
beforeEach(() => {
  saveFile.mockReset();
  vi.mocked(toast.error).mockReset();
  vi.spyOn(console, "error").mockImplementation(() => {});
});

describe("FileEditorProvider save failure", () => {
  it("tells the user when the save request fails", async () => {
    saveFile.mockImplementation((_vars, callbacks) => callbacks.onError?.(new Error("disk full")));
    const onSuccess = vi.fn();
    render(
      <FileEditorProvider pathb64={PATH_B64}>
        <Consumer onSuccess={onSuccess} />
      </FileEditorProvider>
    );

    await userEvent.click(screen.getByText("edit"));
    await userEvent.click(screen.getByText("save"));

    expect(toast.error).toHaveBeenCalledTimes(1);
    expect(toast.error).toHaveBeenCalledWith("Failed to save orders.sql", {
      description: "disk full"
    });
    // The edit is still there to retry, and the blocked navigation did not proceed.
    expect(screen.getByTestId("file-state").textContent).toBe("modified");
    expect(onSuccess).not.toHaveBeenCalled();
  });

  it("tells the user when the save-to-new-branch override rejects", async () => {
    const onSaveOverride = vi.fn().mockRejectedValue(new Error("branch already exists"));
    const onSuccess = vi.fn();
    render(
      <FileEditorProvider pathb64={PATH_B64} onSaveOverride={onSaveOverride}>
        <Consumer onSuccess={onSuccess} />
      </FileEditorProvider>
    );

    await userEvent.click(screen.getByText("edit"));
    await userEvent.click(screen.getByText("save"));

    expect(onSaveOverride).toHaveBeenCalledTimes(1);
    expect(saveFile).not.toHaveBeenCalled();
    expect(toast.error).toHaveBeenCalledTimes(1);
    expect(toast.error).toHaveBeenCalledWith("Failed to save orders.sql", {
      description: "branch already exists"
    });
    expect(screen.getByTestId("file-state").textContent).toBe("modified");
    expect(onSuccess).not.toHaveBeenCalled();
  });

  it("does not repeat a refusal the HTTP client has already toasted", async () => {
    // A 403 gets the client's own "You don't have permission to do this." toast.
    const forbidden = new AxiosError("Request failed with status code 403");
    forbidden.response = { status: 403, data: {} } as AxiosError["response"];
    saveFile.mockImplementation((_vars, callbacks) => callbacks.onError?.(forbidden));
    const onSuccess = vi.fn();
    render(
      <FileEditorProvider pathb64={PATH_B64}>
        <Consumer onSuccess={onSuccess} />
      </FileEditorProvider>
    );

    await userEvent.click(screen.getByText("edit"));
    await userEvent.click(screen.getByText("save"));

    expect(toast.error).not.toHaveBeenCalled();
    expect(screen.getByTestId("file-state").textContent).toBe("modified");
    expect(onSuccess).not.toHaveBeenCalled();
  });

  it("stays quiet when the save succeeds", async () => {
    saveFile.mockImplementation((_vars, callbacks) => callbacks.onSuccess?.());
    const onSuccess = vi.fn();
    render(
      <FileEditorProvider pathb64={PATH_B64}>
        <Consumer onSuccess={onSuccess} />
      </FileEditorProvider>
    );

    await userEvent.click(screen.getByText("edit"));
    await userEvent.click(screen.getByText("save"));

    expect(toast.error).not.toHaveBeenCalled();
    expect(screen.getByTestId("file-state").textContent).toBe("saved");
    expect(onSuccess).toHaveBeenCalledTimes(1);
  });
});
