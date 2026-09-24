// `runWithoutIsolateGlobals` makes the isolate's absent globals absent for one
// call and puts them back — and `btoa` / `atob` are the runtime's own inside.

import { describe, expect, it } from "vitest";
import { ABSENT_GLOBALS, REFUSALS } from "./host-contract";
import { isolateAtob, isolateBtoa, runWithoutIsolateGlobals as run } from "./run";

describe("run", () => {
  it("makes every ABSENT_GLOBALS name a ReferenceError inside, and restores it after", async () => {
    const before = ABSENT_GLOBALS.map(
      (g) => typeof (globalThis as Record<string, unknown>)[g.name]
    );
    expect(before.every((t) => t !== "undefined")).toBe(true);
    const seen = await run(() =>
      ABSENT_GLOBALS.map((g) => {
        try {
          new Function(`return ${g.name}`)();
          return "present";
        } catch (err) {
          return err instanceof ReferenceError ? "ReferenceError" : "other";
        }
      })
    );
    expect(seen).toEqual(ABSENT_GLOBALS.map(() => "ReferenceError"));
    expect(typeof Buffer).toBe("function");
    expect(typeof TextEncoder).toBe("function");
    expect(typeof process).toBe("object");
    expect(typeof crypto).toBe("object");
  });

  it("restores the globals when the call throws, and the value when it resolves", async () => {
    await expect(
      run(() => {
        throw new Error("inside");
      })
    ).rejects.toThrow("inside");
    expect(typeof TextDecoder).toBe("function");
    expect(await run(async () => 42)).toBe(42);
  });

  it("uses the isolate's btoa and atob inside, and Node's outside", async () => {
    const inside = await run(() => {
      let refused = "";
      try {
        btoa(new Uint8Array([1, 2, 3]) as unknown as string);
      } catch (err) {
        refused = (err as Error).message;
      }
      return { refused, roundTrip: atob(btoa("hello")) };
    });
    expect(inside.refused).toBe(REFUSALS.btoaBytes.refusal);
    expect(inside.roundTrip).toBe("hello");
    expect(btoa("hello")).toBe("aGVsbG8=");
  });

  it("refuses to nest", async () => {
    await expect(run(() => run(() => 1))).rejects.toThrow(/must not overlap/);
    expect(await run(() => 2)).toBe(2);
  });
});

describe("the isolate's base64", () => {
  it("encodes Latin1 strings and refuses bytes and non-Latin1 text", () => {
    expect(isolateBtoa("hello")).toBe("aGVsbG8=");
    expect(isolateBtoa("")).toBe("");
    expect(() => isolateBtoa(new Uint8Array(2))).toThrow(REFUSALS.btoaBytes.refusal);
    expect(() => isolateBtoa("𝔘")).toThrow(/outside the Latin1 range/);
  });

  it("decodes forgivingly, and refuses bad padding, length and characters as the runtime does", () => {
    expect(isolateAtob("aGVsbG8=")).toBe("hello");
    expect(isolateAtob(" aGVs\nbG8= ")).toBe("hello");
    expect(isolateAtob("aGVsbG8")).toBe("hello");
    expect(() => isolateAtob("aG=VsbG8=")).toThrow(REFUSALS.atobPadding.refusal);
    expect(() => isolateAtob("a")).toThrow(REFUSALS.atobLength.refusal);
    expect(() => isolateAtob("aGVs*G8=")).toThrow(/invalid base64 character/);
  });
});
