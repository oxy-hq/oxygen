import { beforeEach, describe, expect, it, vi } from "vitest";
import queryKeys from "../queryKey";
import { orgInventoryEndpoints, serviceAccountTokenEndpoints } from "./tokenEndpoints";

const service = vi.hoisted(() => ({
  extendToken: vi.fn(),
  tokenActivity: vi.fn(),
  inventoryActivity: vi.fn()
}));

vi.mock("@/services/api/orgApiAccess", () => ({
  ServiceAccountService: {
    extendToken: service.extendToken,
    tokenActivity: service.tokenActivity
  },
  OrgTokenService: { activity: service.inventoryActivity }
}));

beforeEach(() => {
  for (const fn of Object.values(service)) fn.mockReset();
});

describe("serviceAccountTokenEndpoints", () => {
  const endpoints = serviceAccountTokenEndpoints("org-1", "sa-1");

  it("extends and reads activity through the account's own routes", async () => {
    service.extendToken.mockResolvedValue({ name: "nightly", expires_at: null });
    await endpoints.extend("t-1", { days: 30 });
    expect(service.extendToken).toHaveBeenCalledWith("org-1", "sa-1", "t-1", { days: 30 });

    await endpoints.activity("t-1", 100);
    expect(service.tokenActivity).toHaveBeenCalledWith("org-1", "sa-1", "t-1", 100);
  });

  it("refreshes the account's lists and the org inventory after an extend", () => {
    expect(endpoints.keys.lists).toEqual([
      queryKeys.org.serviceAccounts("org-1"),
      queryKeys.org.tokenInventoryAll("org-1")
    ]);
    // The token list sits under the accounts prefix, so the first key covers it.
    expect(queryKeys.org.serviceAccountTokens("org-1", "sa-1").slice(0, 3)).toEqual(
      queryKeys.org.serviceAccounts("org-1")
    );
    expect(endpoints.keys.activity("t-1")).toEqual(
      queryKeys.org.serviceAccountTokenActivity("org-1", "sa-1", "t-1")
    );
  });
});

describe("orgInventoryEndpoints", () => {
  it("reads a token's activity inside the org, and offers nothing to extend", async () => {
    const endpoints = orgInventoryEndpoints("org-1");
    await endpoints.activity("t-9", 100);
    expect(service.inventoryActivity).toHaveBeenCalledWith("org-1", "t-9", 100);
    expect(endpoints.keys.activity("t-9")).toEqual(
      queryKeys.org.tokenInventoryActivity("org-1", "t-9")
    );
    expect("extend" in endpoints).toBe(false);
  });
});
