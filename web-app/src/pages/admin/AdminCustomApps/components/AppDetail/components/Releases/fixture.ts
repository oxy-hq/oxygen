import type { Release, ReleaseTimeline } from "@/types/appReleases";

/** Test fixtures shaped like the server's answer: r3 healthy, r4 red and live. */
const build = (n: number) => ({
  id: `build-${n}`,
  build_id: `pulse-r${n}-0123456789ab`,
  commit_sha: null,
  source_branch: null,
  source_repo: null,
  published_via: null
});

export const release = (over: Partial<Release> = {}): Release => ({
  index: 0,
  build: build(3),
  pruned: false,
  unpublished: false,
  current: false,
  clipped: false,
  started_at: "2026-10-18T15:45:00Z",
  counted_from: "2026-10-18T15:45:00Z",
  ended_at: "2026-10-21T09:45:00Z",
  started_by: { action: "promote", actor: "app-operator@oxy.local" },
  metrics: {
    calls: 926,
    failed: 0,
    shed: 0,
    failure_rate_bp: 0,
    route_calls: 600,
    p50_ms: 240,
    p95_ms: 380
  },
  vitals: [{ path: "/store", samples: 36, lcp_p75: 1300, inp_p75: 110, cls_p75: 30 }],
  more_screens: false,
  migrations: [],
  new_issues: [],
  previous: null,
  verdict: {
    kind: "not_comparable",
    reasons: [],
    why: "no earlier release in this window to compare with"
  },
  rollback: null,
  ...over
});

const RED = release({
  index: 1,
  build: build(4),
  current: true,
  started_at: "2026-10-21T09:45:00Z",
  counted_from: "2026-10-21T09:45:00Z",
  ended_at: null,
  metrics: {
    calls: 112,
    failed: 19,
    shed: 0,
    failure_rate_bp: 1696,
    route_calls: 80,
    p50_ms: 900,
    p95_ms: 2300
  },
  vitals: [{ path: "/store", samples: 36, lcp_p75: 2618, inp_p75: 260, cls_p75: 30 }],
  new_issues: [
    {
      function_name: "store",
      fingerprint: "abc123",
      first_seen: "2026-10-21T10:37:00Z",
      occurrences: 19
    }
  ],
  previous: 0,
  verdict: {
    kind: "worse",
    reasons: [
      {
        metric: "failure_rate",
        screen: null,
        before: 0,
        after: 1696,
        unit: "bp",
        rule: "failure rate at least 2x and 1 point higher, 50 calls each side"
      }
    ],
    why: null
  },
  rollback: { offered: true, to: build(3), why_not: null }
});

export const timeline = (over: Partial<ReleaseTimeline> = {}): ReleaseTimeline => ({
  window: { days: 7, since: "2026-10-14T17:00:00Z", until: "2026-10-21T17:00:00Z" },
  non_production: "included",
  lanes: [
    {
      lane: "production",
      events: [
        {
          id: "e4",
          environment: "production",
          action: "promote",
          at: "2026-10-21T09:45:00Z",
          build: build(4),
          pruned: false,
          actor: null,
          agent_built: false
        }
      ]
    },
    {
      lane: "staging",
      events: [
        {
          id: "e5",
          environment: "staging",
          action: "publish",
          at: "2026-10-21T14:45:00Z",
          build: build(5),
          pruned: false,
          actor: null,
          agent_built: false
        }
      ]
    },
    { lane: "sandboxes", events: [] }
  ],
  events_truncated: false,
  parts: { metrics: "ok", vitals: "ok", migrations: "ok", issues: "ok" },
  releases: [RED, release()],
  unreleased_migrations: [],
  migrations_truncated: false,
  ...over
});
