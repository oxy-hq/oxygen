// @vitest-environment jsdom
// jsdom because a token's state is judged by `ApiKeyService`, the same call the Expiry cell makes,
// and that module loads the API client.

import { describe, expect, it } from "vitest";
import { summarize } from "./summary";
import { ADA, expired, inHours, JO, LIN, partnerToken, revoked, token } from "./testTokens";

const line = (tokens: Parameters<typeof summarize>[0]) => {
  const { lead, rest } = summarize(tokens);
  return `${lead}${rest}`;
};

const scoped = (id: string, owner = ADA) => token({ id, owner, all_access: false });

describe("summarize", () => {
  it("counts the live tokens, the people holding them and the all-access ones", () => {
    const tokens = [
      token({ id: "a", owner: ADA }),
      token({ id: "b", owner: LIN }),
      token({ id: "c", owner: LIN }),
      partnerToken({ id: "d", owner: JO })
    ];
    expect(summarize(tokens)).toEqual({
      lead: "4 tokens are live",
      rest: ", held by 3 people. 3 of them are all-access."
    });
  });

  it("speaks of one token and one person in the singular", () => {
    expect(line([token()])).toBe("1 token is live, held by one person. It is all-access.");
    expect(line([scoped("a")])).toBe("1 token is live, held by one person. It is not all-access.");
  });

  it("says both, all or none where a count would be clumsier", () => {
    expect(line([token({ id: "a" }), token({ id: "b" })])).toBe(
      "2 tokens are live, held by one person. Both are all-access."
    );
    expect(line([token({ id: "a" }), token({ id: "b" }), token({ id: "c" })])).toBe(
      "3 tokens are live, held by one person. All of them are all-access."
    );
    expect(line([scoped("a"), scoped("b", LIN)])).toBe(
      "2 tokens are live, held by 2 people. None of them is all-access."
    );
  });

  it("says one of several is all-access in the singular", () => {
    expect(line([token({ id: "a" }), scoped("b"), scoped("c")])).toBe(
      "3 tokens are live, held by one person. 1 of them is all-access."
    );
  });

  it("says how many more have ended", () => {
    expect(line([token(), revoked()])).toBe(
      "1 token is live, held by one person. It is all-access. 1 more has expired or been revoked."
    );
    expect(line([token(), revoked(), expired()])).toBe(
      "1 token is live, held by one person. It is all-access. 2 more have expired or been revoked."
    );
  });

  it("says nothing works when every listed token has ended", () => {
    expect(summarize([revoked(), expired()])).toEqual({
      lead: "No token with staff or partner standing works right now.",
      rest: " The 2 below have expired or been revoked."
    });
    expect(line([revoked()])).toBe(
      "No token with staff or partner standing works right now. The one below has expired or been revoked."
    );
  });

  it("counts neither an ended token's reach nor its owner", () => {
    // The revoked one was all-access and Lin's: neither is true of anything live.
    const tokens = [scoped("a", ADA), revoked({ owner: LIN })];
    expect(line(tokens)).toBe(
      "1 token is live, held by one person. It is not all-access. 1 more has expired or been revoked."
    );
  });

  it("does not count a token that lapsed since the fetch as live", () => {
    const lapsed = token({ id: "lapsed", status: "active", expires_at: inHours(-0.1) });
    expect(summarize([lapsed]).lead).toBe(
      "No token with staff or partner standing works right now."
    );
  });
});
