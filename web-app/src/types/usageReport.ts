/**
 * The weekly custom-app usage report, as `GET /admin/usage-report` serves it.
 *
 * The sentences (`headline`, `comparison`, each highlight's `detail`) and both orderings
 * (`highlights`, `orgs`) are written by the server, which also emails this report. The
 * console renders them as given so the page and the email cannot say different things.
 */

/** What a highlight is about. The tone each one carries is fixed server-side. */
export type UsageHighlightKind =
  | "went_quiet"
  | "dropping"
  | "failing_functions"
  | "client_errors"
  | "growing"
  // The first week anyone opened the app. Not "newly published": the stored publish
  // timestamp moves on every release, so the server cannot tell when an app first shipped.
  | "first_week"
  | "unused";

export type UsageHighlightTone = "attention" | "good" | "idle";

export interface UsageHighlight {
  kind: UsageHighlightKind;
  tone: UsageHighlightTone;
  org_id: string;
  org_name: string;
  org_slug: string;
  app_id: string;
  app_name: string;
  app_slug: string;
  /** A server-written sentence, shown verbatim. */
  detail: string;
}

export interface UsageCounts {
  views: number;
  people: number;
  sessions: number;
  error_sessions: number;
  function_calls: number;
  function_failures: number;
  /** Builds promoted to production that week. */
  releases: number;
}

export interface UsageApp {
  app_id: string;
  name: string;
  slug: string;
  published_at: string | null;
  current: UsageCounts;
  previous: UsageCounts;
  /**
   * The size of the app's stored files at the end of the week. `null` means it was never
   * measured — a different fact from `0`, which is an app measured and found empty.
   */
  storage_bytes: number | null;
  /** The same at the start of the week; `null` when it was not measured then. */
  storage_bytes_before: number | null;
}

export interface UsageOrg {
  org_id: string;
  name: string;
  slug: string;
  people: number;
  prev_people: number;
  views: number;
  prev_views: number;
  /** Sums over the organization's apps, for the week of the report. */
  function_calls: number;
  function_failures: number;
  releases: number;
  /** The sum of its apps' measured sizes; `null` when none of them was measured. */
  storage_bytes: number | null;
  apps: UsageApp[];
}

export interface UsageSummary {
  /** e.g. "42 people opened 9 custom apps across 4 organizations." */
  headline: string;
  /** e.g. "That is 7 more people than the week before." */
  comparison: string;
  people: number;
  prev_people: number;
  views: number;
  prev_views: number;
  /** Apps someone opened, out of `apps` in the report. */
  active_apps: number;
  apps: number;
  active_orgs: number;
  orgs: number;
  /** The whole report, for the week it covers. */
  function_calls: number;
  function_failures: number;
  releases: number;
  /** Every measured app's stored files, at the end of the week and at its start. */
  storage_bytes: number | null;
  storage_bytes_before: number | null;
}

export interface UsageReport {
  id: string;
  /** ISO-8601 UTC, inclusive — a Monday 00:00 UTC. */
  period_start: string;
  /** ISO-8601 UTC, exclusive — the next Monday. */
  period_end: string;
  generated_at: string;
  summary: UsageSummary;
  highlights: UsageHighlight[];
  orgs: UsageOrg[];
}

/** `report` is `null` until the first one has been written. */
export interface UsageReportResponse {
  report: UsageReport | null;
}

/**
 * How this deployment delivers the report: a real email, a browser preview only (a dev
 * box), or not at all because no sender is configured.
 */
export type UsageReportDelivery = "email" | "preview" | "off";

export interface UsageReportEmailPreference {
  /** The caller's own address. */
  email: string;
  /** `true` when the caller has never set it. */
  enabled: boolean;
  delivery: UsageReportDelivery;
}

export interface UsageReportSendResult {
  outcome: "sent" | "previewed";
  to: string;
}

/**
 * Someone the Monday report is addressed to: a global owner or a global admin.
 *
 * `role` is open-ended on purpose. The server names the two it has today, and a third
 * must not need a console release before the list can be read.
 */
export interface UsageReportRecipient {
  email: string;
  /** `global_owner` is someone listed in `OXY_OWNER`. */
  role: "global_owner" | "global_admin" | (string & {});
  /** Their grant reaches every organization. */
  scope_all: boolean;
  /** When `scope_all` is false: how many organizations the grant names. */
  org_count: number | null;
  /** Whether they are emailed the report. */
  enabled: boolean;
  /** This row is the caller. It is the same setting as their own email preference. */
  is_self: boolean;
  /** Who last changed it. `null` means nobody has: it is on by default. */
  updated_by: string | null;
  /** ISO-8601. */
  updated_at: string | null;
}

/** Server-ordered; rendered in the order given. */
export interface UsageReportRecipientsResponse {
  recipients: UsageReportRecipient[];
}
