// `createTestContext({ apps, caller })` — `ctx.apps` and `ctx.caller` in a test.
//
// A call runs the handler registered for `"<app>/<function>"`, never another
// app's bundle; a send is recorded, never delivered. Both are gated as the
// host gates them: the manifest's top-level `calls` must name the pair.

import { type AppCallError, appCallErrorFromHostMessage } from "../apps";
import type { OxyCallerInfo } from "../custom-app/function-context";
import { eventId } from "./events";
import type { FunctionGates } from "./gates";
import { REFUSALS, SURFACES } from "./host-contract";
import { refuse, render } from "./host-error";

/** A handler standing in for another app's callable function. */
export type TestAppHandler = (body: unknown, opts?: object) => unknown | Promise<unknown>;

/** One `ctx.apps.send` a test context recorded. */
export interface SentAppCall {
  app: string;
  function: string;
  body: unknown;
  key: string | null;
  delaySeconds: number;
  eventId: string;
}

export interface TestApps {
  /** Every send that took effect, in order. */
  readonly sent: ReadonlyArray<SentAppCall>;
}

/** The host cuts a callee's thrown message to this many characters. */
export const APP_CALL_MESSAGE_MAX = 500;

const sinks = new WeakMap<TestApps, SentAppCall[]>();

export function createTestApps(): TestApps {
  const sent: SentAppCall[] = [];
  const apps: TestApps = { sent };
  sinks.set(apps, sent);
  return apps;
}

/** The host's refusal as the typed error a function sees (the runtime does the same mapping). */
function appCallError(host: string): AppCallError {
  const err = appCallErrorFromHostMessage(host);
  if (err === null) throw new Error(`test context bug: ${host} is not an app-call refusal`);
  return err;
}

/** The gates, in the host's order: capability, names, declared pair. */
export function checkAppCall(gates: FunctionGates, app: unknown, fn: unknown): [string, string] {
  if (gates.calls === null) throw refuse(SURFACES.apps, REFUSALS.appsCapability);
  if (typeof app !== "string" || typeof fn !== "string" || app === "" || fn === "") {
    throw appCallError(render(REFUSALS.appCallNames));
  }
  if (!gates.calls.has(`${app}/${fn}`)) {
    throw appCallError(render(REFUSALS.appCallUndeclared, { app, function: fn }));
  }
  return [app, fn];
}

/** Run the handler registered for the pair, as the host runs the callee. */
export async function runAppCall(
  handlers: Record<string, TestAppHandler>,
  app: string,
  fn: string,
  body: unknown,
  opts: object | undefined
): Promise<unknown> {
  const key = `${app}/${fn}`;
  const handler = new Map(Object.entries(handlers)).get(key);
  if (handler === undefined) {
    throw appCallError(
      `AppCallNotFound: no handler registered for ${key} (pass it in createTestContext's \`apps\` option)`
    );
  }
  try {
    const result = await handler(body, opts);
    // JSON in, JSON out: what crosses the host is what survives a round trip.
    return result === undefined ? null : JSON.parse(JSON.stringify(result));
  } catch (err) {
    const text = err instanceof Error ? err.message : String(err);
    throw appCallError(
      `AppCallFailed: ${Array.from(text).slice(0, APP_CALL_MESSAGE_MAX).join("")}`
    );
  }
}

/** Record a send on `apps`; the fake event id is stable for (org, type, key). */
export function recordSend(
  apps: TestApps,
  org: string,
  environment: string,
  call: Omit<SentAppCall, "eventId">
): { eventId: string } {
  const sink = sinks.get(apps);
  if (!sink) throw new Error("@oxy-hq/sdk/testing: apps was not made by createTestApps()");
  const id = eventId(
    org,
    environment,
    `apps/${call.app}/${call.function}`,
    call.key ?? `send-${sink.length + 1}`
  );
  sink.push({ ...call, eventId: id });
  return { eventId: id };
}

/** `ctx.caller` from the option: a copy, `null` when absent. */
export function callerFrom(caller: OxyCallerInfo | null | undefined): OxyCallerInfo | null {
  return caller ? { ...caller } : null;
}
