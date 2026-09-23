import { describe, expect, it } from "vitest";
import type { OxyCryptoApi, OxyFunctionContext, OxyFunctionRow } from "./function-context";

// The fixtures below are typed against `OxyFunctionContext`, so the SDK
// typecheck ratchet in CI (`tsconfig.test.json`) is what fails when the declared
// shape drifts from the host's again — vitest transpiles without checking. The
// runtime assertions pin what a function written the documented way does with
// each value. The host itself is exercised by the platform canary
// (`customer-apps/examples/platform-canary`, steps `sql_read` and the webhook
// verification); nothing here reaches it.

/** What the host resolves for `ctx.query` (`host.rs` `query`): `{ rows, truncated }`. */
const query: OxyFunctionContext["query"] = async (sql) => ({
  rows: [{ sql, one: 1 }],
  truncated: false
});

/** `ctx.crypto` as `__buildCtx` binds it: three synchronous members, no promise. */
const hostCrypto: OxyCryptoApi = {
  hmac: ({ algorithm = "sha256", key, data, encoding = "hex" }) =>
    `${algorithm}:${encoding}:${key}:${data}`,
  verifyHmac: ({ signature, ...input }) =>
    signature != null && signature === hostCrypto.hmac(input),
  timingSafeEqual: (a, b) => !!a && !!b && a === b
};

describe("OxyFunctionContext.query", () => {
  it("resolves { rows, truncated } — the host's shape, which the docs destructure", async () => {
    const { rows, truncated } = await query("SELECT 1 AS one");
    const first: OxyFunctionRow | undefined = rows[0];
    expect(first?.one).toBe(1);
    expect(truncated).toBe(false);
  });

  it("is not a bare row array (the pre-2.14 type, which the host never sent)", () => {
    // @ts-expect-error — a bare array does not satisfy { rows, truncated }; if this
    // directive turns unused, the type has regressed to the shape that never ran.
    const result: Awaited<ReturnType<OxyFunctionContext["query"]>> = [{ one: 1 }];
    expect(Array.isArray(result)).toBe(true);
  });
});

describe("OxyFunctionContext.crypto", () => {
  const ctx: Pick<OxyFunctionContext, "crypto"> = { crypto: hostCrypto };

  it("is synchronous: hmac returns the digest string, not a promise", () => {
    const digest = ctx.crypto.hmac({ key: "secret", data: "body" });
    expect(typeof digest).toBe("string");
    expect(digest).toBe("sha256:hex:secret:body");
  });

  it("verifyHmac takes a header the caller may have omitted, and answers false for it", () => {
    const omitted: string | undefined = undefined;
    expect(ctx.crypto.verifyHmac({ key: "secret", data: "body", signature: omitted })).toBe(false);
    const signature = ctx.crypto.hmac({ key: "secret", data: "body" });
    expect(ctx.crypto.verifyHmac({ key: "secret", data: "body", signature })).toBe(true);
  });

  it("timingSafeEqual accepts an absent side and fails closed on it", () => {
    expect(ctx.crypto.timingSafeEqual(undefined, "s")).toBe(false);
    expect(ctx.crypto.timingSafeEqual("s", "s")).toBe(true);
  });
});
