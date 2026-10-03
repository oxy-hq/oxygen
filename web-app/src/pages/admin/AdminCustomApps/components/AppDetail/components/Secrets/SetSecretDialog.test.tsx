// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { toast } from "sonner";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CustomAppsService } from "@/services/api/customApps";

vi.mock("@/services/api/customApps", () => ({ CustomAppsService: { setSecret: vi.fn() } }));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

import { SetSecretDialog } from "./SetSecretDialog";

const setSecret = vi.mocked(CustomAppsService.setSecret);

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

/** Opens the dialog the way the panel does (`""` = Add secret, a name = that
 *  row's button), saves `key` with a value, and waits for it to close. */
const save = async (secretKey: string, key: string) => {
  const onClose = vi.fn();
  render(
    <QueryClientProvider client={new QueryClient()}>
      <SetSecretDialog appId='app-1' secretKey={secretKey} stored={false} onClose={onClose} />
    </QueryClientProvider>
  );
  if (!secretKey) fireEvent.change(screen.getByLabelText("Key"), { target: { value: key } });
  fireEvent.change(screen.getByLabelText("Value"), { target: { value: "s3cret" } });
  fireEvent.click(screen.getByTestId("admin-app-secret-dialog-save"));
  await waitFor(() => expect(onClose).toHaveBeenCalled());
  expect(setSecret).toHaveBeenCalledWith("app-1", key, "s3cret");
};

// The write is an upsert, so how the dialog was opened does not say what it
// did: a Missing row's button opens it on a key nothing stores yet, and Add
// secret takes any name, including one already stored. The server's answer does.
describe("SetSecretDialog — the toast says what the write did", () => {
  it("says Set when a declared key is stored for the first time from its row", async () => {
    setSecret.mockResolvedValue("created");
    await save("API_KEY", "API_KEY");
    expect(vi.mocked(toast.success).mock.calls).toEqual([["Set API_KEY."]]);
  });

  it("says Rotated when a key typed into Add secret was already stored", async () => {
    setSecret.mockResolvedValue("updated");
    await save("", "API_KEY");
    expect(vi.mocked(toast.success).mock.calls).toEqual([["Rotated API_KEY."]]);
  });
});
