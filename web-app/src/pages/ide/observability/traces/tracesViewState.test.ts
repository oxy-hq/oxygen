import { describe, expect, it } from "vitest";
import { readTracesViewState, writeTracesViewState } from "./tracesViewState";

const params = (s: string) => new URLSearchParams(s);

describe("readTracesViewState", () => {
  it("is the default view when the URL says nothing", () => {
    expect(readTracesViewState(params(""))).toEqual({
      timeRange: { kind: "preset", value: "30d" },
      search: "",
      status: "all",
      page: 1
    });
  });

  it("reads back every field a link can carry", () => {
    expect(readTracesViewState(params("range=7d&q=revenue&status=error&page=3"))).toEqual({
      timeRange: { kind: "preset", value: "7d" },
      search: "revenue",
      status: "error",
      page: 3
    });
  });

  it("falls back on anything the page cannot render", () => {
    // A URL is user input. `?status=failed` should show every trace, not take
    // the page down, and a page of `0`, `-2` or `1e3` is not a page.
    const v = readTracesViewState(params("range=2w&status=failed&page=0"));
    expect(v.timeRange).toEqual({ kind: "preset", value: "30d" });
    expect(v.status).toBe("all");
    expect(v.page).toBe(1);
    expect(readTracesViewState(params("page=-2")).page).toBe(1);
    expect(readTracesViewState(params("page=1e3")).page).toBe(1);
    expect(readTracesViewState(params("page=abc")).page).toBe(1);
  });

  it("prefers an absolute range over a preset, as the API does", () => {
    const v = readTracesViewState(params("range=7d&from=1700000000&to=1700003600"));
    expect(v.timeRange).toEqual({ kind: "custom", from: 1700000000, to: 1700003600 });
  });

  it("does not treat half a range, or a backwards one, as a range", () => {
    // Querying from a lone `from` to "now" would be a window nobody chose.
    const preset = { kind: "preset", value: "7d" };
    expect(readTracesViewState(params("range=7d&from=1700000000")).timeRange).toEqual(preset);
    expect(readTracesViewState(params("range=7d&to=1700000000")).timeRange).toEqual(preset);
    expect(readTracesViewState(params("range=7d&from=1700003600&to=1700000000")).timeRange).toEqual(
      preset
    );
    expect(readTracesViewState(params("range=7d&from=1700000000&to=1700000000")).timeRange).toEqual(
      preset
    );
  });

  it("trims the search, so a padded link searches what the box will show", () => {
    expect(readTracesViewState(params("q=%20revenue%20")).search).toBe("revenue");
  });
});

describe("writeTracesViewState", () => {
  it("drops params that are back at their default", () => {
    const next = writeTracesViewState(params("range=7d&q=revenue&status=error&page=3"), {
      timeRange: { kind: "preset", value: "30d" },
      search: "",
      status: "all",
      page: 1
    });
    expect(next.toString()).toBe("");
  });

  it("writes only what the patch names", () => {
    const next = writeTracesViewState(params("range=7d&status=error"), { page: 2 });
    expect(next.get("range")).toBe("7d");
    expect(next.get("status")).toBe("error");
    expect(next.get("page")).toBe("2");
  });

  it("leaves params it does not own alone", () => {
    const next = writeTracesViewState(params("branch=feature-x"), { status: "ok" });
    expect(next.get("branch")).toBe("feature-x");
    expect(next.get("status")).toBe("ok");
  });

  it("keeps the two range forms mutually exclusive", () => {
    // A preset left behind a custom range would be dead weight in the link and,
    // worse, would come back to life the moment the custom range was cleared.
    const custom = writeTracesViewState(params("range=7d"), {
      timeRange: { kind: "custom", from: 1700000000, to: 1700003600 }
    });
    expect(custom.get("range")).toBeNull();
    expect(custom.get("from")).toBe("1700000000");
    expect(custom.get("to")).toBe("1700003600");

    const preset = writeTracesViewState(custom, { timeRange: { kind: "preset", value: "24h" } });
    expect(preset.get("from")).toBeNull();
    expect(preset.get("to")).toBeNull();
    expect(preset.get("range")).toBe("24h");
  });

  it("round-trips: what is written is what is read", () => {
    const view = {
      timeRange: { kind: "custom" as const, from: 1700000000, to: 1700003600 },
      search: "it's 50% slower",
      status: "error" as const,
      page: 4
    };
    expect(readTracesViewState(writeTracesViewState(params(""), view))).toEqual(view);
  });
});
