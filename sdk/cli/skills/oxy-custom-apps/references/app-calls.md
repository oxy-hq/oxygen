# One app calling another

An app's function can call a function of another app in the same organization, and wait for the answer (`ctx.apps.call`) or just queue the call (`ctx.apps.send`). Use it when another app owns the data or the rule: ask it, instead of copying its tables.

## Declare it on both sides

The callee marks the function callable. Nothing else about the function changes; it stays an ordinary function with its own routes, schedule and timeout.

```jsonc
// locations/oxy-app.json
{ "functions": { "lookup": { "callable": true } } }
```

The caller lists every (app, function) pair it may call, at the top level of its own `oxy-app.json`. Keys are app slugs, values are non-empty lists of function names with no repeats, 64 pairs at most in total.

```jsonc
// orders/oxy-app.json
{ "calls": { "locations": ["lookup"] } }
```

`oxyc validate` checks the shape. `oxyc publish` warns, and does not fail, when a declared target does not exist yet or is not `callable`, so two apps can ship in either order; the call itself fails until both sides are live.

## Call it

```ts
import { isAppCallError } from "@oxy-hq/sdk/apps";

const place = await ctx.apps.call<{ id: string; name: string }>(
  "locations", "lookup", { code }, { timeoutMs: 5000 }
);

const { eventId } = await ctx.apps.send("billing", "charge", { orderId }, { key: orderId, delaySeconds: 30 });
```

- `call<T>(app, fn, body?, { timeoutMs? })` returns the callee's JSON answer. `send(app, fn, body?, { delaySeconds?, key? })` returns `{ eventId }` at once; the same `key` is the same send.
- Body and answer are JSON, under the existing request body limit.
- A `send` made on a token that expires is refused (`AppCallRefused`) when `delaySeconds` would run to or past that expiry, since the delivery would find the credential gone and kill the send. The message names the delay and says the credential this run is on expires first: shorten the delay. A login session, a token that never expires and a run as the system are not limited this way.
- Using `ctx.apps` with no top-level `calls`, or on a pair `calls` does not list, is refused (`AppsCapabilityMissing`, `AppCallRefused`). `oxyc validate` and `oxyc publish` lint literal calls for both.

## Same environment, never production by fallback

Production calls the target's production build, staging calls its staging build, and a sandbox calls the target's same sandbox. A call never falls back to production: if the target has nothing deployed in that environment it fails with `not_deployed`.

## Identity: the callee runs as itself

A `call` made while a person is behind the chain runs as that person: `ctx.user` is them, with their role, teams and reach resolved again for the callee's app, and the platform first checks they may open the target app (a member who cannot open an app restricted to a team cannot reach it through yours). A call from a background run (a schedule, an event, a webhook) runs as the system identity, as those runs do. A `send` is delivered later, as the same person when one sent it: the platform checks again, at delivery, that the credential they sent it with is still valid (a revoked or expired token fails) and that they may open the target app, and if not the delivery dies with a `sender_refused` error and your function never runs. A `send` from a background run is delivered as the system identity (`ctx.user.kind === "system"`). `ctx.caller` says who called, for attribution and logs only:

```ts
ctx.caller; // null, or { app, function, invocationId, actor }
```

`ctx.caller` is `null` unless another app's call or send started this invocation. `actor` is the person behind a `call` or a delivered `send` (their id), or `null` when no person was behind it.

## Errors

A failed call throws `AppCallError` from `@oxy-hq/sdk/apps`, with `.kind`, `.message` (the text) and `.hostMessage` (the host's whole `<Label>: <text>`):

| `kind` | Meaning |
| --- | --- |
| `refused` | Your manifest does not declare the call, or the names are empty. |
| `not_found` | No such app or function. |
| `not_callable` | The function exists but is not marked `callable`. |
| `not_deployed` | The target has no build in this environment. |
| `timeout` | The budget ran out before an answer. |
| `failed` | The callee threw or answered an error; its message is passed through, cut to 500 characters. |
| `busy` | The target could not take the call right now; retry later. |
| `depth_exceeded` | The chain of calls and events is more than 8 deep. |
| `cycle` | The target function is already on this chain of synchronous calls (A.f → B.g → A.f); the message names the chain. A call into a different function of an app already on the chain is allowed. |

```ts
try {
  await ctx.apps.call("locations", "lookup", { code });
} catch (e) {
  if (isAppCallError(e) && e.kind === "not_found") return Response.json({}, { status: 404 });
  throw e;
}
```

## Limits

- **Depth 8**, shared with events: a call made by a function that an event or another call started counts one deeper, and a chain past 8 is refused with `depth_exceeded`.
- **Budget**: the callee's timeout is the smallest of your `timeoutMs`, the callee function's own `timeoutSeconds`, and what is left of the caller's time.
- Each call is its own invocation (mode `call`), with the caller's invocation as its parent: `oxyc invocations list` shows `parent <id>` on it and `--json` carries `parent_invocation_id`.

## Not supported

- Reading another app's OLTP records, or a transaction shared across apps. Call a function of the owning app that does the read, and return what you need.

## Testing

`createTestContext` from `@oxy-hq/sdk/testing` takes `apps` (handlers keyed `"<app>/<function>"`) and `caller`. A call with no handler throws `not_found`; the declared-pair rules are enforced as in production; `state.apps.sent` lists each `send`.

```ts
const t = createTestContext(manifest, {
  function: "place",
  apps: { "locations/lookup": () => ({ id: "l1", name: "Main" }) },
});
```
