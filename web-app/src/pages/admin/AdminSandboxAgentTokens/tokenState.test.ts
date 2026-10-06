// @vitest-environment jsdom
// jsdom because expiry is judged by `ApiKeyService`, the same call the Expiry cell makes, and
// that module loads the API client.

import { describe, expect, it } from "vitest";
import type { Token } from "@/types/apiToken";
import { endedAt, summarize, tokenState } from "./tokenState";

const HOUR = 60 * 60 * 1000;
const inHours = (hours: number) => new Date(Date.now() + hours * HOUR).toISOString();

type Row = Pick<Token, "status" | "expires_at" | "revoked_at" | "owner">;

const row = (over: Partial<Row> & { minter?: string } = {}): Row => ({
  status: "active",
  expires_at: inHours(8),
  revoked_at: null,
  owner: { type: "user", id: over.minter ?? "u1", label: `${over.minter ?? "u1"}@oxy.tech` },
  ...over
});

describe("tokenState", () => {
  it("takes the server's word for a token that works, is expired or is revoked", () => {
    expect(tokenState(row())).toBe("active");
    expect(tokenState(row({ status: "expired", expires_at: inHours(-1) }))).toBe("expired");
    expect(tokenState(row({ status: "revoked", revoked_at: inHours(-1) }))).toBe("revoked");
  });

  it("reads a token that lapsed since the list was fetched as expired", () => {
    expect(tokenState(row({ status: "active", expires_at: inHours(-0.1) }))).toBe("expired");
  });

  it("keeps a revoked token revoked however its expiry reads", () => {
    expect(tokenState(row({ status: "revoked", expires_at: inHours(-5) }))).toBe("revoked");
    expect(tokenState(row({ status: "revoked", expires_at: inHours(5) }))).toBe("revoked");
  });
});

describe("endedAt", () => {
  it("is the revocation for a revoked token and the expiry for an expired one", () => {
    const revokedAt = inHours(-3);
    const expiredAt = inHours(-2);
    expect(endedAt(row({ status: "revoked", revoked_at: revokedAt }))).toBe(revokedAt);
    expect(endedAt(row({ status: "expired", expires_at: expiredAt }))).toBe(expiredAt);
  });

  it("is nothing while the token works", () => {
    expect(endedAt(row())).toBeNull();
  });
});

describe("summarize", () => {
  const ended = (count: number) =>
    Array.from({ length: count }, () => row({ status: "expired", expires_at: inHours(-1) }));

  it("counts the live tokens and the people who minted them", () => {
    const line = summarize([
      row({ minter: "ada" }),
      row({ minter: "ada" }),
      row({ minter: "lin" })
    ]);
    expect(line.lead).toBe("3 tokens are live");
    expect(line.rest).toBe(", minted by 2 people.");
  });

  it("speaks of one token and one person in the singular", () => {
    const line = summarize([row(), ...ended(1)]);
    expect(line.lead + line.rest).toBe(
      "1 token is live, minted by one person. 1 more has expired or been revoked."
    );
  });

  it("says how many more have ended", () => {
    expect(summarize([row(), ...ended(4)]).rest).toBe(
      ", minted by one person. 4 more have expired or been revoked."
    );
  });

  it("says nothing is live when every listed token has ended", () => {
    expect(summarize(ended(3))).toEqual({
      lead: "No agent holds a token right now.",
      rest: " The 3 below have expired or been revoked."
    });
    expect(summarize(ended(1)).rest).toBe(" The one below has expired or been revoked.");
  });

  it("does not count a token that lapsed since the fetch as live", () => {
    const line = summarize([row({ status: "active", expires_at: inHours(-0.1) })]);
    expect(line.lead).toBe("No agent holds a token right now.");
  });
});
