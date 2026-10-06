/**
 * The post step, against a faked runner and a mocked `fetch`.
 *
 * One property above the rest: IT NEVER FAILS THE JOB. Every answer the
 * deployment can give, and no answer at all, ends as a log line.
 */

import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { cleanup } from "../src/cleanup.mjs";
import { fakeRunner, HOST, MINTED, REVOKE } from "./fake-runner.mjs";

/** What the main step left behind, as the runner hands it to `post`. */
const STATE = { STATE_token: MINTED.token, STATE_host: HOST };

/** @param {string[]} lines */
const warnings = (lines) => lines.filter((line) => line.startsWith("::warning::"));

describe("the post step", () => {
  test("revokes the token the main step minted, with that token as the bearer", async () => {
    const runner = fakeRunner({ env: STATE, routes: { [REVOKE]: () => ({ status: 204 }) } });
    await cleanup(runner.io);
    assert.equal(runner.requests.length, 1);
    assert.equal(`${runner.requests[0]?.method} ${runner.requests[0]?.url}`, REVOKE);
    assert.equal(runner.requests[0]?.headers.authorization, `Bearer ${MINTED.token}`);
    assert.ok(runner.lines.includes("revoked the token minted for this job on https://oxy.test"));
    assert.deepEqual(warnings(runner.lines), []);
  });

  test("masks the token again, and never prints it", async () => {
    const runner = fakeRunner({ env: STATE, routes: { [REVOKE]: () => ({ status: 204 }) } });
    await cleanup(runner.io);
    assert.equal(runner.lines[0], `::add-mask::${MINTED.token}`);
    for (const line of runner.lines.slice(1)) assert.ok(!line.includes(MINTED.token), line);
  });

  test("does nothing when the main step minted no token", async () => {
    // A failed exchange, an older deployment, or a main step that never ran.
    const runner = fakeRunner({ routes: { [REVOKE]: () => ({ status: 204 }) } });
    await cleanup(runner.io);
    assert.deepEqual(runner.requests, []);
    assert.deepEqual(runner.lines, ["no token was minted by this job — nothing to revoke"]);
  });

  test("a token the deployment already refuses is the goal, not a warning", async () => {
    for (const status of [401, 403]) {
      const runner = fakeRunner({ env: STATE, routes: { [REVOKE]: () => ({ status }) } });
      await cleanup(runner.io);
      assert.deepEqual(warnings(runner.lines), []);
      assert.ok(runner.lines.some((line) => line.includes("no longer accepts the token")));
    }
  });

  test("a deployment with no revoke route is a warning that says the token expires anyway", async () => {
    // No route: the fake answers 404.
    const runner = fakeRunner({ env: STATE });
    await cleanup(runner.io);
    const [warned, ...rest] = warnings(runner.lines);
    assert.match(warned ?? "", /has no route to revoke a token/);
    assert.match(warned ?? "", /expires on its own/);
    assert.deepEqual(rest, []);
  });

  test("a 5xx and a dead network are warnings, never a throw", async () => {
    for (const reply of [{ status: 503 }, new TypeError("fetch failed")]) {
      const runner = fakeRunner({ env: STATE, routes: { [REVOKE]: () => reply } });
      await assert.doesNotReject(() => cleanup(runner.io));
      assert.match(warnings(runner.lines)[0] ?? "", /could not confirm the token was revoked/);
    }
  });

  test("revokes at the host the token was minted on, not at an input's", async () => {
    const elsewhere = "DELETE https://staging.oxy.test/api/auth/token";
    const runner = fakeRunner({
      env: { STATE_token: MINTED.token, STATE_host: "https://staging.oxy.test", INPUT_HOST: HOST },
      routes: { [elsewhere]: () => ({ status: 204 }) }
    });
    await cleanup(runner.io);
    assert.equal(`${runner.requests[0]?.method} ${runner.requests[0]?.url}`, elsewhere);
  });
});
