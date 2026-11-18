// `@oxy-hq/sdk/events` — what an event listener function imports.
//
// ```ts
// import { onEvent, NonRetryableError } from "@oxy-hq/sdk/events";
//
// export default onEvent<{ id: string }>(async (event, ctx) => {
//   if (!event.data.id) throw new NonRetryableError("no id: retrying cannot help");
//   await ctx.oltp.exec("INSERT …", [event.data.id]);
// });
// ```
//
// Pure TypeScript, no React, no isolate-hostile builtins.

import type {
  OxyEvent,
  OxyFunctionContext,
  OxyFunctionHandler,
  OxyTableChange
} from "../custom-app/function-context";

export type {
  OxyDeliveryInfo,
  OxyEvent,
  OxyTableChange,
  OxyTableChangeData,
  OxyTableChangePage,
  OxyTablesApi
} from "../custom-app/function-context";

/** Whether `event` is a table change (`on: { table }`), narrowing its `data`. */
export function isTableChange(event: OxyEvent): event is OxyTableChange {
  return typeof event.type === "string" && event.type.startsWith("table:");
}

/**
 * Throw this from a listener to say a retry cannot help (a malformed payload,
 * a record that will never exist): the delivery is dead-lettered at once
 * instead of retried. Any other thrown error is retried.
 */
export class NonRetryableError extends Error {
  override readonly name = "NonRetryableError" as const;

  constructor(message?: string) {
    super(message);
    // Keep `instanceof` working when compiled to an ES5 target.
    Object.setPrototypeOf(this, NonRetryableError.prototype);
  }
}

function isEnvelope(value: unknown): value is OxyEvent {
  return (
    typeof value === "object" &&
    value !== null &&
    !Array.isArray(value) &&
    typeof (value as { specversion?: unknown }).specversion === "string"
  );
}

/**
 * Wrap an event handler as a function's default export. The request body is
 * the event envelope: it is parsed and handed over, a handler that returns
 * nothing answers 204, and a body that is not an event answers 400 without
 * running the handler.
 */
export function onEvent<T>(
  handler: (
    event: OxyEvent<T>,
    ctx: OxyFunctionContext
    // biome-ignore lint/suspicious/noConfusingVoidType: handlers commonly return nothing, and `() => {}` must be assignable
  ) => Promise<Response | void> | Response | void
): OxyFunctionHandler {
  return async (req, ctx) => {
    let parsed: unknown;
    try {
      parsed = JSON.parse(req.body);
    } catch {
      parsed = null;
    }
    if (!isEnvelope(parsed)) {
      return Response.json({ error: "request body is not an event envelope" }, { status: 400 });
    }
    const result = await handler(parsed as OxyEvent<T>, ctx);
    return result instanceof Response ? result : new Response(null, { status: 204 });
  };
}
