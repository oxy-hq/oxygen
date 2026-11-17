import { describe, expect, it } from "vitest";
import type { PublishToken } from "@/types/publishTokens";
import { revoked, token } from "./testTokens";
import { parseKind, summarizeRows, toRows } from "./tokenRows";

const publish = (over: Partial<PublishToken> = {}): PublishToken => ({
  id: "p1",
  name: "ci-publish",
  token_prefix: "oxyp_abcd",
  created_by: "ada@oxy.tech",
  created_at: "2026-11-01T00:00:00Z",
  last_used_at: null,
  revoked: false,
  revoked_at: null,
  ...over
});

describe("toRows", () => {
  it("puts every kind on one list, live first, newest first", () => {
    const rows = toRows({
      publish: [publish({ id: "old", created_at: "2026-01-01T00:00:00Z" })],
      sandbox: [token({ id: "s", created_at: "2026-11-10T00:00:00Z" })],
      standing: [revoked({ id: "r", created_at: "2026-11-15T00:00:00Z" })]
    });
    expect(rows.map((r) => `${r.kind}:${r.id}`)).toEqual([
      "sandbox:s",
      "publish:old",
      "standing:r"
    ]);
  });

  it("reads a revoked publish token as ended", () => {
    const [row] = toRows({ publish: [publish({ revoked: true })] });
    expect(row.live).toBe(false);
  });

  it("lists nothing for a kind it was not given", () => {
    expect(toRows({ publish: [publish()] }).every((r) => r.kind === "publish")).toBe(true);
  });
});

describe("parseKind", () => {
  it("keeps a kind the viewer can list", () => {
    expect(parseKind("sandbox", ["publish", "sandbox"])).toBe("sandbox");
  });
  it("falls back to every kind for one they cannot, or for junk", () => {
    expect(parseKind("standing", ["publish"])).toBe("all");
    expect(parseKind("nope", ["publish"])).toBe("all");
    expect(parseKind(null, ["publish"])).toBe("all");
  });
});

describe("summarizeRows", () => {
  it("breaks the live count down by kind when there is more than one", () => {
    const s = summarizeRows(
      toRows({ publish: [publish()], sandbox: [token({ id: "a" }), token({ id: "b" })] })
    );
    expect(s.lead).toBe("3 tokens are live");
    expect(s.rest).toBe(": 1 app publish, 2 sandbox agent.");
  });

  it("does not break down a single kind, and counts the ended ones after", () => {
    const s = summarizeRows(toRows({ standing: [token(), revoked({ id: "x" })] }));
    expect(s.lead).toBe("1 token is live");
    expect(s.rest).toBe(". 1 more has expired or been revoked.");
  });

  it("says so when nothing works", () => {
    expect(summarizeRows(toRows({ publish: [publish({ revoked: true })] })).lead).toBe(
      "No token works right now."
    );
  });
});
