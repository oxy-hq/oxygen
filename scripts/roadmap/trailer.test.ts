import assert from "node:assert/strict";
import { test } from "node:test";
import { parseTrailer } from "./trailer.ts";

test("Roadmap trailer: lines, none with a reason, several lines add up", () => {
  assert.deepEqual(parseTrailer("## Why\n\nRoadmap: open-apps, phone-apps-in-the-app-stores\n"), {
    kind: "lines",
    ids: ["open-apps", "phone-apps-in-the-app-stores"]
  });
  assert.deepEqual(parseTrailer("Roadmap: `Open-Apps`\nRoadmap-Item: b-line"), {
    kind: "lines",
    ids: ["open-apps", "b-line"]
  });
  assert.deepEqual(parseTrailer("Roadmap: none -- dependency bump only"), {
    kind: "none",
    reason: "dependency bump only"
  });
  assert.equal(parseTrailer("roadmap: NONE: fixes a typo in a log line").kind, "none");
  assert.deepEqual(parseTrailer(null), { kind: "missing" });
  assert.deepEqual(parseTrailer("No trailer here.\nroadmaps: x"), { kind: "missing" });
});

test("Roadmap trailer: what is refused", () => {
  assert.equal(parseTrailer("Roadmap: none").kind, "invalid", "a bare none has no reason");
  assert.equal(parseTrailer("Roadmap: none -- short").kind, "invalid");
  assert.equal(parseTrailer("Roadmap: none -- x\nRoadmap: open-apps").kind, "invalid");
  assert.equal(parseTrailer("Roadmap: Not A Line!").kind, "invalid");
  assert.equal(parseTrailer("Roadmap: \n").kind, "invalid", "the template's line, left empty");
  assert.deepEqual(
    parseTrailer("Roadmap: \n\n## How it was verified\n"),
    { kind: "invalid", problem: "is empty: `Roadmap: <line-id>` or `Roadmap: none -- <why>`" },
    "an empty value never reads the next line"
  );
});
