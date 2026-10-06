/**
 * The main step, against a faked runner and a mocked `fetch`.
 *
 * What is pinned here is the action's CONTRACT with the three things around
 * it: GitHub (audience `oxy`, a bearer on the id-token request, file commands),
 * the deployment (the exchange body, every refusal code) and the rest of the
 * job (`OXY_TOKEN` exported, masked first, never printed).
 */

import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { audienceFor } from "../src/oxy.mjs";
import { readInputs, setup } from "../src/setup.mjs";
import {
  assignments,
  EXCHANGE,
  failure,
  fakeRunner,
  GITHUB_ID_TOKEN,
  GITHUB_OK,
  HOST,
  MINTED
} from "./fake-runner.mjs";

/** The id of `acme/deployer`: what the `service-account` input carries. */
const ACCOUNT_ID = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";

const INPUTS = {
  INPUT_VERSION: "0.6.0",
  "INPUT_SERVICE-ACCOUNT": ACCOUNT_ID,
  INPUT_HOST: HOST,
  "INPUT_EXPORT-TOKEN": "true"
};

const OK = { ...GITHUB_OK, [EXCHANGE]: () => ({ status: 200, body: MINTED }) };

/** @param {import("./fake-runner.mjs").Reply} reply */
const refusing = (reply) => ({ ...GITHUB_OK, [EXCHANGE]: () => reply });

