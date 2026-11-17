import type { UsageApp, UsageCounts, UsageOrg } from "@/types/usageReport";
import { formatBytes, formatChange, formatCount } from "./utils";

/**
 * Which numeric columns the "By organization" table can actually answer.
 *
 * Most custom apps ship no Oxy Functions, most weeks record no page errors, many weeks
 * see no release, and storage is only measured for an app that stores files. Declared
 * outright, those columns would be `0` on every row, in the widest part of the table,
 * telling the reader nothing — and making the one week a failure does appear easy to
 * read past. So the set is derived, on the rule the fleet list already uses
 * (`AdminCustomApps/fleetColumns.ts`):
 *
 *   **A column is shown iff at least one app in the report can answer it.**
 *
 * "Can answer" looks at **both** weeks. An app that called functions last week and none
 * this week has a real zero — it stopped — and that zero stays on screen.
 *
 * What is dropped is named once, with the reason, at the foot of the table. A column
 * that vanishes in silence leaves the reader unable to tell "nothing happened" from
 * "this page does not report that".
 *
 * The name column is not here: it is a link, it is always first, and the table renders
 * it itself.
 */
export type UsageColumnId =
  | "people"
  | "change"
  | "views"
  | "functionCalls"
  | "functionFailures"
  | "errorSessions"
  | "releases"
  | "storage";

export interface UsageColumn {
  id: UsageColumnId;
  label: string;
  /**
   * What an app's row says in this column. `null` means this row has no figure — which
   * is not the same as a figure of zero, and is shown as `missing`.
   */
  app: (app: UsageApp) => string | null;
  /** What an organization's row says. Absent where the report has no org-level figure. */
  org?: (org: UsageOrg) => string | null;
  /** The words, shown muted, for a row with no figure. */
  missing?: string;
}

export interface HiddenUsageColumns {
  /** The column or columns left out, as they read at the start of a sentence. */
  label: string;
  /** Agrees with `label`: "Releases are", "Storage is". */
  verb: "is" | "are";
  /** Why, as a clause that completes "… not shown because <why>". */
  why: string;
}

/** People, the change in people, and opens: the report is about these, so they stay. */
const ALWAYS: UsageColumn[] = [
  {
    id: "people",
    label: "People",
    app: (a) => formatCount(a.current.people),
    org: (o) => formatCount(o.people)
  },
  {
    id: "change",
    label: "Change",
    app: (a) => formatChange(a.current.people, a.previous.people),
    org: (o) => formatChange(o.people, o.prev_people)
  },
  {
    id: "views",
    label: "App opens",
    app: (a) => formatCount(a.current.views),
    org: (o) => formatCount(o.views)
  }
];

const FUNCTION_COLUMNS: UsageColumn[] = [
  {
    id: "functionCalls",
    label: "Function calls",
    app: (a) => formatCount(a.current.function_calls),
    org: (o) => formatCount(o.function_calls)
  },
  {
    id: "functionFailures",
    label: "Failed calls",
    app: (a) => formatCount(a.current.function_failures),
    org: (o) => formatCount(o.function_failures)
  }
];

// No organization figure: the report carries none, and adding up the app rows would be
// this page inventing a number the email does not have.
const ERROR_COLUMN: UsageColumn = {
  id: "errorSessions",
  label: "Sessions with an error",
  app: (a) => formatCount(a.current.error_sessions)
};

const RELEASES_COLUMN: UsageColumn = {
  id: "releases",
  label: "Releases",
  app: (a) => formatCount(a.current.releases),
  org: (o) => formatCount(o.releases)
};

/** Size at the end of the week. An app nobody measured says so; it does not say `0 B`. */
const STORAGE_COLUMN: UsageColumn = {
  id: "storage",
  label: "Storage",
  app: (a) => (a.storage_bytes === null ? null : formatBytes(a.storage_bytes)),
  org: (o) => (o.storage_bytes === null ? null : formatBytes(o.storage_bytes)),
  missing: "not measured"
};

/** Does any app in the report have `pick(counts) > 0` in either week? */
const anyApp = (orgs: readonly UsageOrg[], pick: (counts: UsageCounts) => number): boolean =>
  orgs.some((org) => org.apps.some((app) => pick(app.current) > 0 || pick(app.previous) > 0));

/**
 * Was any app's storage measured, at either end of the week?
 *
 * A measurement, not a size above zero: an app measured and found empty has answered
 * the question, and its `0 B` is worth a column. Only "never measured, anywhere" is not.
 */
const anyMeasured = (orgs: readonly UsageOrg[]): boolean =>
  orgs.some((org) =>
    org.apps.some((app) => app.storage_bytes !== null || app.storage_bytes_before !== null)
  );

export function usageColumns(orgs: readonly UsageOrg[]): {
  shown: UsageColumn[];
  hidden: HiddenUsageColumns[];
} {
  const shown: UsageColumn[] = [...ALWAYS];
  const hidden: HiddenUsageColumns[] = [];

  // The two function columns travel together: "failed calls" is a share of "function
  // calls" and means nothing beside a missing denominator. A failure is counted as a call
  // here too, so a failure can never be the thing that gets a column hidden.
  if (anyApp(orgs, (c) => c.function_calls + c.function_failures)) {
    shown.push(...FUNCTION_COLUMNS);
  } else {
    hidden.push({
      label: "Function calls and failed calls",
      verb: "are",
      why: "no app called a function in either week"
    });
  }

  if (anyApp(orgs, (c) => c.error_sessions)) {
    shown.push(ERROR_COLUMN);
  } else {
    hidden.push({
      label: "Sessions with an error",
      verb: "are",
      why: "no app recorded one in either week"
    });
  }

  if (anyApp(orgs, (c) => c.releases)) {
    shown.push(RELEASES_COLUMN);
  } else {
    hidden.push({
      label: "Releases",
      verb: "are",
      why: "no app had one in either week"
    });
  }

  if (anyMeasured(orgs)) {
    shown.push(STORAGE_COLUMN);
  } else {
    hidden.push({
      label: "Storage",
      verb: "is",
      why: "it has not been measured for any app"
    });
  }

  return { shown, hidden };
}
