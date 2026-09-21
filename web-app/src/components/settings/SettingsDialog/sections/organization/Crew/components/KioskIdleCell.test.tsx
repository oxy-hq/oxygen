// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_IDLE_TIMEOUT_SECONDS } from "@/libs/frontline";
import type { KioskDeviceRow } from "@/types/frontline";
import { KioskIdleCell } from "./KioskIdleCell";

const updateDevice = vi.fn();

// The hook is the seam. What this file is about is the three bodies the cell
// can send — a number, `null`, and nothing at all — not React Query's plumbing.
vi.mock("@/hooks/api/organizations", () => ({
  useUpdateDevice: () => ({ mutateAsync: updateDevice, isPending: false })
}));

afterEach(() => {
  cleanup();
  updateDevice.mockReset();
});

const DEVICE = "k1";

const kiosk = (idle: number): KioskDeviceRow => ({
  id: DEVICE,
  name: "Front counter",
  return_to: null,
  created_at: "2026-09-01T00:00:00Z",
  bound_at: "2026-09-01T01:00:00Z",
  last_seen_at: null,
  revoked_at: null,
  enrol_expires_at: null,
  location_id: null,
  location_name: null,
  idle_timeout_seconds: idle
});

const cell = (device: KioskDeviceRow, editable = true) =>
  render(
    <QueryClientProvider client={new QueryClient()}>
      <KioskIdleCell orgId='org-1' device={device} editable={editable} />
    </QueryClientProvider>
  );

const openEditor = () =>
  fireEvent.click(screen.getByTestId(`settings-crew-kiosk-idle-edit-${DEVICE}`));
const box = () => screen.getByTestId(`settings-crew-kiosk-idle-input-${DEVICE}`);
const saveButton = () => screen.getByTestId(`settings-crew-kiosk-idle-save-${DEVICE}`);

describe("KioskIdleCell", () => {
  it("sends the minutes the admin typed, in seconds", async () => {
    updateDevice.mockResolvedValue(kiosk(900));
    cell(kiosk(60));
    openEditor();
    fireEvent.change(box(), { target: { value: "15" } });
    fireEvent.click(saveButton());
    await waitFor(() =>
      expect(updateDevice).toHaveBeenCalledWith({
        orgId: "org-1",
        deviceId: DEVICE,
        request: { idle_timeout_seconds: 900 }
      })
    );
  });

  it("clears with null when the box is emptied, so the kiosk follows the default", async () => {
    // Not an omitted field — absent means "leave the row alone" on a PATCH, and
    // this is the one action that could not be expressed before the route.
    updateDevice.mockResolvedValue(kiosk(DEFAULT_IDLE_TIMEOUT_SECONDS));
    cell(kiosk(60));
    openEditor();
    fireEvent.change(box(), { target: { value: "" } });
    fireEvent.click(saveButton());
    await waitFor(() =>
      expect(updateDevice).toHaveBeenCalledWith({
        orgId: "org-1",
        deviceId: DEVICE,
        request: { idle_timeout_seconds: null }
      })
    );
  });

  it("opens empty for a kiosk on the default, exactly as New kiosk does", () => {
    cell(kiosk(DEFAULT_IDLE_TIMEOUT_SECONDS));
    expect(screen.getByTestId(`settings-crew-kiosk-idle-edit-${DEVICE}`).textContent).toBe(
      "30 minutes (default)"
    );
    openEditor();
    expect((box() as HTMLInputElement).value).toBe("");
  });

  it("refuses inline what the server would 400 on, and sends nothing", () => {
    cell(kiosk(60));
    openEditor();
    fireEvent.change(box(), { target: { value: "721" } });
    expect(screen.getByTestId(`settings-crew-kiosk-idle-hint-${DEVICE}`).textContent).toContain(
      "Whole minutes"
    );
    fireEvent.click(saveButton());
    expect(updateDevice).not.toHaveBeenCalled();
  });

  it("shows the server's refusal beside the box rather than swallowing it", async () => {
    // `isAxiosError` is what `apiErrorMessage` branches on to read `{ error }`
    // out of the body — without it this would render the generic fallback and
    // the test would pass while the real 409's sentence never reached anyone.
    updateDevice.mockRejectedValue({
      isAxiosError: true,
      response: { status: 409, data: { error: "a revoked kiosk cannot be changed" } }
    });
    cell(kiosk(60));
    openEditor();
    fireEvent.change(box(), { target: { value: "15" } });
    fireEvent.click(saveButton());
    await waitFor(() =>
      expect(screen.getByTestId(`settings-crew-kiosk-idle-hint-${DEVICE}`).textContent).toContain(
        "revoked kiosk"
      )
    );
    // Still editing: a refusal must not close the box over the admin's number.
    expect(box()).toBeTruthy();
  });

  it("is a plain label, not a control, for a revoked kiosk", () => {
    cell(kiosk(60), false);
    expect(screen.queryByTestId(`settings-crew-kiosk-idle-edit-${DEVICE}`)).toBeNull();
    expect(screen.getByText("1 minute")).toBeTruthy();
  });
});
