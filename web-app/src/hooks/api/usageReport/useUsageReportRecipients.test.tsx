// @vitest-environment jsdom
import { renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { UsageReportService } from "@/services/api/usageReport";
import { everyone, seededClient } from "./testSupport";

vi.mock("@/services/api/usageReport", () => ({
  UsageReportService: { recipients: vi.fn() }
}));

import { useUsageReportRecipients } from "./useUsageReportRecipients";

const recipients = vi.mocked(UsageReportService.recipients);

afterEach(() => {
  vi.clearAllMocks();
});

/**
 * The endpoint needs `manage_platform_grants`. The section that shows this list passes
 * the caller's standing as `enabled`; this is the half that has to honour it.
 */
describe("useUsageReportRecipients", () => {
  it("asks the server when enabled, and hands back its answer", async () => {
    recipients.mockResolvedValue(everyone());
    const { wrapper } = seededClient({});
    const { result } = renderHook(() => useUsageReportRecipients({ enabled: true }), { wrapper });
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(recipients).toHaveBeenCalledTimes(1);
    expect(result.current.data).toEqual(everyone());
  });

  it("asks by default", async () => {
    recipients.mockResolvedValue(everyone());
    const { wrapper } = seededClient({});
    const { result } = renderHook(() => useUsageReportRecipients(), { wrapper });
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(recipients).toHaveBeenCalledTimes(1);
  });

  it("asks nothing when disabled", async () => {
    recipients.mockResolvedValue(everyone());
    const { wrapper } = seededClient({});
    const { result } = renderHook(() => useUsageReportRecipients({ enabled: false }), { wrapper });
    // Give a fetch every chance to start before concluding that none did.
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(recipients).not.toHaveBeenCalled();
    expect(result.current.fetchStatus).toBe("idle");
    expect(result.current.data).toBeUndefined();
  });
});
