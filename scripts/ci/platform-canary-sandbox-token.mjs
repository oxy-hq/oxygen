/**
 * The sandbox agent token the sandbox loop runs as
 * (`platform-canary-sandbox-loop.mjs`;
 * `internal-docs/2026-10-03-sandbox-agent-credential-design.md` §7.2).
 *
 * The staff bearer mints one for the canary app, for an hour, the way a
 * person does in a browser: `POST /api/user/tokens`, which takes a session
 * and refuses every API token. This module is the mint, what stays with the
 * staff bearer and why, the requests that ask production directly, and the
 * one function everything printed about a failure goes through.
 *
 * Neither credential is ever on a command line or in this script's output:
 * both reach `oxyc` through `OXY_TOKEN`, the mint's response body is never
 * printed, and a failed command's output goes through `scrub`.
 */

/** A sandbox agent token, whole: the prefix and 36 base62 characters. */
export const SANDBOX_TOKEN_RE = /^oxy_sbx_[0-9A-Za-z]{36}$/;

/** Any new-format Oxy token, wherever it appears in a text. */
const ANY_TOKEN_RE = /oxy_(pat|sat|ci|sbx)_[0-9A-Za-z]{36}/g;

/**
 * What stays with the staff bearer, and why the sandbox agent token may not
 * do it. `step` 0 is the cleanup around the loop. Everything not listed here
 * runs as the token.
 */
export const STAFF_ONLY = [
  {
    step: 0,
    what: "delete dev-loop-a / dev-loop-b before step 1, after a failure, and at the end",
    why: "a sandbox an earlier run left was created by another credential, and the token deletes only its own"
  },
  {
    step: 5,
    what: "read the check run's answer from /api/admin/apps",
    why: "the admin surface answers the token 404, and `oxyc api` refuses it before any request"
  },
  {
    step: 8,
    what: "checks run on production, and its run detail",
    why: "production is not the token's: it runs checks only in a sandbox it created"
  },
  {
    step: 9,
    what: "list dev-loop-a's invocations once it is deleted",
    why: "the token reads the sandbox it has now, never the rows a deleted one left under its name"
  }
];

/** The body of the mint: one app, one hour. */
export function mintBody(appId) {
  return {
    name: "platform-canary sandbox loop",
    kind: "sandbox_agent",
    apps: [appId],
    expires_in_hours: 1
  };
}

/**
 * `text` with every credential taken out: each of `secrets` verbatim, and
 * anything shaped like a new-format Oxy token. What a failed command's
 * output is printed through.
 */
export function scrub(text, secrets = []) {
  let out = String(text ?? "");
  for (const secret of secrets) {
    if (secret) out = out.split(secret).join("[redacted]");
  }
  return out.replace(ANY_TOKEN_RE, (_, family) => `oxy_${family}_[redacted]`);
}

/**
 * Mint a sandbox agent token for app `appId` on `target` with the staff
 * session bearer. Answers the secret. Throws with the status and the
 * refusal's code — never the response body, which on success is the secret.
 */
export async function mintAgentToken(target, staffBearer, appId) {
  const response = await fetch(`${target}/api/user/tokens`, {
    method: "POST",
    headers: { authorization: `Bearer ${staffBearer}`, "content-type": "application/json" },
    body: JSON.stringify(mintBody(appId))
  });
  const body = await response.json().catch(() => ({}));
  if (response.status !== 201) {
    const code = typeof body?.code === "string" ? body.code : "(no code)";
    throw new Error(`mint answered ${response.status} ${code}`);
  }
  const secret = body?.secret;
  if (typeof secret !== "string" || !SANDBOX_TOKEN_RE.test(secret)) {
    throw new Error("mint answered 201 without a well-formed oxy_sbx_ secret");
  }
  if (body?.token?.kind !== "sandbox_agent") {
    throw new Error(`mint answered a ${body?.token?.kind} token`);
  }
  const { display_prefix: prefix, last_four: tail, expires_at: expires } = body.token;
  process.stdout.write(`   minted ${prefix}…${tail}, expires ${expires}\n`);
  return secret;
}

/**
 * What production is asked directly, as the token: `oxyc` refuses these
 * before any request, so the server's own answer needs a request `oxyc` will
 * not make. `app` is `<org-slug>/<app-slug>`. Each must answer the `404` an
 * unknown path gets.
 */
export function productionRequests(appId, app) {
  const api = `/api/customer-apps/${appId}`;
  const [orgSlug, appSlug] = app.split("/");
  return [
    { what: "queue a check run in production", path: `${api}/functions/canary/runs` },
    { what: "promote", path: `${api}/publish` },
    { what: "roll back", path: `${api}/rollback` },
    {
      what: "call a function in production",
      path: `/customer-apps/${orgSlug}/${appSlug}/fn/canary`
    }
  ];
}

/** Send each of `productionRequests` as the token; throw on any answer but 404. */
export async function productionRefusesTheToken(target, agentToken, appId, app) {
  for (const { what, path } of productionRequests(appId, app)) {
    const response = await fetch(`${target}${path}`, {
      method: "POST",
      headers: { authorization: `Bearer ${agentToken}`, "content-type": "application/json" },
      body: "{}"
    });
    if (response.status !== 404) {
      // The path and the status only: a body is not printed.
      throw new Error(`${what}: POST ${path} answered the token ${response.status}, not 404`);
    }
  }
}
