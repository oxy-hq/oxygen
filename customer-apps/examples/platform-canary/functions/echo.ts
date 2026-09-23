// echo — answers `{ ok: true, echo: <request body> }`.
//
// Two entry points, and they prove very different things. Read the second one
// before writing anything that depends on it.
//
// 1. `/customer-apps/<org>/<app>/fn/echo`, session-authenticated. The canary's
//    UI invokes it through `useFunction` with a fresh nonce and checks the nonce
//    comes back (src/checks.ts). That proves the route path a live app's page
//    takes: session cookie → `/fn/echo` → isolate → the SSE result stream. This
//    is the only entry point that sees the value below.
//
// 2. `POST /api/webhooks/apps/<org>/<app>/echo`, anonymous and HMAC-signed
//    (`CANARY_PROBE_SECRET`). This exists for an EXTERNAL uptime monitor, which
//    cannot use (1): route-mode resolves a user identity, so probing it would
//    mean parking a long-lived user credential in a monitoring vendor.
//
// ⚠️ THE WEBHOOK ROUTE NEVER RUNS THIS FUNCTION INLINE, AND RETURNS NO BODY.
// It verifies the signature, enqueues a durable job and answers **202 with an
// empty body** (`app_function_webhook` returns `Result<StatusCode, _>`; see its
// "Why it enqueues rather than runs" section). The job is then executed by the
// WORKER fleet, and this function's return value lands on the run as
// `TaskOutcome::Done`, never on the HTTP response. So an external monitor
// cannot assert on the nonce, or on anything else in this body — it can only
// assert the status.
//
// What the external probe therefore proves, exactly:
//   • an ALB-reachable serve replica exists (the route is `route_fleet`)
//   • Postgres answers: org → app → published build → app_functions
//   • the secret manager read works, and HMAC verification passes
// And what it does NOT prove, which matters just as much:
//   • that any isolate ran — a total worker-fleet outage leaves it green at 202
//   • that the session-auth path in (1) works
//
// That is still the gap worth closing, because it is the SERVE side, and the
// serve side is what a readiness failure takes out: serve readiness keeps its
// Postgres dependency deliberately, so an RDS blip fails readiness on every
// serve pod, removing them from Service endpoints and therefore from every
// target group — custom-apps included — while the worker fleet keeps running
// schedules and checking in on time. Worker-side execution is already covered,
// by `canary`'s All Quiet check-in. The two are complementary; neither is the
// other's backstop.
//
// Why 202 is a real assertion where 200 would not be: a custom-app host answers
// **200** with the SPA shell for every path (measured 2026-08-21 — the app host,
// a real custom-app host and a host for an app that does not exist were
// byte-identical). Nothing on that path produces a 202. Reaching one means the
// handler ran and resolved all of the above.
//
// ⚠️ The signature is a standing credential. The route signs the raw body with
// no timestamp and no nonce, which is what lets a static monitor work at all —
// and also means the digest is replayable until `CANARY_PROBE_SECRET` rotates.
// Anyone who reads it out of the monitoring vendor's config or a proxy log can
// enqueue `echo` jobs, and this route carries no rate limit. Low severity (the
// job echoes its input and touches nothing), but it is why the secret is
// hand-rotated and why the probe interval is not set aggressively: every tick
// enqueues a real durable job the worker executes.

import type { OxyFunctionContext, OxyFunctionRequest } from "@oxy-hq/sdk";

export default async function echo(
  req: OxyFunctionRequest,
  _ctx: OxyFunctionContext
): Promise<Response> {
  const body: unknown = req.body ? JSON.parse(req.body) : {};
  return Response.json({ ok: true, echo: body });
}
