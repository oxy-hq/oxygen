/**
 * `oxyc bootstrap plan | run | verify` — standing a new customer up.
 *
 * The sequence this drives was tribal knowledge: nine steps across three
 * credentials with no runbook in the repository, which is the ceiling
 * `internal-docs/customer-bootstrap.md` is about. The table in
 * `../bootstrap/phases.ts` is the runbook; these three verbs are what a person
 * or an agent does with it.
 *
 *   plan     the whole sequence, with the stage boundaries and who does each
 *            step. Reads nothing, needs no credential, writes nothing.
 *   run      performs the phases `oxyc` can perform, in order, and prints the
 *            checklist for the ones it cannot. Re-runnable: it reads each
 *            phase's state first and writes only what is missing.
 *   verify   reads every phase and reports whether its fact is actually there.
 *
 * IDEMPOTENCE IS THE WHOLE VALUE. A half-finished standup is the common case —
 * somebody is interrupted after the org and before the store — so `run` is
 * read-then-write for every phase and the thing you do when you do not know
 * where you got to.
 *
 * EXIT CODES, because the caller is half agent:
 *   0  every phase this invocation looked at has landed (or was skipped)
 *   9  a phase has not landed — the same meaning `oxyc checks run` gives it:
 *      the check ran and the answer was no. Reusing it rather than minting a
 *      tenth code, since the response is the same (read the report, act).
 *   8  refused: `run` found a live assume-role session, which closes /admin
 *   4/5/6/7 as everywhere else — the credential, the org, the request, the server
 *
 * WHAT IT WILL NOT DO. It never provisions the OLTP store (a billable project
 * at a paid provider — `internal-docs/data-placement.md`), never provisions a
 * subscription (owner-only), never sets a secret's value, and never enrols a
 * worker. Those four are `manual` in the table with their reasons, and `run`
 * prints and verifies them instead.
 */

import { type Phase, type PhaseId, phasesFor, stages } from "../bootstrap/phases.js";
import { liveAssumeSessions, type Reading, readAll } from "../bootstrap/read.js";
import {
  announceStage,
  renderChecklist,
  renderPlan,
  renderVerify,
  verifyDocument
} from "../bootstrap/render.js";
import {
  type BootstrapFlags,
  type BootstrapTarget,
  parsePhases,
  parseTarget
} from "../bootstrap/target.js";
import type { Context } from "../context/resolve.js";
import * as log from "../ui/log.js";
import { out } from "../ui/tty.js";
import { CliError, ExitCode, refusal } from "../util/errors.js";
import { provisionPath } from "./airhouse.js";
import { call, confirm, PROVISION_TIMEOUT_MS } from "./oltp-client.js";

/** `POST /api/admin/orgs` — `AdminCreateOrgResponse` in `crates/api-tenancy`. */
interface CreatedOrg {
  org?: { id?: string; slug?: string; name?: string };
  owner_status?: string;
  owner_email?: string;
}

function selection(target: BootstrapTarget, flags: BootstrapFlags): Phase[] {
  return phasesFor({ crews: target.crews, only: parsePhases(flags.phase ?? []) });
}

/** The plan, and nothing else. No credential, no request. */
export function runBootstrapPlan(ctx: Context, slug: string, flags: BootstrapFlags): void {
  const target = parseTarget(slug, flags, { requireOwner: false });
  const phases = selection(target, flags);
  // `ctx.target()` can throw on an unresolvable `--env`, and a plan is worth
  // printing without one: the sequence does not depend on the deployment.
  let where = "the deployment";
  try {
    where = ctx.target();
  } catch {
    where = "an unresolved deployment (pass --env or --target to name one)";
  }
  process.stdout.write(renderPlan(target, stages(phases), where));
  log.info(
    "nothing was read and nothing was written — `oxyc bootstrap verify` reads, `run` writes"
  );
}

