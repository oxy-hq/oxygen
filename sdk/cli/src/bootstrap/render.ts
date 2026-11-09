/**
 * Four renderings of one table: the plan, the checklist, the verify report and
 * the JSON document.
 *
 * STDOUT IS THE ANSWER, stderr is the narration — the rule the rest of this
 * tool follows. So `plan` writes the plan to stdout (it IS what was asked
 * for), `verify` writes its report to stdout, and the progress lines a `run`
 * emits go to stderr through `log`.
 */

import * as log from "../ui/log.js";
import { heading, table } from "../ui/render.js";
import { out } from "../ui/tty.js";
import type { Automation, Phase, Stage, Surface } from "./phases.js";
import type { Landed, Reading } from "./read.js";
import type { BootstrapTarget } from "./target.js";

/** What a surface costs the caller, said once per stage. */
const SURFACE_NOTE: Record<Surface, string> = {
  local: "this machine and GitHub — no Oxy credential",
  admin: "Oxy staff standing on /api/admin — refused while an assume-role session is live",
  tenant: "inside the org — staff need `oxyc assume start`, which closes /admin for the session"
};

const AUTOMATION_LABEL: Record<Automation, string> = {
  automated: "oxyc",
  assisted: "oxyc + you",
  manual: "you",
  verified: "read only"
};

const LANDED_MARK: Record<Landed, string> = {
  yes: "ok",
  no: "MISSING",
  unknown: "?",
  skipped: "skip"
};

function mark(landed: Landed): string {
  if (landed === "yes") return out.green(LANDED_MARK.yes);
  if (landed === "no") return out.red(LANDED_MARK.no);
  if (landed === "unknown") return out.yellow(LANDED_MARK.unknown);
  return out.dim(LANDED_MARK.skipped);
}

function repeat(phase: Phase): string {
  if (phase.idempotent === true) return "idempotent";
  if (phase.idempotent === false) return "refuses a repeat";
  return "A REPEAT DUPLICATES";
}

/**
 * The plan: every phase, who does it, and the stage boundaries.
 *
 * Written whether or not anything will be run, because a dry run and the
 * preamble of a real run should not be able to disagree about the sequence.
 */
export function renderPlan(
  target: BootstrapTarget,
  stages: Stage[],
  deploymentTarget: string
): string {
  const lines: string[] = [
    heading(`standing up ${target.slug} on ${deploymentTarget}`),
    `  org          ${target.name} (${target.slug})${target.ownerEmail ? `, owner ${target.ownerEmail}` : ""}`,
    `  repository   ${target.repo}`,
    `  first app    ${target.slug}/${target.app}`,
    `  OLTP writers ${target.writers.join(", ")}`,
    `  crews        ${target.crews ? "yes — the frontline phase applies" : "no (pass --crews if the customer has them)"}`,
    ""
  ];
  let n = 0;
  stages.forEach((stage, index) => {
    lines.push(
      `  ${out.dim(`stage ${index + 1} · ${stage.surface} — ${SURFACE_NOTE[stage.surface]}`)}`
    );
    for (const phase of stage.phases) {
      n += 1;
      lines.push(
        `  ${String(n).padStart(2)}. ${phase.title}`,
        `      ${out.dim(`${AUTOMATION_LABEL[phase.automation]} · ${repeat(phase)}${phase.automation === "manual" ? ` · manual because ${phase.manualBecause}` : ""}`)}`,
        `      ${out.dim(phase.how)}`,
        `      ${out.dim(`who: ${phase.who}`)}`,
        `      ${out.dim(`wrong: ${phase.wrong}`)}`
      );
    }
    lines.push("");
  });
  return `${lines.join("\n")}\n`;
}

/**
 * The checklist: only what a human has to do, in order, with the command.
 *
 * SEPARATE FROM THE PLAN on purpose. The plan is the whole sequence and is
 * long; this is the part that does not happen unless somebody does it, and it
 * is the thing that gets pasted into a ticket.
 */
export function renderChecklist(phases: Phase[], readings: Map<string, Reading>): string {
  // A row earns its place here when it is NOT there, or when it needs a human
  // and this pass could not confirm it is. `automated` and `verified` rows
  // appear only when the answer was no: there is nothing for a person to do
  // about one that landed, or about one nobody performs.
  const todo = phases.filter((p) => {
    const reading = readings.get(p.id);
    if (reading?.landed === "no") return true;
    if (reading?.landed === "yes") return false;
    return p.automation === "assisted" || p.automation === "manual";
  });
  if (todo.length === 0) {
    return `${heading("for a human")}\n  ${out.green("nothing — every phase reads as landed")}\n`;
  }
  const lines = [heading("for a human")];
  todo.forEach((phase, index) => {
    const reading = readings.get(phase.id);
    lines.push(
      `  ${index + 1}. [${mark(reading?.landed ?? "unknown")}] ${phase.title}`,
      `       ${reading?.detail ?? "not read"}`
    );
    const next = reading?.next;
    if (next) for (const line of next.split("\n")) lines.push(`       ${out.dim(`→ ${line}`)}`);
  });
  return `${lines.join("\n")}\n`;
}

/** The verify report, one row per phase. */
export function renderVerify(phases: Phase[], readings: Map<string, Reading>): string {
  const rows = phases.map((p) => ({ phase: p, reading: readings.get(p.id) }));
  return `${table(rows, [
    { header: "", value: (r) => mark(r.reading?.landed ?? "unknown") },
    { header: "PHASE", value: (r) => r.phase.id },
    { header: "BY", value: (r) => AUTOMATION_LABEL[r.phase.automation] },
    { header: "WHAT THE DEPLOYMENT SAYS", value: (r) => r.reading?.detail ?? "not read" }
  ])}\n`;
}

/** The machine answer: the same facts, with nothing styled. */
export function verifyDocument(
  target: BootstrapTarget,
  phases: Phase[],
  readings: Map<string, Reading>,
  deploymentTarget: string
): unknown {
  return {
    target: deploymentTarget,
    org_slug: target.slug,
    app: `${target.slug}/${target.app}`,
    repository: target.repo,
    phases: phases.map((p) => {
      const reading = readings.get(p.id);
      return {
        id: p.id,
        title: p.title,
        surface: p.surface,
        automation: p.automation,
        idempotent: p.idempotent,
        landed: reading?.landed ?? "unknown",
        detail: reading?.detail ?? null,
        next: reading?.next ?? null
      };
    })
  };
}

/**
 * The one line that says whether a stage boundary needs a credential change.
 *
 * Emitted by `run` between stages, where it is actionable, rather than once at
 * the top where it reads as background.
 */
export function announceStage(stage: Stage, index: number, total: number): void {
  log.info(`stage ${index + 1}/${total} · ${stage.surface} — ${SURFACE_NOTE[stage.surface]}`);
}
