// Builders for the usage-report tests. Not imported by anything that ships.
//
// The defaults are the quietest report there can be: nothing counted, nothing released,
// nothing measured. A test states only the facts it is about.
import type {
  UsageApp,
  UsageCounts,
  UsageHighlight,
  UsageOrg,
  UsageReport,
  UsageSummary
} from "@/types/usageReport";

export const counts = (over: Partial<UsageCounts> = {}): UsageCounts => ({
  views: 0,
  people: 0,
  sessions: 0,
  error_sessions: 0,
  function_calls: 0,
  function_failures: 0,
  releases: 0,
  ...over
});

export const app = (over: Partial<UsageApp> = {}): UsageApp => ({
  app_id: "app-1",
  name: "Store Ops",
  slug: "store-ops",
  published_at: "2026-08-01T00:00:00Z",
  current: counts(),
  previous: counts(),
  storage_bytes: null,
  storage_bytes_before: null,
  ...over
});

export const org = (over: Partial<UsageOrg> = {}): UsageOrg => ({
  org_id: "org-1",
  name: "Rivermark",
  slug: "rivermark",
  people: 0,
  prev_people: 0,
  views: 0,
  prev_views: 0,
  function_calls: 0,
  function_failures: 0,
  releases: 0,
  storage_bytes: null,
  apps: [app()],
  ...over
});

export const highlight = (over: Partial<UsageHighlight> = {}): UsageHighlight => ({
  kind: "went_quiet",
  tone: "attention",
  org_id: "org-1",
  org_name: "Rivermark",
  org_slug: "rivermark",
  app_id: "app-1",
  app_name: "Store Ops",
  app_slug: "store-ops",
  detail: "5 people → nobody",
  ...over
});

export const summary = (over: Partial<UsageSummary> = {}): UsageSummary => ({
  headline: "42 people opened 9 custom apps across 4 organizations.",
  comparison: "That is 7 more people than the week before.",
  people: 42,
  prev_people: 35,
  views: 1318,
  prev_views: 1100,
  active_apps: 9,
  apps: 12,
  active_orgs: 4,
  orgs: 5,
  function_calls: 0,
  function_failures: 0,
  releases: 0,
  storage_bytes: null,
  storage_bytes_before: null,
  ...over
});

export const report = (over: Partial<UsageReport> = {}): UsageReport => ({
  id: "report-1",
  period_start: "2026-09-28T00:00:00Z",
  period_end: "2026-10-05T00:00:00Z",
  generated_at: "2026-10-05T06:00:00Z",
  summary: summary(),
  highlights: [],
  orgs: [org()],
  ...over
});
