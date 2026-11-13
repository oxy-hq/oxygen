import { describe, expect, it } from "vitest";
import { lineDiff } from "./lineDiff";

describe("lineDiff", () => {
  it("keeps common lines and marks the changed ones", () => {
    expect(lineDiff("a\nb\nc\n", "a\nx\nc\n")).toEqual([
      { kind: "same", text: "a", oldNo: 1, newNo: 1 },
      { kind: "removed", text: "b", oldNo: 2 },
      { kind: "added", text: "x", newNo: 2 },
      { kind: "same", text: "c", oldNo: 3, newNo: 3 }
    ]);
  });

  it("shows a deleted file as every line removed", () => {
    expect(lineDiff("a\nb", "")).toEqual([
      { kind: "removed", text: "a", oldNo: 1 },
      { kind: "removed", text: "b", oldNo: 2 }
    ]);
  });

  it("shows a new file as every line added", () => {
    expect(lineDiff("", "a")).toEqual([{ kind: "added", text: "a", newNo: 1 }]);
  });

  it("numbers a snippet from where it starts in its file", () => {
    expect(lineDiff("x", "y", 40)).toEqual([
      { kind: "removed", text: "x", oldNo: 40 },
      { kind: "added", text: "y", newNo: 40 }
    ]);
  });
});
