import { describe, expect, it } from "vitest";
import type { FunctionLogLine } from "@/types/apps";
import {
  formatLogTime,
  groupByInvocation,
  LOG_WINDOWS,
  parseRequestFilter,
  WIDEST_LOG_WINDOW,
  windowPhrase
} from "./functionLogs";

const ID = "0b7a1c2e-5d3f-4a6b-8c9d-0e1f2a3b4c5d";

function line(overrides: Partial<FunctionLogLine>): FunctionLogLine {
  return {
    timestamp: "2026-10-03T14:02:11.000000Z",
    build_id: "build-1",
    invocation_id: "inv-a",
    request_id: "req-a",
    function_name: "syncOrders",
    mode: "route",
    level: "info",
    seq: 0,
    message: "",
    trace_id: "",
    environment: "production",
    ...overrides
  };
}

describe("parseRequestFilter", () => {
  it("is no filter when the box is empty or blank", () => {
    expect(parseRequestFilter("")).toEqual({ kind: "none" });
    expect(parseRequestFilter("   ")).toEqual({ kind: "none" });
  });

  it("accepts a UUID however it was pasted", () => {
    expect(parseRequestFilter(`  ${ID}\n`)).toEqual({ kind: "id", requestId: ID });
    expect(parseRequestFilter(ID.toUpperCase())).toEqual({ kind: "id", requestId: ID });
    expect(parseRequestFilter(ID.replaceAll("-", ""))).toEqual({
      kind: "id",
      requestId: ID.replaceAll("-", "")
    });
  });

  it("holds back anything the route would answer with a 400", () => {
    // Half a pasted id must read as "keep typing", not as a failed fetch.
    for (const raw of [ID.slice(0, 18), `${ID}'`, `${ID}0`, "not-a-uuid", "' OR '1'='1"]) {
      expect(parseRequestFilter(raw), raw).toEqual({ kind: "invalid" });
    }
  });
});

describe("groupByInvocation", () => {
  it("keeps one invocation's lines together, in the order they were written", () => {
    // The API answers newest first, and two concurrent invocations interleave.
    const groups = groupByInvocation([
      line({ invocation_id: "inv-b", seq: 1, message: "b done" }),
      line({ invocation_id: "inv-a", seq: 2, message: "a done" }),
      line({ invocation_id: "inv-b", seq: 0, message: "b start" }),
      line({ invocation_id: "inv-a", seq: 1, message: "a working" }),
      line({ invocation_id: "inv-a", seq: 0, message: "a start" })
    ]);
    expect(groups.map((g) => g.key)).toEqual(["inv-b", "inv-a"]);
    expect(groups[0].lines.map((l) => l.message)).toEqual(["b start", "b done"]);
    expect(groups[1].lines.map((l) => l.message)).toEqual(["a start", "a working", "a done"]);
  });

  it("heads a group with its earliest line", () => {
    const [group] = groupByInvocation([
      line({ seq: 3, timestamp: "2026-10-03T14:02:15.000000Z" }),
      line({ seq: 0, timestamp: "2026-10-03T14:02:11.000000Z" })
    ]);
    expect(group.head.seq).toBe(0);
    expect(group.head.timestamp).toBe("2026-10-03T14:02:11.000000Z");
  });

  it("flags an invocation that printed an error anywhere in it", () => {
    const groups = groupByInvocation([
      line({ invocation_id: "inv-a", seq: 1, level: "info" }),
      line({ invocation_id: "inv-a", seq: 0, level: "error" }),
      line({ invocation_id: "inv-b", seq: 0, level: "warn" })
    ]);
    expect(groups.map((g) => [g.key, g.hasError])).toEqual([
      ["inv-a", true],
      ["inv-b", false]
    ]);
  });

  it("does not pool lines that have no invocation id", () => {
    // Nothing says two such lines belong to the same call, so each stands alone
    // rather than reading as one invocation's output.
    const groups = groupByInvocation([
      line({ invocation_id: "", message: "first" }),
      line({ invocation_id: "", message: "second" })
    ]);
    expect(groups).toHaveLength(2);
    expect(groups.map((g) => g.lines.map((l) => l.message))).toEqual([["first"], ["second"]]);
    expect(new Set(groups.map((g) => g.key)).size).toBe(2);
  });

  it("is empty for an empty page", () => {
    expect(groupByInvocation([])).toEqual([]);
  });
});

describe("formatLogTime", () => {
  it("is the wire's UTC time, with the date only when asked", () => {
    expect(formatLogTime("2026-10-03T14:02:11.123456Z", false)).toBe("14:02:11");
    expect(formatLogTime("2026-10-03T14:02:11.123456Z", true)).toBe("10-03 14:02:11");
  });

  it("returns what it was given when that is not a timestamp", () => {
    expect(formatLogTime("yesterday", false)).toBe("yesterday");
    expect(formatLogTime("", true)).toBe("");
  });
});

describe("log windows", () => {
  it("offers nothing wider than the window a request is searched in", () => {
    // A request filter always reads the widest window, so that has to be the
    // ceiling the picker offers too.
    expect(Math.max(...LOG_WINDOWS.map((w) => w.hours))).toBe(WIDEST_LOG_WINDOW);
  });

  it("names each window the way the empty state reads", () => {
    expect(windowPhrase(1)).toBe("the last hour");
    expect(windowPhrase(24)).toBe("the last 24 hours");
    expect(windowPhrase(168)).toBe("the last 7 days");
  });
});
