/**
 * The table subscriptions of `oxyc events ls`: each function that runs on a
 * table (`on: { table }`) with where its cursor is. A pure view of the
 * server's `tables` array, kept apart from the verbs so it needs no network.
 */

type Json = Record<string, unknown>;

const isObject = (value: unknown): value is Json =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const str = (value: unknown, fallback = "-"): string =>
  typeof value === "string" || typeof value === "number" ? String(value) : fallback;

/** A state, with the words a person should act on. */
function stateLabel(state: unknown): string {
  switch (state) {
    case "active":
      return "active";
    case "behind_horizon":
      return "BEHIND HORIZON";
    case "broken":
      return "BROKEN";
    case "paused":
      return "PAUSED";
    default:
      return str(state);
  }
}

/** The `tables` rows of an `events ls` reply; none from an older server. */
export function tableRows(payload: unknown): Json[] {
  if (!isObject(payload) || !Array.isArray(payload.tables)) return [];
  return payload.tables.filter(isObject);
}

/**
 * One line per (function, table, environment), the cursor's lag in snapshots
 * and, for a state that needs a person, why. `undefined` when no function
 * watches a table.
 */
export function tableSection(payload: unknown): string | undefined {
  const rows = tableRows(payload);
  if (rows.length === 0) return undefined;
  const header = ["TABLE", "LISTENER", "ENV", "STATE", "CURSOR", "HEAD", "LAG", "CHECKED"];
  const cells = rows.map((r) => [
    str(r.table),
    `${str(r.app)}/${str(r.function)}`,
    str(r.environment),
    stateLabel(r.state),
    str(r.snapshot),
    str(r.head),
    str(r.lag),
    str(r.checkedAt)
  ]);
  const widths = header.map((_, i) =>
    Math.max(header[i]?.length ?? 0, ...cells.map((c) => c[i]?.length ?? 0))
  );
  const line = (c: string[]) =>
    c
      .map((cell, i) => cell.padEnd(widths[i] ?? 0))
      .join("  ")
      .trimEnd();
  const out = ["Table subscriptions", line(header), ...cells.map(line)];
  for (const r of rows) {
    if (r.state !== "active" && typeof r.detail === "string" && r.detail !== "") {
      out.push(
        `  ${str(r.table)} (${str(r.app)}/${str(r.function)}, ${str(r.environment)}): ${r.detail}`
      );
    }
  }
  return out.join("\n");
}
