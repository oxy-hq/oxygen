import { afterEach, describe, expect, it, vi } from "vitest";
import { ServiceAccountService } from "@/services/api/orgApiAccess";
import type { Token } from "@/types/apiToken";
import type { ServiceAccount } from "@/types/orgApiAccess";
import {
  discardMinted,
  type MintedApiToken,
  mintApiToken,
  suggestedAccountName,
  tokenNameFor
} from "./mint";

vi.mock("@/services/api/orgApiAccess", () => ({
  ServiceAccountService: {
    create: vi.fn(),
    createToken: vi.fn(),
    revokeToken: vi.fn(),
    remove: vi.fn()
  }
}));

const create = vi.mocked(ServiceAccountService.create);
const createToken = vi.mocked(ServiceAccountService.createToken);
const revokeToken = vi.mocked(ServiceAccountService.revokeToken);
const remove = vi.mocked(ServiceAccountService.remove);

const account = (over: Partial<ServiceAccount> = {}): ServiceAccount => ({
  id: "sa-1",
  org_id: "org-1",
  name: "store-ops",
  description: null,
  org_role: "admin",
  created_by: null,
  created_at: "2026-11-18T00:00:00Z",
  disabled_at: null,
  token_count: 0,
  trust_policy_count: 0,
  ...over
});

const minted = (over: Partial<Token> = {}) => ({
  token: { id: "tok-1", expires_at: "2027-02-16T00:00:00Z", ...over } as Token,
  secret: "oxy_sat_secret"
});

afterEach(() => vi.resetAllMocks());

describe("mintApiToken", () => {
  it("creates the account, then a token on it, and hands back the secret it never showed", async () => {
    create.mockResolvedValue(account());
    createToken.mockResolvedValue(minted());

    const result = await mintApiToken({
      orgId: "org-1",
      account: { kind: "new", name: "store-ops", role: "admin" },
      tokenName: "Secret TABLETS_API_KEY of Store Ops",
      expiry: { expires_in_days: 90 }
    });

    expect(create).toHaveBeenCalledWith("org-1", { name: "store-ops", org_role: "admin" });
    // No grants: the account's own org-wide reach at its role.
    expect(createToken).toHaveBeenCalledWith("org-1", "sa-1", {
      name: "Secret TABLETS_API_KEY of Store Ops",
      expires_in_days: 90
    });
    expect(result).toEqual({
      value: "oxy_sat_secret",
      orgId: "org-1",
      tokenId: "tok-1",
      expiresAt: "2027-02-16T00:00:00Z",
      account: { id: "sa-1", name: "store-ops", role: "admin", created: true }
    });
  });

  it("reuses an existing account without creating one", async () => {
    createToken.mockResolvedValue(minted());
    const result = await mintApiToken({
      orgId: "org-1",
      account: { kind: "existing", account: account({ org_role: "member" }) },
      tokenName: "Secret REPORT_TOKEN",
      expiry: { expires_at: null }
    });
    expect(create).not.toHaveBeenCalled();
    expect(result.account).toEqual({
      id: "sa-1",
      name: "store-ops",
      role: "member",
      created: false
    });
  });

  it("deletes an account it just made when the token cannot be minted, and says why", async () => {
    create.mockResolvedValue(account());
    createToken.mockRejectedValue(new Error("lifetime_exceeds_policy"));
    remove.mockResolvedValue(undefined);

    await expect(
      mintApiToken({
        orgId: "org-1",
        account: { kind: "new", name: "store-ops", role: "admin" },
        tokenName: "t",
        expiry: { expires_in_days: 400 }
      })
    ).rejects.toThrow("lifetime_exceeds_policy");
    expect(remove).toHaveBeenCalledWith("org-1", "sa-1");
  });

  it("leaves an existing account alone when the token cannot be minted", async () => {
    createToken.mockRejectedValue(new Error("nope"));
    await expect(
      mintApiToken({
        orgId: "org-1",
        account: { kind: "existing", account: account() },
        tokenName: "t",
        expiry: { expires_in_days: 90 }
      })
    ).rejects.toThrow("nope");
    expect(remove).not.toHaveBeenCalled();
  });
});

describe("discardMinted", () => {
  const unsaved = (created: boolean): MintedApiToken => ({
    value: "oxy_sat_secret",
    orgId: "org-1",
    tokenId: "tok-1",
    expiresAt: null,
    account: { id: "sa-1", name: "store-ops", role: "admin", created }
  });

  it("revokes the token, and takes back an account made only for it", async () => {
    await discardMinted(unsaved(true));
    expect(revokeToken).toHaveBeenCalledWith("org-1", "sa-1", "tok-1");
    expect(remove).toHaveBeenCalledWith("org-1", "sa-1");
  });

  it("never deletes an account that existed before", async () => {
    await discardMinted(unsaved(false));
    expect(revokeToken).toHaveBeenCalledWith("org-1", "sa-1", "tok-1");
    expect(remove).not.toHaveBeenCalled();
  });
});

describe("names", () => {
  it("names the token after the secret it lives in, and the app when there is one", () => {
    expect(tokenNameFor("TABLETS_API_KEY", "Store Ops")).toBe(
      "Secret TABLETS_API_KEY of Store Ops"
    );
    expect(tokenNameFor("REPORT_TOKEN")).toBe("Secret REPORT_TOKEN");
    // The server stops a token name at 100 characters.
    expect(tokenNameFor("K".repeat(200))).toHaveLength(100);
  });

  it("suggests the app's slug for its account, else the secret's name as a slug", () => {
    expect(suggestedAccountName("TABLETS_API_KEY", "store-ops")).toBe("store-ops");
    expect(suggestedAccountName("OXY_REPORT_UPLOAD_TOKEN")).toBe("oxy-report-upload-token");
    // A slug starts with a letter, and is never empty.
    expect(suggestedAccountName("2FA_KEY")).toBe("fa-key");
    expect(suggestedAccountName("___")).toBe("app-token");
  });
});
