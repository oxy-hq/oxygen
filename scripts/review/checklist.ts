// The incident checklist: questions past incidents taught, each asked only of a diff
// whose shape calls for it. internal-docs/code-review.md ("The incident checklist")
// says how an item earns its place and how it leaves.
//
// An item is a trigger readable from the diff (matched like a lens: `paths` against
// changed files, `patch` against added lines in `patchPaths`), a `check` the general pass
// answers, and the `source` it came from. Triggered items go into the general pass's
// prompt, at most MAX_CHECKLIST of them, in this order: nobody has to remember the
// outage, and no extra pass runs. A finding an item produces cites `checklist:<id>` as its
// rule, so the weekly report shows which items earn their place.
//
// Add an item when an escaped bug or an incident has a shape a diff shows; remove one
// whose trigger keeps firing on PRs it never finds anything in.

import type { ChecklistItem } from "./src/types.ts";

const TESTS = [
  "**/tests/**",
  "**/test/**",
  "**/*_test.rs",
  "**/*_tests.rs",
  "**/*.test.*",
  "**/fixtures*"
];

export const CHECKLIST: readonly ChecklistItem[] = [
  {
    id: "persisted-shape-compat",
    patch: /#\[serde\(|\bderive\([^)]*\bDeserialize\b/,
    patchPaths: ["crates/**"],
    check:
      "A type this diff changes is read where another binary also reads or wrote it: workspace YAML, a JSON column, a queued TaskSpec, a cache entry, a wire payload. Does what the previous release wrote still parse (a new field `Option` or `#[serde(default)]`, a widened type still accepting the old shape), and does the previous binary, still running mid-deploy or after a revert, parse what this one writes?",
    source: "#3560: `group_by` became a list, and a running older binary could not parse it"
  },
  {
    id: "clickhouse-statement-tail",
    patch: /\bINSERT\s+INTO\b|sqlcommenter/i,
    patchPaths: ["crates/**", "sdk/**"],
    check:
      "Is anything appended after a statement that can reach ClickHouse: a comment, an audit or sqlcommenter trailer, an `ON CONFLICT`? ClickHouse reads everything after `VALUES` or `FORMAT` as row data and rejects the insert whole (`Code: 27`); decoration goes in front.",
    source: "#3158: the custom-app audit trailer broke every ClickHouse INSERT"
  },
  {
    id: "ducklake-ddl",
    patch: /\bCREATE\s+(UNIQUE\s+)?INDEX\b|\bPRIMARY\s+KEY\b|\bUNIQUE\b/i,
    patchPaths: [
      "crates/airhouse/**",
      "crates/airform/**",
      "**/airhouse/**",
      "**/*airhouse*",
      "**/*ducklake*"
    ],
    check:
      "Is this DDL bound for Airhouse (DuckLake)? DuckLake has no indexes or constraints: one `CREATE INDEX`, `PRIMARY KEY` or `UNIQUE` makes the writer go inert. Postgres, Oxy's own or an org's OLTP store, is fine.",
    source:
      "#3958: the DuckDB manifest dropped its PRIMARY KEY; product-context.md, DuckLake has no indexes"
  },
  {
    id: "stamp-before-fatal-work",
    patch: /\blast_\w+_at\b|\bnext_\w+_at\b|\battempted_at\b/,
    patchPaths: ["crates/**"],
    check:
      "When periodic or retried work can take the process down (native connector code, a probe, a customer query), is the attempt recorded before the work runs? Stamping only on success turns one fatal input into a crash on every restart.",
    source:
      "#2916: a MotherDuck probe SIGSEGV'd before `last_smoke_at` was written, a two-week crash loop"
  },
  {
    id: "duckdb-open",
    patch: /\bduckdb::|\bConnection::open(_in_memory)?\(|\bDuckDBPool\b/,
    patchPaths: ["crates/**"],
    check:
      "Does this diff open a DuckDB database outside the pool, or from tasks that can run at once? Two handles initialising one file concurrently have SIGSEGV'd; the pool serialises init.",
    source: "#2252: the DuckDBPool concurrent-init race; product-context.md, DuckDB concurrent init"
  },
  {
    id: "per-request-scan",
    patch: /\bLOWER\(|\blower\(|\bILIKE\b|Expr::cust|::find\(\)\s*\.all\(/,
    patchPaths: ["crates/server/src/**", "crates/api-*/**"],
    check:
      "Does a request path now run a query whose cost grows with tenant data: no indexed filter, a function around an indexed column (`LOWER(email)`) with no matching expression index, an unfiltered `find().all()`, or a query per item in a loop?",
    source:
      "#3539: an email lookup lowered both sides with no index, a full scan per sign-in (fixed in #3541)"
  },
  {
    id: "oauth-refresh-rotation",
    patch: /\brefresh_token\b|\brefreshToken\b/,
    patchPaths: ["crates/**", "sdk/**"],
    check:
      "Does this add a second place that exchanges or rotates an OAuth refresh token? A provider that voids the old token on rotation (QuickBooks) deadlocks two rotators into `invalid_grant`. One component rotates; the rest read.",
    source: "product-context.md: only one component may rotate a QuickBooks refresh token"
  },
  {
    id: "abort-leaves-loading",
    patch: /\bAbortError\b|\bAbortController\b|\bsignal\.aborted\b/,
    patchPaths: ["sdk/**", "web-app/src/**"],
    check:
      "Does every way out of this async state settle it: success, error, unmount, and an abort this code did not start? Ignoring any `AbortError` by name left a hook on `loading: true` for the life of the page when a timeout or a dropped socket aborted the fetch.",
    source: "#3274: an abort the SDK hook did not cause left it loading forever"
  },
  {
    id: "clock-in-tests",
    patch: /\bUtc::now\(\)|\bSystemTime::now\(\)|\bDate\.now\(\)|\bnew Date\(\)/,
    patchPaths: TESTS,
    check:
      "Does a test build a fixture from the current time and compare it after a store round trip, or render code that reads the clock itself? macOS reads microseconds and Linux CI nanoseconds, so it passes locally and fails in CI, and code reading the clock flakes by a millisecond. Truncate to what the store keeps, or pin the clock.",
    source: "#3816: usage fixtures at nanosecond precision failed only in CI"
  },
  {
    id: "review-state-contract",
    paths: ["scripts/review/src/state.ts", "scripts/review/src/types.ts"],
    check:
      "This diff changes the review state's types or encoding. Every open PR's summary comment holds a state block the previous reviewer wrote: does it still decode, and is `STATE_VERSION` bumped where a field changed meaning?",
    source: "internal-docs/code-review.md: the review state is a contract"
  }
];
