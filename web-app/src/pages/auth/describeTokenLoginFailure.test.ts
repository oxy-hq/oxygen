import { describe, expect, it } from "vitest";
import { classifyTokenLoginFailure, describeTokenLoginFailure } from "./describeTokenLoginFailure";

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
  it("lists every way a link can be spent, and names the fix", () => {
    const { title, description } = describeTokenLoginFailure("link");
    expect(title).toBe("Sign-in link didn't work");
    expect(description).toContain("invalid, has already been used, or has expired");
    expect(description).toContain("work once and last 5 minutes");
    expect(description).toContain("`oxyc login-link`");
  });

  it("does not blame the link when the server could not be reached", () => {
    const { title, description } = describeTokenLoginFailure("server");
    expect(title).toBe("Couldn't reach the server");
    expect(description).toContain("could not be reached");
    expect(description).not.toContain("already been used");
    expect(description).not.toContain("expired");
  });
});
