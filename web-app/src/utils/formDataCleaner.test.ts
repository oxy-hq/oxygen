import { describe, expect, it } from "vitest";
import { cleanObject } from "./formDataCleaner";

describe("cleanObject", () => {
  it("strips empty strings, null, NaN, empty lists and objects by default", () => {
    expect(
      cleanObject({
        a: "",
        b: null,
        c: Number.NaN,
        d: [],
        e: {},
        f: { g: "" },
        h: ["", "x", null],
        keep: 0,
        flag: false
      })
    ).toEqual({ h: ["x"], keep: 0, flag: false });
  });

  it("returns null when nothing is left", () => {
    expect(cleanObject({ a: "", b: [] })).toBeNull();
  });

  it("keeps a value the caller marks 'keep' even when it cleans to empty", () => {
    const preserve = (key: string) => (key === "tasks" || key === "value" ? "keep" : undefined);
    expect(cleanObject({ tasks: [], value: null, other: [] }, { preserve })).toEqual({
      tasks: [],
      value: null
    });
    // Still cleaned inside: only the emptiness of the key itself is kept.
    expect(cleanObject({ tasks: [{ name: "", type: "" }] }, { preserve })).toEqual({ tasks: [] });
  });

  it("keeps a value the caller marks 'verbatim' exactly as written, at any depth", () => {
    const preserve = (key: string) => (key === "values" ? "verbatim" : undefined);
    expect(cleanObject({ loop: { values: ["a", "", null], empty: "" } }, { preserve })).toEqual({
      loop: { values: ["a", "", null] }
    });
  });

  it("never keeps an undefined value, whatever the caller says", () => {
    expect(cleanObject({ a: undefined, b: 1 }, { preserve: () => "keep" })).toEqual({ b: 1 });
  });
});
