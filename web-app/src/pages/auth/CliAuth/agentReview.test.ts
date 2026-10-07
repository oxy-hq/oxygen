import { describe, expect, it } from "vitest";
import type { TokenOptions } from "@/types/apiToken";
import { agentMint, reviewAgentAsk } from "./agentReview";
import type { AgentAsk } from "./cliAuthRequest";

type Held = Pick<TokenOptions, "agent" | "can_platform" | "can_partner">;

const LIMITS = { default_hours: 8, max_hours: 168 };
const holds = (can_platform: boolean, can_partner: boolean): Held => ({
  agent: LIMITS,
  can_platform,
  can_partner
});
const MEMBER = holds(false, false);
const STAFF = holds(true, false);

const ask = (over: Partial<AgentAsk> = {}): AgentAsk => ({
  hours: "8",
  name: "triage run",
  standing: false,
  ...over
});

const review = (asked: AgentAsk, held: Held = MEMBER) => reviewAgentAsk(asked, "luong-mbp", held);

describe("reviewAgentAsk", () => {
  it("reads a request that can be approved as it stands", () => {
    expect(review(ask())).toEqual({
      hours: 8,
      name: "triage run",
      standing: { asked: false, held: [] },
      problems: { hours: null, name: null },
      unsupported: false,
      approvable: true
    });
  });

  it("gives the server's default lifetime and name to a request that sent neither", () => {
    const read = review(ask({ hours: null, name: null }));
    expect(read.hours).toBe(8);
    expect(read.name).toBe("agent on luong-mbp");
    expect(read.approvable).toBe(true);
    // A name sent empty is no name either.
    expect(review(ask({ name: "   " })).name).toBe("agent on luong-mbp");
  });

  it("refuses a lifetime out of range, and one that is no number, where it stands", () => {
    const cases: [string, string][] = [
      ["0", "An agent token lasts at least 1 hour."],
      ["169", "An agent token lasts at most 168 hours (7 days)."],
      ["8.5", "Not a whole number of hours."],
      ["-4", "Not a whole number of hours."],
      ["soon", "Not a whole number of hours."],
      ["", "Not a whole number of hours."]
    ];
    for (const [hours, why] of cases) {
      const read = review(ask({ hours }));
      expect(read.problems.hours).toBe(why);
      expect(read.approvable).toBe(false);
      expect(agentMint(read, ask({ hours }), true)).toBeUndefined();
    }
    expect(review(ask({ hours: "1" })).approvable).toBe(true);
    expect(review(ask({ hours: "168" })).approvable).toBe(true);
  });

  it("follows the server's own limits, not a number written here", () => {
    const short: Held = { ...MEMBER, agent: { default_hours: 2, max_hours: 24 } };
    expect(review(ask({ hours: null }), short).hours).toBe(2);
    expect(review(ask({ hours: "48" }), short).problems.hours).toBe(
      "An agent token lasts at most 24 hours."
    );
  });

  it("refuses a name longer than a token's may be, and never the server's own", () => {
    const long = review(ask({ name: "x".repeat(101) }));
    expect(long.problems.name).toBe("The token's name is longer than 100 characters.");
    expect(long.approvable).toBe(false);
    expect(review(ask({ name: "x".repeat(100) })).approvable).toBe(true);
    // The default is the server's to give: a long hostname is not the request's fault.
    const host = "h".repeat(200);
    expect(reviewAgentAsk(ask({ name: null }), host, MEMBER).problems.name).toBeNull();
  });

  it("says what the person holds, in the order it is named", () => {
    expect(review(ask({ standing: true }), holds(true, true)).standing).toEqual({
      asked: true,
      held: ["platform", "partner"]
    });
    expect(review(ask({ standing: true }), holds(false, true)).standing.held).toEqual(["partner"]);
    // Asked for by someone who holds none: no problem, and still approvable.
    const none = review(ask({ standing: true }));
    expect(none.standing).toEqual({ asked: true, held: [] });
    expect(none.approvable).toBe(true);
  });

  it("can't be approved on a server that names no limits for an agent token", () => {
    const old: Held = { can_platform: true, can_partner: false };
    const read = review(ask({ hours: null }), old);
    expect(read.unsupported).toBe(true);
    expect(read.approvable).toBe(false);
    expect(read.hours).toBeNull();
    // Nothing about the request is wrong, so nothing on it is marked.
    expect(read.problems).toEqual({ hours: null, name: null });
    expect(agentMint(read, ask({ hours: null }), true)).toBeUndefined();
    expect(review(ask({ hours: "8" }), old).approvable).toBe(false);
  });
});

describe("agentMint", () => {
  it("is the authorize body's mint: the kind, the hours, and a name only when one was sent", () => {
    expect(agentMint(review(ask()), ask(), true)).toEqual({
      kind: "agent",
      standing: false,
      expires_in_hours: 8,
      name: "triage run"
    });
    const unnamed = ask({ hours: null, name: null });
    expect(agentMint(review(unnamed), unnamed, true)).toEqual({
      kind: "agent",
      standing: false,
      expires_in_hours: 8
    });
  });

  it("carries standing only when it was asked for, is held, and was left ticked", () => {
    const asked = ask({ standing: true });
    expect(agentMint(review(asked, STAFF), asked, true)?.standing).toBe(true);
    // The approver cleared the box.
    expect(agentMint(review(asked, STAFF), asked, false)?.standing).toBe(false);
    // Asked for by someone who holds none: the token is asked to carry none.
    expect(agentMint(review(asked, MEMBER), asked, true)?.standing).toBe(false);
    // Held and not asked for: never added on the page's own account.
    expect(agentMint(review(ask(), STAFF), ask(), true)?.standing).toBe(false);
  });
});
