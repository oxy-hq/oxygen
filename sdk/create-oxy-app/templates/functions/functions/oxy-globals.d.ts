// Globals the Oxy Functions isolate provides. Declared here rather than in
// `@oxy-hq/sdk` on purpose: this file is picked up ONLY by functions/tsconfig.json
// (which has no DOM lib), so these types can never leak into the browser half of
// the app, where `btoa` exists but behaves differently.
//
// Not a function entry — `oxyc publish` bundles the files named in oxy-app.json's
// `functions` map, so a .d.ts sitting here is inert.
//
// The isolate is bare V8, not a browser or Node: ECMAScript (this config's
// `lib: ["ES2022"]`) plus the four globals below, which its bootstrap installs.
// There is no `TextEncoder`, `Buffer`, `Blob`, `crypto`, `setTimeout` or `fetch`
// (use `ctx.fetch`), and naming one is a type error here as it is an error there.
//
// An import can still pull the DOM lib in: `@oxy-hq/sdk/email` does, through
// preact's JSX types. `console` is written in the shape `lib.dom.d.ts` uses (an
// interface plus a `var`), so it merges with the DOM's cleanly. The isolate's
// `Response` differs from the fetch API's and cannot, so keep this config's
// `skipLibCheck` on: with it off, that difference is reported here as errors.

/** The second argument of `new Response(…)` and `Response.json(…)`. */
interface ResponseInit {
  /** Defaults to 200. */
  status?: number;
  /**
   * Kept on the object, but the platform answers with the status and body
   * alone, so a header set here does not reach the caller.
   */
  headers?: Record<string, string>;
}

/**
 * What a function returns: the isolate's own small `Response`, not the fetch
 * API's. It holds a text body, a status and headers; an instance has no `ok`,
 * `text()` or `json()`. Read-only here because a function's unit tests run on
 * Node, whose `Response` throws on a write to these.
 */
interface Response {
  readonly status: number;
  readonly headers: Record<string, string>;
  /** Sent as text: build it with `JSON.stringify`, or use `Response.json`. */
  readonly body: string;
}

declare var Response: {
  prototype: Response;
  new (body?: string | null, init?: ResponseInit): Response;
  /** `JSON.stringify(value)` as the body. */
  json(value: unknown, init?: ResponseInit): Response;
};

/**
 * The function's log, the same sink as `ctx.log`. A non-string argument is
 * written as JSON; `debug`, `trace` and `dir` log at info. The grouping,
 * counting, timing and `table` methods exist so that calling one cannot throw,
 * and do nothing.
 */
interface Console {
  log(...data: unknown[]): void;
  info(...data: unknown[]): void;
  debug(...data: unknown[]): void;
  warn(...data: unknown[]): void;
  error(...data: unknown[]): void;
  trace(...data: unknown[]): void;
  dir(...data: unknown[]): void;
  /** Logs `data` as an error when `condition` is falsy. */
  assert(condition?: boolean, ...data: unknown[]): void;
  table(...data: unknown[]): void;
  group(...data: unknown[]): void;
  groupCollapsed(...data: unknown[]): void;
  groupEnd(): void;
  count(label?: string): void;
  countReset(label?: string): void;
  time(label?: string): void;
  timeEnd(label?: string): void;
  timeLog(label?: string, ...data: unknown[]): void;
}

declare var console: Console;

/**
 * Base64-encode a **Latin1 string**.
 *
 * Characters above U+00FF throw, and U+0080..U+00FF encode as one byte each —
 * meaning `btoa(utf8Text)` yields mojibake, not an error. For text, don't
 * encode at all: pass the string with `{ encoding: "utf8" }` to
 * `ctx.email.send` attachments or `ctx.storage.put`.
 *
 * For **bytes**, use `bytesToBase64` from `@oxy-hq/sdk`. Passing a
 * `Uint8Array` here throws: the Web spec would stringify it to "37,80,68,70"
 * and silently encode that text instead of your file.
 */
declare function btoa(data: string): string;

/**
 * Decode base64 to a Latin1 string. Throws on malformed input (including a
 * misplaced `=`) rather than returning a truncated result.
 *
 * To get bytes back, use `base64ToBytes` from `@oxy-hq/sdk`.
 */
declare function atob(data: string): string;
