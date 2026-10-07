import { describe, expect, it } from "vitest";
import {
  classifyTokenLoginFailure,
  describeTokenLoginFailure,
  NEW_LINK_COMMAND
} from "./describeTokenLoginFailure";

// The redeem endpoint refuses every bad ticket the same way, so the page has
// exactly two things to say — and saying the wrong one sends whoever opened the
// link off to mint another when the server was simply down.
describe("classifyTokenLoginFailure", () => {
  it("puts the server's one refusal on the link", () => {
    expect(classifyTokenLoginFailure(400)).toBe("link");
  });

  it("puts everything else on the server", () => {
    // No response at all: the request never arrived.
    expect(classifyTokenLoginFailure(undefined)).toBe("server");
    for (const status of [401, 403, 404, 429, 500, 502, 503]) {
      expect(classifyTokenLoginFailure(status)).toBe("server");
    }
  });
});

describe("describeTokenLoginFailure", () => {
  it("states the rule a dead link broke, and leaves the fix to the command", () => {
    const { title, description } = describeTokenLoginFailure("link");
    expect(title).toBe("Link didn't work");
    expect(description).toBe("Links work once and expire in 5 minutes. Get a new one:");
    // The command is printed as code by the page, never quoted inside a sentence.
    expect(description).not.toContain("`");
    expect(NEW_LINK_COMMAND).toBe("oxyc login-link");
  });

  it("does not blame the link when the server could not be reached", () => {
    const { title, description } = describeTokenLoginFailure("server");
    expect(title).toBe("Couldn't reach the server");
    expect(description).toContain("may still be good");
    expect(description).not.toContain("once");
    expect(description).not.toContain("expire");
  });
});