describe("the main step", () => {
  test("installs the pinned oxyc with no install scripts, and puts it on PATH", async () => {
    const runner = fakeRunner({ env: INPUTS, routes: OK });
    await setup(runner.io);
    assert.deepEqual(runner.commands[0], [
      "npm",
      "install",
      "--prefix",
      "/runner/temp/oxyc",
      "--no-audit",
      "--no-fund",
      "--ignore-scripts",
      "@oxy-hq/cli@0.6.0"
    ]);
    assert.equal(runner.files["/files/path"], "/runner/temp/oxyc/node_modules/.bin\n");
    assert.deepEqual(runner.commands[1], ["/runner/temp/oxyc/node_modules/.bin/oxyc", "--version"]);
    assert.ok(runner.lines.includes("installed oxyc 0.6.0"));
  });

  test("asks GitHub for a token for the host it is about to post to, and asks the deployment nothing", async () => {
    const runner = fakeRunner({
      env: INPUTS,
      routes: {
        ...OK,
        // A deployment that WOULD name an audience — another deployment's — to
        // show it is never asked, and so cannot.
        [`GET ${HOST}/api/auth/oidc/audience`]: () => ({
          status: 200,
          body: { audience: "oxy:app.oxygen-hq.com" }
        })
      }
    });
    await setup(runner.io);
    const [minted, exchanged] = runner.requests;
    // Two requests, and the token is for the host's own audience.
    assert.equal(audienceFor(HOST), "oxy:oxy.test");
    assert.equal(`${minted?.method} ${minted?.url}`, GITHUB_ID_TOKEN);
    assert.match(String(minted?.url), /audience=oxy%3Aoxy\.test$/);
    assert.equal(minted?.headers.authorization, "bearer gh-request-token");
    assert.equal(`${exchanged?.method} ${exchanged?.url}`, EXCHANGE);
    assert.deepEqual(JSON.parse(exchanged?.body ?? ""), {
      token: "gh-jwt-1",
      service_account: ACCOUNT_ID
    });
    assert.equal(runner.requests.length, 2);
    assert.ok(!runner.requests.some((request) => request.url.includes("/api/auth/oidc/audience")));
  });

  /**
   * URL → audience. THE SAME LIST is in `crates/auth/src/github_oidc/audience.rs`
   * (the server deriving its own, from the URL it calls itself by) and in
   * `sdk/cli/src/auth/oidc.test.ts`. The three must never disagree: a client
   * that derived anything else would be refused by the very deployment it was
   * pointed at. Change one, change all.
   */
  const AUDIENCE_CASES = /** @type {const} */ ([
    // The three deployments, as `oxyc --env` and `OXY_API_URL` name them.
    ["https://app.oxygen-hq.com", "oxy:app.oxygen-hq.com"],
    ["https://aip.dev.oxy.tech", "oxy:aip.dev.oxy.tech"],
    ["https://aip.staging.oxy.tech", "oxy:aip.staging.oxy.tech"],
    // The scheme's default port is dropped…
    ["https://app.oxygen-hq.com:443", "oxy:app.oxygen-hq.com"],
    ["http://app.oxygen-hq.com:80", "oxy:app.oxygen-hq.com"],
    // …and any other port is kept: it is part of which deployment it is.
    ["http://localhost:3000", "oxy:localhost:3000"],
    ["https://app.oxygen-hq.com:8443", "oxy:app.oxygen-hq.com:8443"],
    ["http://localhost:443", "oxy:localhost:443"],
    // The host is lowercased.
    ["https://App.Oxygen-HQ.com", "oxy:app.oxygen-hq.com"],
    // Only the address counts: not a path such as `/api`, nor a slash.
    ["https://app.oxygen-hq.com/api", "oxy:app.oxygen-hq.com"],
    ["https://app.oxygen-hq.com/", "oxy:app.oxygen-hq.com"],
    ["https://app.oxygen-hq.com/api/", "oxy:app.oxygen-hq.com"],
    ["https://app.oxygen-hq.com:443/api?x=1#y", "oxy:app.oxygen-hq.com"],
    // An IPv6 literal keeps its brackets.
    ["http://[::1]:3000", "oxy:[::1]:3000"],
    ["https://[2001:db8::1]", "oxy:[2001:db8::1]"],
    ["https://[2001:db8::1]:443/api", "oxy:[2001:db8::1]"],
    // Credentials in a URL are no part of the address.
    ["https://user:secret@app.oxygen-hq.com", "oxy:app.oxygen-hq.com"]
  ]);

  test("derives the audience from `host` exactly as the server derives its own", () => {
    for (const [url, audience] of AUDIENCE_CASES) {
      assert.equal(audienceFor(url), audience, url);
      // Never the plain one: a token for it would be good at every deployment
      // that has no public URL.
      assert.notEqual(audienceFor(url), "oxy", url);
    }
  });

  test("has no fallback audience: a deployment with no exchange is still only asked for its own host's token", async () => {
    // Everything on the deployment answers 404 — an old one, or one pretending.
    const runner = fakeRunner({ env: INPUTS, routes: GITHUB_OK });
    await setup(runner.io);
    assert.deepEqual(
      runner.requests.map((request) => `${request.method} ${request.url}`),
      [GITHUB_ID_TOKEN, EXCHANGE]
    );
  });

  test("drops a trailing slash from the host", async () => {
    const runner = fakeRunner({ env: { ...INPUTS, INPUT_HOST: `${HOST}/` }, routes: OK });
    await setup(runner.io);
    assert.equal(runner.requests[1]?.url, `${HOST}/api/auth/oidc/exchange`);
  });

  test("requires `service-account`: with none, installs nothing and asks nobody for a token", async () => {
    // The deployment never picks an account on a run's behalf, so a step that
    // names none has nothing to exchange for.
    for (const named of ["", "   ", undefined]) {
      const runner = fakeRunner({ env: { ...INPUTS, "INPUT_SERVICE-ACCOUNT": named }, routes: OK });
      const cause = await failure(() => setup(runner.io));
      assert.equal(cause.message, "`service-account` is required");
      assert.match(cause.hints.join("\n"), /by its ID/);
      assert.match(cause.hints.join("\n"), /API access → Service accounts/);
      assert.deepEqual(runner.commands, []);
      assert.deepEqual(runner.requests, []);
      assert.equal(runner.files["/files/env"], undefined);
    }
  });

  test("exports OXY_TOKEN, sets the outputs, and saves the token for the post step", async () => {
    const runner = fakeRunner({ env: INPUTS, routes: OK });
    await setup(runner.io);
    assert.deepEqual(assignments(runner.files["/files/env"]), { OXY_TOKEN: MINTED.token });
    assert.deepEqual(assignments(runner.files["/files/output"]), {
      token: MINTED.token,
      "token-id": "tok-1",
      "expires-at": "2026-10-01T12:15:00Z",
      "service-account": "acme/deployer"
    });
    assert.deepEqual(assignments(runner.files["/files/state"]), {
      token: MINTED.token,
      host: HOST
    });
  });

  test("masks both tokens, and prints neither anywhere else", async () => {
    const runner = fakeRunner({ env: INPUTS, routes: OK });
    await setup(runner.io);
    assert.ok(runner.lines.includes(`::add-mask::${MINTED.token}`));
    assert.ok(runner.lines.includes("::add-mask::gh-jwt-1"));
    const unmasked = runner.lines.filter((line) => !line.startsWith("::add-mask::"));
    for (const line of unmasked) {
      assert.ok(!line.includes(MINTED.token), line);
      assert.ok(!line.includes("gh-jwt-1"), line);
    }
    // The summary a person reads names the account, not the credential.
    assert.ok(
      unmasked.some((line) => line.includes("signed in to https://oxy.test as acme/deployer"))
    );
  });

  test("masks the token before it is written to any file", async () => {
    const runner = fakeRunner({ env: INPUTS, routes: OK });
    await setup(runner.io);
    const masked = runner.events.indexOf(`write ::add-mask::${MINTED.token}`);
    const firstUse = runner.events.findIndex(
      (event) => event.startsWith("append ") && event.includes(MINTED.token)
    );
    assert.ok(masked >= 0 && firstUse >= 0);
    assert.ok(masked < firstUse, "the mask must be registered before the token goes anywhere");
  });

  test("saves the token for the post step before exporting it, so a failed export is still revoked", async () => {
    const runner = fakeRunner({ env: { ...INPUTS, GITHUB_ENV: undefined }, routes: OK });
    const cause = await failure(() => setup(runner.io));
    assert.match(cause.message, /GITHUB_ENV is not set/);
    assert.equal(assignments(runner.files["/files/state"]).token, MINTED.token);
  });

  test("with export-token: false, the token is an output only — and is still revoked later", async () => {
    const runner = fakeRunner({ env: { ...INPUTS, "INPUT_EXPORT-TOKEN": "false" }, routes: OK });
    await setup(runner.io);
    assert.equal(runner.files["/files/env"], undefined);
    assert.equal(assignments(runner.files["/files/output"]).token, MINTED.token);
    assert.equal(assignments(runner.files["/files/state"]).token, MINTED.token);
  });

  test("uses the defaults: latest, the production host, and exporting", async () => {
    const runner = fakeRunner({
      // The one input with no default: the account to act as.
      env: { "INPUT_SERVICE-ACCOUNT": ACCOUNT_ID },
      routes: {
        // The default host's own audience: the token is for where it is sent.
        "GET https://gh.test/token?api-version=2.0&audience=oxy%3Aapp.oxygen-hq.com": () => ({
          status: 200,
          body: { value: "gh-jwt-1" }
        }),
        "POST https://app.oxygen-hq.com/api/auth/oidc/exchange": () => ({
          status: 200,
          body: MINTED
        })
      }
    });
    await setup(runner.io);
    assert.equal(runner.commands[0]?.at(-1), "@oxy-hq/cli@latest");
    assert.equal(assignments(runner.files["/files/env"]).OXY_TOKEN, MINTED.token);
    assert.equal(assignments(runner.files["/files/state"]).host, "https://app.oxygen-hq.com");
  });
});

