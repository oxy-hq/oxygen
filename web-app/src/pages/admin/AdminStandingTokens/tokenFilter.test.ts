// @vitest-environment jsdom
// jsdom because a token's state is judged by `ApiKeyService`, the same call the Expiry cell makes,
// and that module loads the API client.

import { describe, expect, it } from "vitest";
import { expired, inHours, partnerToken, revoked, token } from "./testTokens";
import {
  filterCounts,
  filterTokens,
  NO_FILTER,
  noMatchMessage,
  type TokenFilter
} from "./tokenFilter";

const staff = token({ id: "staff" });
const partner = partnerToken({ id: "partner" });
const both = token({ id: "both", partner: true });
const staffRevoked = revoked({ id: "staff-revoked" });
const partnerExpired = expired({ id: "partner-expired", platform: false, partner: true });
const all = [staff, partner, both, staffRevoked, partnerExpired];

const ids = (filter: TokenFilter) => filterTokens(all, filter).map((each) => each.id);

describe("filterTokens", () => {
  it("shows everything the server sent when nothing is chosen, in its order", () => {
    expect(ids(NO_FILTER)).toEqual([
      "staff",
      "partner",
      "both",
      "staff-revoked",
      "partner-expired"
    ]);
  });

  it("keeps the tokens carrying staff standing", () => {
    expect(ids({ standing: "staff", hideEnded: false })).toEqual([
      "staff",
      "both",
      "staff-revoked"
    ]);
  });

  it("keeps the tokens carrying partner standing", () => {
    expect(ids({ standing: "partner", hideEnded: false })).toEqual([
      "partner",
      "both",
      "partner-expired"
    ]);
  });

  it("leaves out the revoked and the expired when asked", () => {
    expect(ids({ standing: "all", hideEnded: true })).toEqual(["staff", "partner", "both"]);
  });

  it("applies both at once", () => {
    expect(ids({ standing: "partner", hideEnded: true })).toEqual(["partner", "both"]);
  });

  it("counts a token that lapsed since the fetch as ended, as its row does", () => {
    const lapsed = token({ id: "lapsed", status: "active", expires_at: inHours(-0.1) });
    expect(filterTokens([staff, lapsed], { standing: "all", hideEnded: true })).toEqual([staff]);
  });

  it("keeps a token with no expiry among the live ones", () => {
    const lasting = token({ id: "lasting", expires_at: null });
    expect(filterTokens([lasting], { standing: "all", hideEnded: true })).toEqual([lasting]);
  });
});

describe("filterCounts", () => {
  it("counts what each standing choice would show, a token with both under each", () => {
    expect(filterCounts(all, NO_FILTER).standing).toEqual({ all: 5, staff: 3, partner: 3 });
  });

  it("counts only the live ones once the ended are hidden", () => {
    expect(filterCounts(all, { standing: "all", hideEnded: true }).standing).toEqual({
      all: 3,
      staff: 2,
      partner: 2
    });
  });

  it("counts the ended tokens of the chosen standing: what hiding takes away", () => {
    expect(filterCounts(all, NO_FILTER).ended).toBe(2);
    expect(filterCounts(all, { standing: "staff", hideEnded: false }).ended).toBe(1);
    expect(filterCounts(all, { standing: "partner", hideEnded: false }).ended).toBe(1);
  });

  it("keeps the ended count the same whether or not they are hidden", () => {
    expect(filterCounts(all, { standing: "staff", hideEnded: true }).ended).toBe(1);
  });

  it("is all zeroes for a list with nothing in it", () => {
    expect(filterCounts([], NO_FILTER)).toEqual({
      standing: { all: 0, staff: 0, partner: 0 },
      ended: 0
    });
  });
});

describe("noMatchMessage", () => {
  it("says no token carries the chosen standing", () => {
    expect(noMatchMessage({ standing: "partner", hideEnded: false })).toBe(
      "No token carries partner standing."
    );
    expect(noMatchMessage({ standing: "staff", hideEnded: false })).toBe(
      "No token carries staff standing."
    );
  });

  it("says none works right now when the ended ones are hidden: one may be listed", () => {
    expect(noMatchMessage({ standing: "partner", hideEnded: true })).toBe(
      "No token with partner standing works right now."
    );
  });

  it("says every token has ended when hiding them leaves nothing", () => {
    expect(noMatchMessage({ standing: "all", hideEnded: true })).toBe(
      "Every token listed has expired or been revoked."
    );
  });
});
