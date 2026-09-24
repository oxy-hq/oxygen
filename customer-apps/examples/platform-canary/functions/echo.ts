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
// ⚠️ THE MONITOR DOES NOT SEND A VALID SIGNATURE, AND THAT IS THE DESIGN.
// It POSTs a deliberately wrong one and asserts **401 + "signature mismatch"**.
//
// Reaching that specific body means the request got past `resolve_webhook_target`
// (Postgres answered org → app → published build → this declared webhook) and
// past `resolve_signing_keys` (the secret manager read a non-empty
// `CANARY_PROBE_SECRET`), and then actually ran the HMAC. The neighbouring 401s
// are what make it specific: "webhook signing secret is not configured" means
// the secret vanished, "signature is not hex" means the monitor's own header is
// malformed. Pinned by `a_wrong_signature_reports_mismatch_verbatim` in
// crates/app/src/server/api/webhooks/app_function.rs.
//
// What this buys, and why it beats sending a real signature:
//   • NO signing material outside this app. No SSM parameter, no TF_VAR_*, no
//     .mise.toml fetch, no redactions entry, no digest to recompute.
//   • Rotating CANARY_PROBE_SECRET cannot break the probe. With a valid
//     signature, rotation silently turns the monitor red until someone
//     remembers to recompute the digest.
//   • Nothing replayable sits in a monitoring vendor's config. A valid
//     signature over a fixed body is a standing bearer token for "enqueue this
//     job" — the route signs the raw body with no timestamp and no nonce.
//   • No junk in the durable queue. A valid signature would enqueue a real job
//     every tick (~288/day at 300s); a rejected one enqueues nothing.
//
// What it gives up: proof that the task queue accepts writes, which is the one
// step past verification. That is covered by `canary`'s own scheduled runs.
//
// ⚠️ CANARY_PROBE_SECRET IS NOT A MANAGED CREDENTIAL — do not treat it as one.
// Nothing ever reads its value: the monitor sends a knowingly wrong signature,
// and no human or system needs to know the right one. So generate something
// random, set it once, and record it NOWHERE. No rotation schedule (nothing on
// our side can leak it), no password-manager entry, no recovery path — if it is
// ever lost, set a new one and nothing breaks, because nothing was derived
// from it.
//
// It does still have to be UNGUESSABLE, and that is the one property that
// matters. It is the only thing keeping this anonymous route closed, and the
// route enqueues durable jobs with no rate limit — a public value would let
// anyone sign any body and file unbounded work.
//
// "Can it just be an env var?" — it already is one. `ctx.env.KEY` and a
// `webhook.secretVar` resolve the SAME key, `apps/<app_id>/<KEY>`, from the one
// value store a custom app has; "secret" vs "env var" is naming, not mechanism,
// so moving it would remove no step. The thing that would be a plain env var —
// a value written into the manifest — is forbidden precisely here: a manifest
// ships inside the bundle and is fetchable over the app's own host, so putting
// the value there publishes it.
//
// And the canary cannot mint it for itself, which was worth checking since it
// already holds `secrets.write` and calls `ctx.secrets.set` every run: the
// isolate has no CSPRNG. `ctx.crypto` is `hmac` / `verifyHmac` /
// `timingSafeEqual` only, and there is no `crypto.getRandomValues`, so a
// self-provisioned value would carry `Math.random()` strength. Not worth
// trading a real property for one deploy step.
//
// ⚠️ It also never runs this function. The route enqueues and answers 202 with
// an EMPTY body (`app_function_webhook` returns `Result<StatusCode, _>`; see its
// "Why it enqueues rather than runs" section), so there is no body to assert on
// even when the signature is right, and the return value below is only ever seen
// through entry point (1).
//
// So what the external probe proves, exactly:
//   • an ALB-reachable serve replica exists (the route is `route_fleet`)
//   • Postgres answers: org → app → published build → app_functions
//   • the secret-manager read works and the key is non-empty
//   • the HMAC path executes
// And what it does NOT prove:
//   • that any isolate ran, or that the worker fleet is alive at all
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
// Why a 401 is a real assertion where a 200 would not be: a custom-app host
// answers **200** with the SPA shell for every path (measured 2026-08-21 — the
// app host, a real custom-app host and a host for an app that does not exist
// were byte-identical). Nothing on that path produces a 401 carrying this body.

import type { OxyFunctionContext, OxyFunctionRequest } from "@oxy-hq/sdk";

export default async function echo(
  req: OxyFunctionRequest,
  _ctx: OxyFunctionContext
): Promise<Response> {
  const body: unknown = req.body ? JSON.parse(req.body) : {};
  return Response.json({ ok: true, echo: body });
}