describe("a job that cannot sign in", () => {
  test("without `id-token: write`, fails before installing anything and says which permission", async () => {
    const runner = fakeRunner({
      env: {
        ...INPUTS,
        ACTIONS_ID_TOKEN_REQUEST_URL: undefined,
        ACTIONS_ID_TOKEN_REQUEST_TOKEN: undefined
      },
      routes: OK
    });
    const cause = await failure(() => setup(runner.io));
    assert.match(cause.message, /cannot request a GitHub OIDC token/);
    assert.match(cause.hints.join("\n"), /permissions: id-token: write/);
    assert.deepEqual(runner.commands, []);
    assert.deepEqual(runner.requests, []);
  });

  test("`service_account_required` names the input to set", async () => {
    const runner = fakeRunner({
      env: INPUTS,
      routes: refusing({
        status: 400,
        body: { error: "name the service account", code: "service_account_required" }
      })
    });
    const cause = await failure(() => setup(runner.io));
    assert.match(cause.message, /must be named by its ID/);
    assert.match(cause.hints.join("\n"), /`service-account` input/);
  });

  test("`no_matching_policy` says where to register one, and what the run looked like", async () => {
    const runner = fakeRunner({
      env: {
        ...INPUTS,
        GITHUB_REPOSITORY: "acme-co/acme-apps",
        GITHUB_WORKFLOW_REF: "acme-co/acme-apps/.github/workflows/release.yml@refs/heads/main"
      },
      routes: refusing({ status: 403, body: { error: "no policy", code: "no_matching_policy" } })
    });
    const cause = await failure(() => setup(runner.io));
    assert.match(cause.message, /no trust policy of the named service account matches/);
    assert.match(cause.hints.join("\n"), /`service-account` input/);
    const hints = cause.hints.join("\n");
    assert.match(hints, /Organization settings → API access → Service accounts/);
    assert.match(hints, /oxyc init-ci/);
    assert.match(hints, /repository: acme-co\/acme-apps/);
    assert.match(hints, /release\.yml@refs\/heads\/main/);
  });

  for (const [code, status, says] of /** @type {const} */ ([
    ["invalid_token", 401, /rejected the GitHub OIDC token/],
    ["wrong_audience", 401, /wrong audience/],
    ["expired", 401, /expired before it was exchanged/],
    ["replayed", 401, /already been used/],
    ["pull_request_target", 403, /pull_request_target/],
    ["self_hosted_runner", 403, /self-hosted runner/],
    ["missing_environment", 403, /environment is required and missing/]
  ])) {
    test(`\`${code}\` is explained, and exports nothing`, async () => {
      const runner = fakeRunner({ env: INPUTS, routes: refusing({ status, body: { code } }) });
      const cause = await failure(() => setup(runner.io));
      assert.match(cause.message, says);
      assert.ok(cause.hints.length > 0);
      assert.equal(runner.files["/files/env"], undefined);
      assert.equal(runner.files["/files/state"], undefined);
    });
  }

  test("`wrong_audience` names the address the deployment answers to, and the input to set", async () => {
    // The fix is where the action is pointed — never which audience it asks for.
    const runner = fakeRunner({
      env: INPUTS,
      routes: refusing({
        status: 401,
        body: { code: "wrong_audience", audience: "oxy:app.oxygen-hq.com" }
      })
    });
    const cause = await failure(() => setup(runner.io));
    assert.match(cause.message, /answers to another address/);
    const hints = cause.hints.join("\n");
    assert.match(hints, /this deployment answers to app\.oxygen-hq\.com/);
    assert.match(hints, /\(here oxy\.test\)/);
    assert.match(hints, /Set the `host` input to https:\/\/app\.oxygen-hq\.com/);
  });

  test("`wrong_audience` from a deployment with no public URL says GitHub sign-in is not available there", async () => {
    // Plain `oxy` is what a deployment with no OXY_API_URL takes, and it is
    // never asked for: a token for it would be good at every such deployment.
    const runner = fakeRunner({
      env: INPUTS,
      routes: refusing({ status: 401, body: { code: "wrong_audience", audience: "oxy" } })
    });
    const cause = await failure(() => setup(runner.io));
    assert.equal(cause.message, "GitHub sign-in is not available on this deployment");
    assert.match(cause.hints.join("\n"), /no public URL configured \(OXY_API_URL\)/);
    assert.equal(runner.files["/files/env"], undefined);
  });

  test("an unknown refusal reports the status and the server's own words", async () => {
    const runner = fakeRunner({
      env: INPUTS,
      routes: refusing({ status: 403, body: { error: "policy disabled", code: "brand_new" } })
    });
    const cause = await failure(() => setup(runner.io));
    assert.equal(cause.message, "the OIDC token exchange failed (403)");
    assert.deepEqual(cause.hints, ["policy disabled"]);
  });

  test("a 400 with no code is an unreadable request, and says what an account name looks like", async () => {
    const runner = fakeRunner({
      env: INPUTS,
      routes: refusing({
        status: 400,
        body: { error: "'service_account' must be '<org_slug>/<name>'" }
      })
    });
    const cause = await failure(() => setup(runner.io));
    assert.equal(cause.message, "the deployment could not read the exchange request");
    assert.equal(cause.hints[0], "'service_account' must be '<org_slug>/<name>'");
    assert.match(cause.hints.join("\n"), /rewriting request bodies/);
  });

  test("a 429 is waited out once for its Retry-After, with a fresh id token", async () => {
    const runner = fakeRunner({
      env: INPUTS,
      routes: {
        ...GITHUB_OK,
        [EXCHANGE]: (_request, nth) =>
          nth === 1
            ? { status: 429, body: { code: "rate_limited" }, headers: { "retry-after": "7" } }
            : { status: 200, body: MINTED }
      }
    });
    await setup(runner.io);
    const sent = runner.requests
      .filter((request) => request.method === "POST")
      .map((request) => JSON.parse(request.body ?? "").token);
    assert.deepEqual(sent, ["gh-jwt-1", "gh-jwt-2"]);
    assert.deepEqual(runner.sleeps, [7_000]);
    assert.equal(assignments(runner.files["/files/env"]).OXY_TOKEN, MINTED.token);
  });

  test("a second 429 is final: the wait is capped, and the refusal says why", async () => {
    const runner = fakeRunner({
      env: INPUTS,
      routes: refusing({
        status: 429,
        body: { error: "too many requests; try again later", code: "rate_limited" },
        headers: { "retry-after": "3600" }
      })
    });
    const cause = await failure(() => setup(runner.io));
    assert.match(cause.message, /rate-limiting token exchanges/);
    assert.match(cause.hints.join("\n"), /already waited once and retried/);
    assert.equal(runner.requests.filter((request) => request.method === "POST").length, 2);
    assert.deepEqual(runner.sleeps, [60_000]);
    assert.equal(runner.files["/files/env"], undefined);
  });

  test("a deployment with no exchange is a warning, not a failure: oxyc stays installed", async () => {
    // No exchange route: the fake answers 404, as an older deployment does.
    const runner = fakeRunner({ env: INPUTS, routes: GITHUB_OK });
    await setup(runner.io);
    const warned = runner.lines.find((line) => line.startsWith("::warning::"));
    assert.match(warned ?? "", /has no OIDC token exchange/);
    assert.match(warned ?? "", /oxyc publish/);
    assert.equal(runner.files["/files/path"], "/runner/temp/oxyc/node_modules/.bin\n");
    assert.equal(runner.files["/files/env"], undefined);
    assert.equal(runner.files["/files/state"], undefined);
  });

  test("a 5xx is retried with a fresh id token each time", async () => {
    const runner = fakeRunner({
      env: INPUTS,
      routes: {
        ...GITHUB_OK,
        [EXCHANGE]: (_request, nth) =>
          nth < 3 ? { status: 503, body: {} } : { status: 200, body: MINTED }
      }
    });
    await setup(runner.io);
    const sent = runner.requests
      .filter((request) => request.method === "POST")
      .map((request) => JSON.parse(request.body ?? "").token);
    // A GitHub id token is single-use on the deployment's side.
    assert.deepEqual(sent, ["gh-jwt-1", "gh-jwt-2", "gh-jwt-3"]);
    assert.deepEqual(runner.sleeps, [1_000, 3_000]);
    assert.equal(assignments(runner.files["/files/env"]).OXY_TOKEN, MINTED.token);
  });

  test("gives up after three attempts at an unreachable deployment", async () => {
    const runner = fakeRunner({
      env: INPUTS,
      routes: { ...GITHUB_OK, [EXCHANGE]: () => new TypeError("fetch failed") }
    });
    const cause = await failure(() => setup(runner.io));
    assert.match(
      cause.message,
      /could not reach https:\/\/oxy\.test\/api\/auth\/oidc\/exchange: fetch failed/
    );
    assert.equal(runner.requests.filter((request) => request.method === "POST").length, 3);
  });

  test("does not retry a refusal — the deployment's answer is final", async () => {
    const runner = fakeRunner({
      env: INPUTS,
      routes: refusing({ status: 403, body: { code: "no_matching_policy" } })
    });
    await failure(() => setup(runner.io));
    assert.equal(runner.requests.filter((request) => request.method === "POST").length, 1);
    assert.deepEqual(runner.sleeps, []);
  });

  test("fails when GitHub will not mint the id token", async () => {
    const runner = fakeRunner({
      env: INPUTS,
      routes: { [GITHUB_ID_TOKEN]: () => ({ status: 403, body: {} }) }
    });
    const cause = await failure(() => setup(runner.io));
    assert.match(cause.message, /GitHub refused to mint an OIDC token \(403\)/);
  });

  test("fails when npm cannot install the CLI, with npm's own last words", async () => {
    const runner = fakeRunner({
      env: INPUTS,
      routes: OK,
      exec: () => ({
        status: 1,
        stdout: "",
        stderr: "npm error code E404\nnpm error 404 Not Found\n"
      })
    });
    const cause = await failure(() => setup(runner.io));
    assert.equal(cause.message, "npm could not install @oxy-hq/cli@0.6.0");
    assert.ok(cause.hints.includes("npm error 404 Not Found"));
    // Nothing was requested: no id token is spent on a job that cannot run oxyc.
    assert.deepEqual(runner.requests, []);
  });
});

