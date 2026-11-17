import { PRODUCTION } from "@/types/appEnvironments";
import type { AppInvocation, FunctionInvocation } from "@/types/apps";
import { STAGING } from "../../AppDetail/components/Secrets/environmentLabel";

/**
 * The pure half of the Checks panel: which environments a check's last run is
 * shown for, which run that is, and what it says.
 */

/** Production and staging always; the console's environment as well when it is a
 *  sandbox, since that is where "Run checks" would run them. */
export function checkEnvironments(selected: string): string[] {
  const base = [PRODUCTION, STAGING];
  return base.includes(selected) ? base : [...base, selected];
}

/** The newest run of each of `environments` among `rows`, by `created_at`; null
 *  for an environment with none. Rows of any other environment are ignored. */
export function latestPerEnvironment(
  rows: readonly AppInvocation[],
  environments: readonly string[]
): Map<string, AppInvocation | null> {
  const latest = new Map<string, AppInvocation | null>(environments.map((env) => [env, null]));
  for (const iv of rows) {
    if (!latest.has(iv.environment)) continue;
    const held = latest.get(iv.environment);
    if (!held || Date.parse(iv.created_at) > Date.parse(held.created_at)) {
      latest.set(iv.environment, iv);
    }
  }
  return latest;
}

/** What a check's latest run says. Only `failed` is coloured. */
export type CheckVerdict = "failed" | "passed" | "running" | "other" | "never";

export function checkVerdict(
  iv: Pick<FunctionInvocation, "status" | "failed"> | null
): CheckVerdict {
  if (!iv) return "never";
  // The platform's rule, not the status word: a `success` that answered 5xx or
  // caught a failed ctx call failed.
  if (iv.failed) return "failed";
  if (iv.status === "running") return "running";
  if (iv.status === "success") return "passed";
  // Cancelled or shed: it did not get to say.
  return "other";
}

/** One environment's cell for one check. */
export type CheckCell =
  | { environment: string; state: "loading" }
  /** No read of that environment: a 403, or the read failed. */
  | { environment: string; state: "unreadable"; forbidden: boolean }
  | {
      environment: string;
      state: "read";
      latest: AppInvocation | null;
      verdict: CheckVerdict;
    };

export interface CheckRow {
  name: string;
  cells: CheckCell[];
}

/** What one `(check, environment)` read answered. */
export interface CellRead {
  rows?: readonly AppInvocation[];
  pending: boolean;
  failed: boolean;
  forbidden: boolean;
}

/** Every check against every environment, from the reads `read` returns. */
export function checkMatrix(
  checks: readonly string[],
  environments: readonly string[],
  read: (check: string, environment: string) => CellRead
): CheckRow[] {
  return checks.map((name) => ({
    name,
    cells: environments.map((environment): CheckCell => {
      const r = read(name, environment);
      if (r.rows) {
        const latest = latestPerEnvironment(r.rows, [environment]).get(environment) ?? null;
        return { environment, state: "read", latest, verdict: checkVerdict(latest) };
      }
      if (r.failed) return { environment, state: "unreadable", forbidden: r.forbidden };
      return { environment, state: "loading" };
    })
  }));
}

/** The checks whose latest run in `environment` failed. */
export function failingIn(matrix: readonly CheckRow[], environment: string): string[] {
  return matrix
    .filter((row) =>
      row.cells.some(
        (c) => c.environment === environment && c.state === "read" && c.verdict === "failed"
      )
    )
    .map((row) => row.name);
}
