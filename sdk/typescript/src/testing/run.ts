// `t.run(fn)`: evaluate `fn` with the isolate's absent globals absent.
//
// A test runs under Node, where `Buffer`, `TextEncoder` and `process` exist; the
// isolate defines `Response`, `btoa`, `atob`, `console` and `__buildCtx` and
// nothing else (`runtime.rs`). The context cannot delete those globals for the
// whole file without breaking vitest, so this removes them from `globalThis`
// for the duration of ONE call and restores them after — the technique the
// canary uses for `process` (`new Function("process", script)(undefined)`),
// generalised. Inside `fn`, `new TextEncoder()` throws the `ReferenceError` it
// throws in production, and `btoa` / `atob` are the runtime's own, with its
// `TypeError`s.
//
// THE EXPLICIT PER-CALL FORM ONLY (`internal-docs/sdk-testing-context.md` §7,
// answer 7). A vitest environment that removes the globals for a whole file is
// what an app would eventually want, and may follow; shipping both now would
// mean two mechanisms with one job. Because this mutates `globalThis`, two
// `run` calls must not overlap: vitest runs one test at a time per file unless
// `test.concurrent` is used, and `run` refuses to nest.
//
// CALL THE FUNCTION UNDER TEST INSIDE; ASSERT OUTSIDE. What this removes, it
// removes from the whole process for the duration — and the test runner needs
// some of it. `expect(…)` inside `fn` throws `ReferenceError: Buffer is not
// defined` from vitest's own frames, not from the code under test, and so does
// `console.log`. That reads like a bug in the function being tested and is not
// one. Do the work inside, return what you want to check, assert after:
//
//     const rows = await t.run(() => handler(req, t.ctx));
//     expect(rows).toHaveLength(1);
//
// This is the price of the per-call form, and the strongest argument for the
// file-wide vitest environment above: there the runner's own frames never
// execute inside the window.

import { ABSENT_GLOBALS, REFUSALS } from "./host-contract";
import { contextError, render } from "./host-error";

const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const B64R = new Uint8Array(256).fill(255);
for (let i = 0; i < 64; i++) B64R[B64.charCodeAt(i)] = i;

/** `globalThis.btoa` as the isolate defines it: strings only, Latin1 only. */
export function isolateBtoa(input: unknown): string {
  if (input instanceof ArrayBuffer || ArrayBuffer.isView(input)) {
    throw new TypeError(render(REFUSALS.btoaBytes));
  }
  const s = String(input);
  let out = "";
  for (let i = 0; i < s.length; i += 3) {
    const c0 = s.charCodeAt(i);
    const c1 = i + 1 < s.length ? s.charCodeAt(i + 1) : 0;
    const c2 = i + 2 < s.length ? s.charCodeAt(i + 2) : 0;
    if (c0 > 0xff || c1 > 0xff || c2 > 0xff) {
      throw new TypeError(
        'btoa: input contains characters outside the Latin1 range; for text pass it directly with { encoding: "utf8" }'
      );
    }
    const n = (c0 << 16) | (c1 << 8) | c2;
    out +=
      B64[(n >> 18) & 63] +
      B64[(n >> 12) & 63] +
      (i + 1 < s.length ? B64[(n >> 6) & 63] : "=") +
      (i + 2 < s.length ? B64[n & 63] : "=");
  }
  return out;
}

/** `globalThis.atob` as the isolate defines it: WHATWG forgiving-base64. */
export function isolateAtob(input: unknown): string {
  let s = String(input).replace(/[ \t\n\f\r]/g, "");
  if (s.length % 4 === 0) {
    let pad = 0;
    while (pad < 2 && s.charCodeAt(s.length - 1) === 61) {
      s = s.slice(0, -1);
      pad++;
    }
  }
  if (s.indexOf("=") >= 0) throw new TypeError(render(REFUSALS.atobPadding));
  if (s.length % 4 === 1) throw new TypeError(render(REFUSALS.atobLength));
  let out = "";
  let buf = 0;
  let bits = 0;
  for (let i = 0; i < s.length; i++) {
    const code = s.charCodeAt(i);
    const v = code < 256 ? B64R[code] : 255;
    if (v === 255) throw new TypeError(`atob: invalid base64 character '${s[i]}'`);
    buf = (buf << 6) | v;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      out += String.fromCharCode((buf >> bits) & 0xff);
    }
  }
  return out;
}

let running = false;

/**
 * Evaluate `fn` with every `ABSENT_GLOBALS` entry removed from `globalThis` and
 * `btoa` / `atob` replaced by the isolate's, restoring all of them afterwards
 * — whether `fn` resolves or throws.
 */
export async function runWithoutIsolateGlobals<T>(fn: () => Promise<T> | T): Promise<T> {
  if (running) throw contextError("t.run(fn) calls must not overlap — one at a time per test");
  running = true;
  const saved: [string, PropertyDescriptor | undefined][] = [];
  const g = globalThis as Record<string, unknown>;
  const shadow = (name: string, value: unknown | undefined) => {
    saved.push([name, Object.getOwnPropertyDescriptor(globalThis, name)]);
    if (value === undefined) delete g[name];
    else Object.defineProperty(globalThis, name, { value, configurable: true, writable: true });
  };
  try {
    for (const entry of ABSENT_GLOBALS) shadow(entry.name, undefined);
    shadow("btoa", isolateBtoa);
    shadow("atob", isolateAtob);
    return await fn();
  } finally {
    for (const [name, descriptor] of saved.reverse()) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else delete g[name];
    }
    running = false;
  }
}
