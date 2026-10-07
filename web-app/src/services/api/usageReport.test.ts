import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("./axios", () => ({
  apiClient: { get: vi.fn(), put: vi.fn(), post: vi.fn() }
}));

import { apiClient } from "./axios";
import { UsageReportService } from "./usageReport";

const put = vi.mocked(apiClient.put);
const get = vi.mocked(apiClient.get);

afterEach(() => {
  vi.clearAllMocks();
});

describe("UsageReportService.setRecipient", () => {
  it("puts the address in the path, encoded, and the choice in the body", async () => {
    put.mockResolvedValue({ data: { email: "ada@oxy.tech", enabled: false } });
    const saved = await UsageReportService.setRecipient("ada@oxy.tech", false);
    expect(put.mock.calls).toEqual([
      ["/admin/usage-report/recipients/ada%40oxy.tech", { enabled: false }]
    ]);
    expect(saved).toEqual({ email: "ada@oxy.tech", enabled: false });
  });

  it("encodes the characters an address can carry that a path cannot", async () => {
    // A plus-address is the common one: left raw, `+` reaches some servers as a space,
    // and the request is then about a person who is not on the list.
    put.mockResolvedValue({ data: {} });
    await UsageReportService.setRecipient("ada+reports@oxy.tech", true);
    await UsageReportService.setRecipient("a/b?c#d@oxy.tech", true);
    expect(put.mock.calls.map(([url]) => url)).toEqual([
      "/admin/usage-report/recipients/ada%2Breports%40oxy.tech",
      "/admin/usage-report/recipients/a%2Fb%3Fc%23d%40oxy.tech"
    ]);
  });
});

describe("UsageReportService.recipients", () => {
  it("reads the list from its own route", async () => {
    get.mockResolvedValue({ data: { recipients: [] } });
    expect(await UsageReportService.recipients()).toEqual({ recipients: [] });
    expect(get.mock.calls).toEqual([["/admin/usage-report/recipients"]]);
  });
});
