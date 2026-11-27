/**
 * A production or staging move as the server plans it (`Plan`), and how the
 * plan reads to the person deciding whether to go ahead.
 *
 * The plan is the server's, printed verbatim with `--json`. What this adds
 * for a person is its lines: the build, what the target serves now, what
 * changes, the build's checks where the org holds promotes on them
 * (`promote-checks.ts`), and under each blocker the checks raise, what to do
 * next.
 */

import { out } from "../ui/tty.js";
import {
  type AdviceFor,
  checksAdvice,
  type PlanChecks,
  type ProductionWait,
  planCheckLines
} from "./promote-checks.js";

export interface Blocker {
  code: string;
  message: string;
}

interface Cadence {
  function: string;
  cron: string;
  timezone: string;
}

/** The server's plan, printed verbatim with `--json` (snake_case). */
export interface Plan {
  action: "promote" | "rollback";
  app_id: string;
  from: string | null;
  to: string;
  build: {
    id: string;
    build_id: string;
    commit_sha: string | null;
    source_branch: string | null;
    source_repo: string | null;
    published_by: string | null;
    created_at: string;
    published_to: string | null;
  };
  current: { id: string; build_id: string } | null;
  expect_current: string | null;
  served_since: string | null;
  served_for_seconds: number | null;
  functions: { added: string[]; removed: string[]; changed: string[] };
  schedules: {
    added: Cadence[];
    removed: Cadence[];
    changed: { function: string; from: Cadence; to: Cadence }[];
  } | null;
  migrations: { when: "before_move" | "queued_after"; oltp: string[]; airhouse: string[] };
  required_secrets_unset: string[];
  blockers: Blocker[];
  warnings: string[];
  dry_run: boolean;
  moved: boolean;
  migrations_applied: { oltp: string[]; airhouse: string[] } | null;
  /** The build's checks, where the org holds promotes on them; null or absent otherwise, and for a rollback. */
  checks?: PlanChecks | null;
}

/** What `--json` adds to the plan once production was waited for: how its check ended. */
export interface ProductionCheckResult {
  /** `passed`, `failed`, `running` (no verdict in time), `none` (none was started) or `unreadable`. */
  state: ProductionWait["outcome"];
  attempt: number | null;
}

/** What the blockers' advice names: the app as typed, the build, and whether the caller may override. */
export function adviceFor(plan: Plan, app: string): AdviceFor {
  return { app, build: plan.build.build_id, overridable: plan.checks?.overridable === true };
}

/** The plan, for a person deciding whether to go ahead. `app` is how the caller named the app. */
export function printPlan(
  plan: Plan,
  stream: { write: (chunk: string) => unknown } = process.stdout,
  app?: string
): void {
  for (const line of planLines(plan, app)) stream.write(`${line}\n`);
}

export function planLines(plan: Plan, app: string = plan.app_id): string[] {
  const b = plan.build;
  const source = plan.from ?? plan.to;
  const lines = [
    `${plan.action} ${plan.from ? `${plan.from} → ` : ""}${plan.to}: build ${b.build_id}`,
    `  ${[b.commit_sha && `commit ${b.commit_sha.slice(0, 12)}`, b.source_branch && `on ${b.source_branch}`, b.published_by && `published by ${b.published_by}`, `created ${b.created_at}`].filter(Boolean).join(" · ")}`,
    `  ${plan.to} serves ${plan.current ? plan.current.build_id : "nothing"} now`
  ];
  if (plan.served_for_seconds !== null) {
    lines.push(`  ${source} has served it for ${duration(plan.served_for_seconds)}`);
  }
  const f = plan.functions;
  lines.push(`  functions: ${changes(f.added, f.removed, f.changed)}`);
  if (plan.schedules) {
    const s = plan.schedules;
    const added = s.added.map((c) => `${c.function} (${c.cron})`);
    const removed = s.removed.map((c) => c.function);
    const changed = s.changed.map((c) => `${c.function} (${c.from.cron} → ${c.to.cron})`);
    lines.push(`  schedules: ${changes(added, removed, changed)}`);
  }
  const m = plan.migrations;
  const when = m.when === "before_move" ? "applied before the move" : "queued after the move";
  lines.push(`  migrations (${when}): ${files(m.oltp, "OLTP")}; ${files(m.airhouse, "Airhouse")}`);
  if (plan.required_secrets_unset.length > 0) {
    lines.push(`  required secrets unset in production: ${plan.required_secrets_unset.join(", ")}`);
  }
  lines.push(...planCheckLines(plan.checks));
  for (const w of plan.warnings) lines.push(`  ${out.yellow("warning")} ${w}`);
  const about = adviceFor(plan, app);
  for (const blocker of plan.blockers) {
    lines.push(`  ${out.red("blocked")} ${blocker.code}: ${blocker.message}`);
    const next = checksAdvice(blocker.code, about);
    if (next) lines.push(`    ${out.dim("next:")} ${next}`);
  }
  return lines;
}

function changes(added: string[], removed: string[], changed: string[]): string {
  const parts = [
    ...added.map((n) => `+${n}`),
    ...removed.map((n) => `-${n}`),
    ...changed.map((n) => `~${n}`)
  ];
  return parts.length > 0 ? parts.join(" ") : "no change";
}

function files(names: string[], store: string): string {
  return names.length > 0 ? `${store} ${names.join(", ")}` : `no ${store} file`;
}

function duration(seconds: number): string {
  if (seconds < 120) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 120) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  return hours < 48 ? `${hours}h ${minutes % 60}m` : `${Math.floor(hours / 24)}d ${hours % 24}h`;
}
