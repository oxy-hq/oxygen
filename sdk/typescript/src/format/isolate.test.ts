// The format entry runs where an Oxy Function runs: bundled the way `oxyc publish` bundles a
// function (esbuild, platform neutral) and executed in a bare V8 context with no Node or DOM
// globals, as the email entry's test does. The isolate is bare `deno_core`: no `Buffer`,
// `process`, `TextEncoder`, `console` or `document`, but the engine's own `Intl`.

import { fileURLToPath } from "node:url";
import vm from "node:vm";
import { build } from "esbuild";
import { describe, expect, it } from "vitest";
import * as format from "./index";

const EXPORTS = [
  "MISSING",
  "countPhrase",
  "csvCell",
  "defaultTimeZone",
  "formatAge",
  "formatAsOf",
  "formatDataThrough",
  "formatDate",
  "formatDateRange",
  "formatDateTime",
  "formatMoney",
  "formatMonthDay",
  "formatMonthYear",
  "formatNumber",
  "formatPercent",
  "formatRelative",
  "formatShortMonth",
  "formatTime",
  "formatWeekday",
  "joinNames",
  "plural",
  "sentenceCase",
  "toCsv"
];

type Format = typeof format;

/** One call per export, with inputs that reach its Intl and non-Intl paths. */
function exercise(f: Format): unknown[] {
  const LA = "America/Los_Angeles";
  const now = "2026-11-19T20:00:00Z";
  return [
    f.MISSING,
    f.formatMoney(-1234.5),
    f.formatMoney(3_400_000, { compact: true }),
    f.formatNumber(1284.4, { signed: true }),
    f.formatPercent(0.145, { fraction: true }),
    f.formatDate("2026-11-19T03:00:00Z", { timeZone: LA, now }),
    f.formatDateTime("2026-03-08T10:30:00Z", { timeZone: LA }),
    f.formatMonthYear("2026-07"),
    f.formatShortMonth("2026-07"),
    f.formatMonthDay("2026-08-12"),
    f.formatWeekday("2026-11-19", { long: true }),
    f.formatTime("2026-11-19T15:03:00Z"),
    f.formatDateRange("2026-09-28", "2026-10-04"),
    f.formatAsOf("2026-11-19T15:03:00Z", { now, timeZone: LA }),
    f.formatDataThrough("2026-11-18", { now }),
    f.formatRelative("2026-11-18T07:30:00Z", "2026-11-19T09:00:00Z", { timeZone: LA }),
    f.formatAge("2026-10-14T11:55:00Z", Date.parse("2026-10-14T12:00:00Z")),
    f.countPhrase(3, "guest"),
    f.plural(1284, "document"),
    f.joinNames(["a", "b", "c"]),
    f.sentenceCase("three guests"),
    f.toCsv(
      [{ n: "=1", v: 2 }],
      [
        { header: "N", value: "n" },
        { header: "V", value: (r) => r.v }
      ]
    ),
    f.csvCell('a "b"')
  ];
}

describe("the format entry", () => {
  it("exports exactly its documented API", () => {
    expect(Object.keys(format).sort()).toEqual([...EXPORTS].sort());
  });

  it("bundles for the Functions isolate and answers the same there, without Node or DOM globals", async () => {
    const bundled = await build({
      entryPoints: [fileURLToPath(new URL("./index.ts", import.meta.url))],
      bundle: true,
      format: "iife",
      globalName: "__format",
      platform: "neutral",
      write: false,
      logLevel: "silent"
    });
    const code = bundled.outputFiles[0].text;

    const context = vm.createContext({});
    vm.runInContext("delete globalThis.console", context);
    const hostOnly = vm.runInContext(
      `["Buffer", "process", "TextEncoder", "TextDecoder", "console", "document", "window",
        "navigator", "Blob", "setTimeout", "structuredClone"]
        .filter((name) => typeof globalThis[name] !== "undefined")`,
      context
    );
    expect(hostOnly).toEqual([]);

    vm.runInContext(code, context);
    const inIsolate = vm.runInContext(`(${exercise.toString()})(__format)`, context) as unknown[];
    expect(Array.from(inIsolate)).toEqual(exercise(format));
    expect(inIsolate).toContain("As of 7:03 AM");
    // No `document` in the isolate, so an instant with no zone is shown in UTC (decision 65),
    // not on the server's own clock.
    expect(vm.runInContext("typeof document", context)).toBe("undefined");
    expect(vm.runInContext("__format.defaultTimeZone()", context)).toBe("UTC");
    expect(vm.runInContext('__format.formatTime("2026-11-19T15:03:00Z")', context)).toBe("3:03 PM");
  });
});
