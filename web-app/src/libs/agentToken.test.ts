import { describe, expect, it } from "vitest";
import {
  AGENT_TOKEN_POWERS,
  agentLimits,
  heldStanding,
  isAgentToken,
  standingAdds,
  standingWords
} from "./agentToken";
import { hoursProblem } from "./sandboxAgentToken";

describe("isAgentToken", () => {
  it("is a personal token whose source is the agent approval, and nothing else", () => {
    expect(isAgentToken({ kind: "personal", source: "oxyc_agent" })).toBe(true);
    // An `oxyc login`, a token made in the web app, and one an older server sends no source for.
    expect(isAgentToken({ kind: "personal", source: "oxyc_login" })).toBe(false);
    expect(isAgentToken({ kind: "personal", source: "ui" })).toBe(false);
    expect(isAgentToken({ kind: "personal" })).toBe(false);
    // The source alone decides nothing: a sandbox agent token is its own kind.
    expect(isAgentToken({ kind: "sandbox_agent", source: "oxyc_agent" })).toBe(false);
    expect(isAgentToken({ source: "oxyc_agent" })).toBe(false);
    expect(isAgentToken({})).toBe(false);
  });
});

describe("agentLimits", () => {
  it("is the server's, with no fallback: a server that names none can't mint one", () => {
    expect(agentLimits({ agent: { default_hours: 8, max_hours: 168 } })).toEqual({
      default_hours: 8,
      max_hours: 168
    });
    expect(agentLimits({})).toBeUndefined();
    expect(agentLimits(undefined)).toBeUndefined();
  });
});

describe("standing", () => {
  it("lists what the person holds, staff first", () => {
    expect(heldStanding({ can_platform: true, can_partner: true })).toEqual([
      "platform",
      "partner"
    ]);
    expect(heldStanding({ can_platform: false, can_partner: true })).toEqual(["partner"]);
    expect(heldStanding({ can_platform: false, can_partner: false })).toEqual([]);
  });

  it("words it, and what including it adds, by what is held", () => {
    expect(standingWords(["platform"])).toBe("staff");
    expect(standingWords(["partner"])).toBe("partner");
    expect(standingWords(["platform", "partner"])).toBe("staff and partner");
    expect(standingAdds(["platform"])).toBe("Adds every organization on this deployment.");
    expect(standingAdds(["partner"])).toBe("Adds your client organizations.");
    expect(standingAdds(["platform", "partner"])).toBe(
      "Adds every organization on this deployment, and your client organizations."
    );
  });
});

describe("what an agent token can and cannot do", () => {
  it("is two short lists, each line an act", () => {
    expect(AGENT_TOKEN_POWERS).toEqual({
      can: ["Do what you can do through the API", "Open a browser session as itself"],
      cannot: ["Create, extend or revoke tokens", "Be extended", "Last past the time shown"]
    });
  });
});

describe("hoursProblem, for an agent token", () => {
  const LIMITS = { max_hours: 168 };

  it("names the kind of token the sentence is about", () => {
    expect(hoursProblem(0, LIMITS, "An agent token")).toBe("An agent token lasts at least 1 hour.");
    expect(hoursProblem(169, LIMITS, "An agent token")).toBe(
      "An agent token lasts at most 168 hours (7 days)."
    );
    expect(hoursProblem(8, LIMITS, "An agent token")).toBeNull();
  });

  it("still speaks of a sandbox agent token when not told otherwise", () => {
    expect(hoursProblem(0, LIMITS)).toBe("A sandbox agent token lasts at least 1 hour.");
  });
});