/** The report, and the exit code that says whether it is all there. */
export async function runBootstrapVerify(
  ctx: Context,
  slug: string,
  flags: BootstrapFlags
): Promise<void> {
  const target = parseTarget(slug, flags, { requireOwner: false });
  const phases = selection(target, flags);
  const { readings } = await readAll(ctx, target, new Set(phases.map((p) => p.id)));

  if (flags.json) {
    process.stdout.write(
      `${JSON.stringify(verifyDocument(target, phases, readings, ctx.target()), null, 2)}\n`
    );
  } else {
    process.stdout.write(renderVerify(phases, readings));
    process.stderr.write(`\n${renderChecklist(phases, readings)}`);
  }
  reportOutcome(phases, readings);
}

/**
 * Do what can be done, then say what is left.
 *
 * The admin stages and the tenant stages cannot be held in one session, so
 * this refuses up front when an assume session is live rather than performing
 * the local phases and then failing with a 403 on the first admin call.
 */
export async function runBootstrapRun(
  ctx: Context,
  slug: string,
  flags: BootstrapFlags
): Promise<void> {
  if (flags.dryRun === true) {
    runBootstrapPlan(ctx, slug, flags);
    return;
  }
  // EVERY USAGE ERROR BEFORE A REQUEST. `--owner` is needed only when `org`
  // is one of the phases; asking for it to re-run `airhouse` alone would be
  // ceremony.
  const phaseIds = parsePhases(flags.phase ?? []);
  const willCreateOrg = phaseIds.length === 0 || phaseIds.includes("org");
  const target = parseTarget(slug, flags, { requireOwner: willCreateOrg });
  const phases = selection(target, flags);
  const wanted = new Set(phases.map((p) => p.id));

  const plan = stages(phases);
  process.stderr.write(renderPlan(target, plan, ctx.target()));

  // BEFORE any read, not after. `block_admin_while_acting` answers 403 on
  // every `/api/admin/*` route during a session, so asking `readAll` first
  // meant the first admin read threw `exit 4` with a raw 403 and this refusal
  // was unreachable — the documented exit 8 never fired for the case it was
  // written for (staff who assumed into the org for `git-link`, then re-ran).
  if (phases.some((p) => p.surface === "admin")) {
    const assuming = await liveAssumeSessions(ctx);
    if (assuming > 0) {
      throw refusal(
        `there is a live assume-role session on ${ctx.target()}, and it closes /admin`,
        {
          detail:
            "the admin phases (org, billing, oltp, airhouse, app, secrets) and the tenant phases (git-link, frontline) cannot be done in one session — that is the sequence, not a limitation of this command",
          remedy: "oxyc assume end, then re-run; `oxyc bootstrap plan` shows the stage boundaries"
        }
      );
    }
  }

  const before = await readAll(ctx, target, wanted);

  for (const [index, stage] of plan.entries()) announceStage(stage, index, plan.length);

  const readings = new Map<PhaseId, Reading>(before.readings);
  // `org` first and on its own: every later read needs its id, so a run that
  // creates it has to re-read before it can say anything about the rest.
  if (wanted.has("org") && readings.get("org")?.landed === "no") {
    await createOrg(ctx, target, flags);
    const after = await readAll(ctx, target, wanted);
    for (const [id, reading] of after.readings) readings.set(id, reading);
    for (const key of Object.keys(after.facts) as Array<keyof typeof after.facts>) {
      // Narrowing by key keeps the facts object the single source; a spread
      // would silently drop a field added to `Facts` later.
      Reflect.set(before.facts, key, after.facts[key]);
    }
  }

  if (wanted.has("airhouse") && readings.get("airhouse")?.landed === "no") {
    const workspaceId = before.facts.workspaceId;
    if (workspaceId) {
      await provisionAirhouse(ctx, workspaceId, flags);
      const after = await readAll(ctx, target, new Set(["airhouse"] as PhaseId[]));
      const reading = after.readings.get("airhouse");
      if (reading) readings.set("airhouse", reading);
    }
  }

  process.stdout.write(renderVerify(phases, readings));
  process.stderr.write(`\n${renderChecklist(phases, readings)}`);
  reportOutcome(phases, readings);
}