describe("the inputs", () => {
  /** @param {Record<string, string>} env */
  const reading = (env) => () => readInputs(fakeRunner({ env: { ...INPUTS, ...env } }).io);

  test("refuses a version that is not a version or a dist-tag", () => {
    for (const version of ["0.6.0; id", "$(id)", "-g", "0.6.0 --registry=http://evil"]) {
      assert.throws(reading({ INPUT_VERSION: version }), /`version` is not a version/, version);
    }
    assert.equal(reading({ INPUT_VERSION: "0.6.0-rc.1" })().version, "0.6.0-rc.1");
  });

  test("takes the service account's ID, and refuses its name", () => {
    // `acme/deployer` is the form that can be re-pointed — a slug is free for
    // anyone once its org renames — so it is refused before any token is asked for.
    for (const account of [
      "acme/deployer",
      "deployer",
      `acme/${ACCOUNT_ID}`,
      `${ACCOUNT_ID}; id`,
      "3f2504e0-4f89-41d3-9a0c",
      "$(id)"
    ]) {
      assert.throws(
        reading({ "INPUT_SERVICE-ACCOUNT": account }),
        /is not a service account ID/,
        account
      );
    }
    assert.equal(reading({ "INPUT_SERVICE-ACCOUNT": ACCOUNT_ID })().serviceAccount, ACCOUNT_ID);
    assert.equal(
      reading({ "INPUT_SERVICE-ACCOUNT": ACCOUNT_ID.toUpperCase() })().serviceAccount,
      ACCOUNT_ID.toUpperCase()
    );
  });

  test("refuses a host the token should not be sent to", () => {
    assert.throws(reading({ INPUT_HOST: "app.oxygen-hq.com" }), /`host` is not a URL/);
    assert.throws(reading({ INPUT_HOST: "http://oxy.example.com" }), /must be an https URL/);
    assert.throws(reading({ INPUT_HOST: "https://user:pw@oxy.test" }), /bare base URL/);
    assert.throws(reading({ INPUT_HOST: "https://oxy.test/?next=x" }), /bare base URL/);
    // A deployment on the runner itself is the one place plain http is fine.
    assert.equal(reading({ INPUT_HOST: "http://localhost:3000/" })().host, "http://localhost:3000");
  });

  test("refuses an export-token that is not a boolean", () => {
    assert.throws(reading({ "INPUT_EXPORT-TOKEN": "yes" }), /must be true or false/);
    assert.equal(reading({ "INPUT_EXPORT-TOKEN": "FALSE" })().exportToken, false);
  });
});
