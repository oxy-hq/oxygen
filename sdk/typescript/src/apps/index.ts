// `@oxy-hq/sdk/apps` — what a function imports to handle a failed app-to-app call.
//
// ```ts
// import { isAppCallError } from "@oxy-hq/sdk/apps";
//
// try {
//   const place = await ctx.apps.call<{ id: string }>("locations", "lookup", { code });
// } catch (e) {
//   if (isAppCallError(e) && e.kind === "not_found") return Response.json({}, { status: 404 });
//   throw e;
// }
// ```
//
// The host throws a plain `HostError` whose message starts with a label
// (`AppCallRefused: ...`); the runtime bootstrap turns it into an
// `AppCallError`. Pure TypeScript, no React, no isolate-hostile builtins.

/** Why an app-to-app call failed. */
export type AppCallErrorKind =
  | "refused"
  | "not_found"
  | "not_callable"
  | "not_deployed"
  | "timeout"
  | "failed"
  | "busy"
  | "depth_exceeded";

/** Host message label -> kind. The label is the text before the first `": "`. */
export const APP_CALL_LABELS: Readonly<Record<string, AppCallErrorKind>> = {
  AppCallRefused: "refused",
  AppCallNotFound: "not_found",
  AppCallNotCallable: "not_callable",
  AppCallNotDeployed: "not_deployed",
  AppCallTimeout: "timeout",
  AppCallFailed: "failed",
  AppCallBusy: "busy",
  AppCallDepthExceeded: "depth_exceeded"
};

/**
 * Thrown by `ctx.apps.call` / `ctx.apps.send` when the call fails. A callee's
 * own thrown message passes through as `kind: "failed"`, cut to 500 characters
 * by the host.
 */
export class AppCallError extends Error {
  override readonly name = "AppCallError" as const;
  readonly kind: AppCallErrorKind;
  /** The host's whole message, `<Label>: <text>`; `message` is the text alone. */
  readonly hostMessage: string;

  constructor(kind: AppCallErrorKind, message: string, hostMessage?: string) {
    super(message);
    this.kind = kind;
    this.hostMessage = hostMessage ?? message;
    // Keep `instanceof` working when compiled to an ES5 target.
    Object.setPrototypeOf(this, AppCallError.prototype);
  }
}

/** True for an {@link AppCallError}, including one from another copy of the SDK. */
export function isAppCallError(e: unknown): e is AppCallError {
  return (
    e instanceof AppCallError ||
    (typeof e === "object" &&
      e !== null &&
      (e as { name?: unknown }).name === "AppCallError" &&
      typeof (e as { kind?: unknown }).kind === "string")
  );
}

/**
 * Parse a host message `<Label>: <text>` into an {@link AppCallError}, or
 * `null` when the label is not an app-call one (a different refusal, such as
 * `AppsCapabilityMissing`, stays a plain `HostError`).
 */
export function appCallErrorFromHostMessage(message: string): AppCallError | null {
  const at = message.indexOf(": ");
  if (at < 0) return null;
  // `typeof`, not a bare lookup: "constructor" and "__proto__" are on the prototype.
  const kind: unknown = APP_CALL_LABELS[message.slice(0, at)];
  if (typeof kind !== "string") return null;
  return new AppCallError(kind as AppCallErrorKind, message.slice(at + 2), message);
}
