import { describe, expect, it } from "vitest";
import { markPosted, parsePointer, renderComment } from "./record";
import { announcedPrs, threadComment } from "./release";
import { summaryTable } from "./release-run";
import type { ShowcaseRecord } from "./types";

describe("announcedPrs", () => {
  it("mirrors the announcement: feat then fix, oldest first, five of each", () => {
    const subjects = [
      "fix: one (#1)",
      "chore: bump (#2)",
      ...Array.from({ length: 6 }, (_, i) => `feat(web): new ${i} (#${10 + i})`),
      "fix: no number",
      "docs: nope (#3)"
    ];
    const got = announcedPrs(subjects);
    expect(got.map((p) => p.pr)).toEqual([10, 11, 12, 13, 14, 1]);
    expect(got[0]).toEqual({ pr: 10, type: "feat", text: "New 0" });
  });
});

describe("threadComment", () => {
  it("escapes Slack mrkdwn and links the PR last", () => {
    expect(threadComment("o/r", { pr: 5, type: "feat", text: "A < B" }, "Tables & charts")).toBe(
      "*Tables &amp; charts*\nA &lt; B · <https://github.com/o/r/pull/5|#5>"
    );
  });
});

const record: ShowcaseRecord = {
  version: 1,
  pr: 5,
  title: "feat: x",
  head_sha: "abc",
  outcome: "captured",
  reason: "recorded and replayed",
  plan: {
    verdict: "show",
    reason: "r",
    headline: "Charts export to PNG",
    start_path: "/local",
    steps: ["Click Export"],
    expect: "A PNG download notice",
    media: "screenshot"
  },
  screenshot: "media/screenshot.png",
  cost_usd: 0.1,
  captured_at: "2026-09-30T00:00:00Z"
};
const pointer = {
  run_id: "42",
  artifact: "showcase-pr-5",
  head_sha: "abc",
  outcome: "captured" as const
};

describe("the PR comment", () => {
  it("round-trips its pointer", () => {
    const body = renderComment(record, pointer, "https://gh/run/42");
    expect(body.startsWith("<!-- oxy-showcase -->")).toBe(true);
    expect(parsePointer(body)).toEqual(pointer);
    expect(body).toContain("1. Click Export");
  });
  it("records each thread it was posted in, once", () => {
    const body = renderComment(record, pointer, "u");
    const once = markPosted(body, "1700.1") ?? "";
    const twice = markPosted(once, "1700.1") ?? "";
    expect(parsePointer(twice)?.posted_in).toEqual(["1700.1"]);
    expect(markPosted("no pointer here", "1")).toBeUndefined();
  });
});

describe("summaryTable", () => {
  it("keeps a reason with pipes inside its cell", () => {
    const table = summaryTable([
      {
        item: { pr: 5, type: "fix", text: "t" },
        outcome: "rejected",
        reason: "a | b\nc",
        cost_usd: 0.01
      }
    ]);
    expect(table.split("\n")[2]).toBe("| #5 | rejected | — | a \\| b c | $0.010 |");
  });
});
