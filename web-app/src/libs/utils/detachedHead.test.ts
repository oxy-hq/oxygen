import { describe, expect, it } from "vitest";
import { detachedHeadFor, detachedHeadLabel, readDetachedHeadBody } from "./detachedHead";

const detached = { active_branch: { name: "HEAD@abc1234" }, detached_head: "abc1234" };

describe("detachedHeadFor", () => {
  it("is the sha when the branch in hand is the detached working copy", () => {
    expect(detachedHeadFor(detached, "HEAD@abc1234")).toBe("abc1234");
  });

  it("is null on a real branch of a detached workspace", () => {
    // The IDE switched to `feature`, which lives in its own worktree.
    expect(detachedHeadFor(detached, "feature")).toBeNull();
  });

  it("never infers the state from the label alone", () => {
    // No `detached_head` from the server → not detached, whatever the name looks like.
    expect(detachedHeadFor({ active_branch: { name: "HEAD@abc1234" } }, "HEAD@abc1234")).toBeNull();
    expect(
      detachedHeadFor({ active_branch: { name: "main" }, detached_head: null }, "main")
    ).toBeNull();
  });

  it("is null without a workspace or a branch", () => {
    expect(detachedHeadFor(null, "HEAD@abc1234")).toBeNull();
    expect(detachedHeadFor(undefined, "main")).toBeNull();
    expect(detachedHeadFor(detached, "")).toBeNull();
  });
});

describe("detachedHeadLabel", () => {
  it("reads as a state, not a branch name", () => {
    expect(detachedHeadLabel("abc1234")).toBe("Detached at abc1234");
  });
});

describe("readDetachedHeadBody", () => {
  const message =
    "This workspace is on a detached HEAD at abc1234; switch to or create a branch first.";

  it("returns the server's message for a 409 detached_head", () => {
    expect(readDetachedHeadBody(409, { code: "detached_head", message, sha: "abc1234" })).toBe(
      message
    );
  });

  it("falls back when the server sent no message", () => {
    expect(readDetachedHeadBody(409, { code: "detached_head" })).toMatch(/detached HEAD/);
  });

  it("ignores every other response", () => {
    expect(readDetachedHeadBody(409, { code: "preview_read_only", message })).toBeNull();
    expect(readDetachedHeadBody(400, { code: "detached_head", message })).toBeNull();
    expect(readDetachedHeadBody(409, "plain text")).toBeNull();
    expect(readDetachedHeadBody(409, null)).toBeNull();
    expect(readDetachedHeadBody(undefined, undefined)).toBeNull();
  });
});