async function createOrg(
  ctx: Context,
  target: BootstrapTarget,
  flags: BootstrapFlags
): Promise<void> {
  process.stderr.write(
    `On ${ctx.target()}:\n` +
      `  org       a NEW organization ${target.name} (${target.slug}), billing Incomplete\n` +
      `  owner     ${target.ownerEmail} — added as Owner if the account exists, otherwise sent an Owner invitation that expires in 7 days\n` +
      "  workspace none — an org gets its workspace when `git-link` connects its repository\n"
  );
  if (flags.yes !== true) {
    await confirm("Create it?", {
      verb: "creating an organization",
      why: "it creates a tenant and emails its owner",
      declined: "nothing was created — the confirmation was declined"
    });
  }
  const created = await call<CreatedOrg>(ctx, "POST", "/api/admin/orgs", {
    body: { name: target.name, slug: target.slug, owner_email: target.ownerEmail },
    refusal: orgRefusal,
    routesArea: "orgs",
    timeoutMs: PROVISION_TIMEOUT_MS
  });
  process.stdout.write(
    `${out.green(`created org ${created.org?.slug ?? target.slug}`)} (${created.org?.id ?? "no id in the response"})\n`
  );
  if (created.owner_status === "invited") {
    // Said loudly: the call succeeded and nobody can sign in yet, which is the
    // one outcome here that reads as done and is not.
    log.warn(
      `${created.owner_email ?? target.ownerEmail} has no account yet — an Owner invitation was emailed, and it expires in 7 days. Until it is accepted the org has no owner who can sign in.`
    );
  }
}

/** Why `POST /admin/orgs` refused, in the terms an operator acts on. */
function orgRefusal(status: number): string | undefined {
  switch (status) {
    case 400:
      return "the slug is reserved, or the body was rejected — a slug is lowercase words joined by single hyphens";
    case 403:
      return "creating a tenant needs the org-creation capability, which administering one does not imply — and an active `oxyc assume` session closes /admin";
    case 409:
      return "an organization with that slug already exists";
    case 500:
      return "the route is IdeOnly (it scaffolds a working copy): a deployment whose Factory pod is down cannot answer it";
    default:
      return undefined;
  }
}

async function provisionAirhouse(
  ctx: Context,
  workspaceId: string,
  flags: BootstrapFlags
): Promise<void> {
  process.stderr.write(
    `On ${ctx.target()}:\n  airhouse  provision or reconcile the tenant for workspace ${workspaceId}\n`
  );
  if (flags.yes !== true) {
    await confirm("Provision it?", {
      verb: "provisioning an Airhouse tenant",
      why: "it creates a tenant and a service account under a global name that cannot be reused",
      declined: "not provisioned — the confirmation was declined"
    });
  }
  await call<unknown>(ctx, "POST", provisionPath(workspaceId), {
    timeoutMs: PROVISION_TIMEOUT_MS,
    routesArea: "airhouse"
  });
  process.stdout.write(`${out.green("provisioned the Airhouse tenant")} for ${workspaceId}\n`);
}

/**
 * The exit code, from the readings.
 *
 * An `unknown` does not fail the run — it means the pass could not tell, and
 * failing on it would make a bounded staff grant look like a broken account —
 * but it is counted out loud, because an operator reading "0 missing" deserves
 * to know how many rows that number did not cover.
 */
function reportOutcome(phases: Phase[], readings: Map<PhaseId, Reading>): void {
  const missing = phases.filter((p) => readings.get(p.id)?.landed === "no");
  const unknown = phases.filter((p) => (readings.get(p.id)?.landed ?? "unknown") === "unknown");
  if (unknown.length > 0) {
    log.warn(
      `${unknown.length} phase(s) could not be read: ${unknown.map((p) => p.id).join(", ")}`
    );
  }
  if (missing.length === 0) {
    log.info("every phase read as landed");
    return;
  }
  throw new CliError(`${missing.length} phase(s) have not landed`, {
    code: ExitCode.CHECK_FAILED,
    detail: missing.map((p) => `  ${p.id} — ${p.title}`).join("\n"),
    remedy: "the checklist above names the step for each; re-run `oxyc bootstrap run` after"
  });
}
