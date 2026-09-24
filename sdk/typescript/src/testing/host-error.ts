// How the context throws: as the runtime throws. `__wrapOp` in `runtime.rs`
// turns the `reply_json` envelope `{ __oxyError: "HostError", message }` into an
// `Error` whose `name` is `"HostError"` and whose message is
// `"<surface>: <host message>"`. A test that matches on `err.name` or on the
// message prefix sees exactly what a function sees in production.

import type { RefusalTemplate } from "./host-contract";

/** The `name` the runtime gives every refusal it unwraps. */
export const HOST_ERROR_NAME = "HostError";

/**
 * Render a refusal template: every `{placeholder}` replaced by `vars[name]`.
 * A placeholder the caller did not supply is a bug in the context, not in the
 * app under test, so it throws rather than leaking `{database}` into a message
 * a test might then match on.
 */
export function render(
  template: RefusalTemplate,
  vars: Record<string, string | number> = {}
): string {
  return template.refusal.replace(/\{([A-Za-z_][A-Za-z0-9_]*)\}/g, (whole, name: string) => {
    const value = vars[name];
    if (value === undefined) {
      throw new Error(`test context bug: refusal template leaves {${name}} unfilled: ${whole}`);
    }
    return String(value);
  });
}

/**
 * The error a function sees when the host refuses: `name` `"HostError"`,
 * message `"<surface>: <message>"`.
 */
export function hostError(surface: string, message: string): Error {
  const err = new Error(`${surface}: ${message}`);
  err.name = HOST_ERROR_NAME;
  return err;
}

/** `hostError` over a rendered template. */
export function refuse(
  surface: string,
  template: RefusalTemplate,
  vars: Record<string, string | number> = {}
): Error {
  return hostError(surface, render(template, vars));
}

/** Whether `err` is a refusal the context (or the runtime) threw. */
export function isHostError(err: unknown): err is Error {
  return err instanceof Error && err.name === HOST_ERROR_NAME;
}

/**
 * An error in the context's OWN words — never the host's. Thrown where the
 * context cannot model what the host would do (an unrecognised statement, an
 * unregistered `fetch` answer) and says so, rather than answering something
 * production would not. Its `name` is `"TestContextError"` so a test can never
 * mistake it for a refusal.
 */
export function contextError(message: string): Error {
  const err = new Error(`@oxy-hq/sdk/testing: ${message}`);
  err.name = "TestContextError";
  return err;
}
