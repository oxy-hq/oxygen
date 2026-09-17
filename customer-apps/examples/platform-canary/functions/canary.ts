// canary — the platform canary's scheduled check.
//
// Every five minutes, and on `oxyc checks run`, it exercises each host API that
// live custom apps depend on, inside an Oxy-owned org no customer reads. The
// first step that breaks throws `canary step <name> failed: <cause>`, so the
// run is an ordinary failed invocation: it logs, marks its span `ERROR`, and
// pages ops through the function-failure alert under that step's fingerprint.
// The last step checks in with All Quiet, whose silence pages when runs stop.
//
// Two app secrets configure it:
//   CANARY_STEPS        comma list of steps to run; absent means all
//   CANARY_CHECKIN_URL  the All Quiet cron check-in URL; required when check_in runs
//
// The steps themselves live in steps.ts. README.md has the step table and the
// per-deployment setup.

import type { OxyFunctionContext, OxyFunctionRequest } from "@oxy-hq/sdk";
import { runCanary, selectSteps } from "./steps";

/** An id for this run. The isolate has no `crypto`, and this only has to be unique. */
function newRunId(): string {
  return `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 10)}`;
}

export default async function canary(
  _req: OxyFunctionRequest,
  ctx: OxyFunctionContext
): Promise<Response> {
  // No try/catch, on purpose. A thrown error is what marks the run `failed` and
  // what the pager fingerprints; a 500 carrying `ok: false` would hide the step.
  const result = await runCanary(ctx, {
    runId: newRunId(),
    steps: selectSteps(ctx.env.CANARY_STEPS),
    checkinUrl: ctx.env.CANARY_CHECKIN_URL
  });
  return Response.json(result);
}
