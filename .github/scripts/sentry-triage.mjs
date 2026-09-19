// Pull the week's unresolved Sentry issues for one environment and render a brief
// an agent can act on. Deliberately a script and not a set of `curl`s in the
// agent's own hands: the token stays in this process, the ranking is reviewable
// in a diff, and two runs a week apart differ only where production differed.
//
// Four things about THIS org's data shape are load-bearing, each found by
// querying it rather than assumed:
//
//   * `level` does not rank severity here. The highest-count `error` in the
//     oxy project is a compile-boundary "retry shortly" that resolves itself,
//     while `cameras ingest … The rows are lost` — an actual data-loss bug —
//     is logged at `info`. Filtering to `level:error` would drop the only
//     finding on the page that matters, so we rank by event count and flag
//     severity from the TEXT (see SEVERITY_MARKERS).
//   * `GET /issues/{id}/events/latest/` ignores the environment the issue list
//     was filtered by. Ask it for an issue you selected under
//     `environment=production` and it will happily hand back a staging event
//     with a different release and a different stack. The `?environment=`
//     below is what keeps the brief talking about production.
//   * Almost no event carries an `exception` entry. These are `tracing::error!`
//     calls captured by sentry-tracing, not panics, so there is no stack trace
//     to walk. The actionable location is `contexts["Rust Tracing Location"]`
//     (a repo-relative file + line) and the real error text is
//     `contexts["Rust Tracing Fields"].error` — the title is just the log
//     message and is usually too generic to act on ("agentic task failed").
//   * One underlying fault arrives as many issues. "No API key found in
//     headers" is seven separate issues across three levels and four culprits.
//     Without grouping, a fixed-size top-N is seven slots of the same thing.
//
// Usage:
//   SENTRY_AUTH_TOKEN=… node .github/scripts/sentry-triage.mjs \
//     [--org oxygen-intelligence] [--environment production] \
//     [--stats-period 7d] [--top 8] [--out .sentry-triage]
//
// Writes <out>/brief.md (for the agent and the issue body) and <out>/brief.json
// (for the workflow's own gating). Exit code is 0 with an empty candidate list
// when production was quiet — "nothing to fix" is a success, not a failure.

import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";

const API = "https://sentry.io/api/0";

// Expected client-side behaviour that reaches us as an event. These are NOT
// dropped — a caller that stops sending its API key is a real incident when it
// is our own service — they are moved out of the fix-candidate list into a
// counted table, so a 10x spike is still on the page while a steady background
// hum never occupies a slot an actual defect could hold.
const EXPECTED_CLIENT_ERRORS = [
  {
    pattern: /no api key found in headers/,
    why: "caller sent no X-API-Key — an unauthenticated request, not a defect in our code",
  },
  {
    pattern: /jwt validation failed: expiredsignature/,
    why: "an expired token the client is expected to refresh",
  },
  {
    pattern: /authentication failed: no api key/,
    why: "same unauthenticated caller, surfaced from a second layer",
  },
];

// Words that outrank raw volume. A fault that loses rows or panics is worth a
// slot at 5 events; a retryable one is not at 500.
const SEVERITY_MARKERS = [
  { pattern: /\b(lost|loses|losing|discard(?:ed|ing)?|dropp(?:ed|ing)|data loss)\b/, label: "possible data loss" },
  { pattern: /\b(panic(?:ked|s)?|unwrap|index out of bounds|overflow)\b/, label: "panic" },
  { pattern: /\bcorrupt(?:ed|ion)?\b/, label: "corruption" },
  { pattern: /\b(deadlock|leaked?|exhaust(?:ed|ion))\b/, label: "resource exhaustion" },
];

function parseArgs(argv) {
  const out = {
    org: process.env.SENTRY_ORG || "oxygen-intelligence",
    environment: process.env.SENTRY_ENVIRONMENT || "production",
    statsPeriod: process.env.SENTRY_STATS_PERIOD || "7d",
    top: Number(process.env.SENTRY_TOP || 8),
    out: process.env.SENTRY_OUT || ".sentry-triage",
  };
  for (let i = 0; i < argv.length; i += 2) {
    const key = argv[i]?.replace(/^--/, "");
    const value = argv[i + 1];
    if (key === undefined || value === undefined) continue;
    if (key === "org") out.org = value;
    else if (key === "environment") out.environment = value;
    else if (key === "stats-period") out.statsPeriod = value;
    else if (key === "top") out.top = Number(value);
    else if (key === "out") out.out = value;
  }
  return out;
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// Retried because both failure modes here are transient and neither deserves to
// lose a weekly run: Sentry rate-limits a burst of per-issue event fetches with
// a 429, and the connection itself can drop mid-response (undici surfaces that
// as a bare `TypeError: terminated`, with no status to branch on). A 4xx that
// is not 429 is a real answer — a missing scope, a nonexistent environment — so
// it fails immediately rather than burning three attempts to say the same thing.
async function api(token, path, params = {}, attempt = 1) {
  const MAX_ATTEMPTS = 4;
  const url = new URL(API + path);
  for (const [k, v] of Object.entries(params)) {
    if (v !== undefined && v !== null) url.searchParams.append(k, String(v));
  }

  let res;
  try {
    res = await fetch(url, { headers: { Authorization: `Bearer ${token}` } });
  } catch (err) {
    if (attempt >= MAX_ATTEMPTS) throw new Error(`GET ${url.pathname} failed after ${attempt} attempts: ${err.message}`);
    await sleep(500 * 2 ** (attempt - 1));
    return api(token, path, params, attempt + 1);
  }

  if (res.status === 429 && attempt < MAX_ATTEMPTS) {
    const retryAfter = Number(res.headers.get("retry-after")) || 2 ** attempt;
    await sleep(retryAfter * 1000);
    return api(token, path, params, attempt + 1);
  }
  if (!res.ok) {
    // The body carries Sentry's own reason (a scope the token lacks, an
    // environment that does not exist on that project). Losing it turns every
    // failure into an indistinguishable "request failed".
    const body = await res.text().catch(() => "");
    throw new Error(`GET ${url.pathname} → ${res.status} ${res.statusText}: ${body.slice(0, 300)}`);
  }
  return res.json();
}

// Anything that varies per occurrence has to go, or every occurrence is its own
// group. Order matters: UUIDs before the bare-number rule, which would otherwise
// chew their digit runs apart.
function normalizeTitle(title) {
  return String(title || "")
    .replace(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/gi, "<uuid>")
    .replace(/\b[0-9a-f]{16,}\b/gi, "<hex>")
    .replace(/'[^']*'/g, "'<s>'")
    .replace(/"[^"]*"/g, '"<s>"')
    .replace(/\b\d+\b/g, "<n>")
    .replace(/\s+/g, " ")
    .trim()
    .toLowerCase();
}

// This brief becomes a GitHub issue and gets pasted into Slack, so it leaves the
// blast radius Sentry itself has. Three classes have to come off on the way out,
// and all three were observed in a real production run, not imagined:
//
//   * Customer SQL. `data.rs` logs the failing statement, so a connector blip
//     puts a customer's predicates and schema names (`netsuite.…`, a literal
//     SKU) in the brief. The codebase already holds this line — sentry_config.rs
//     pins `traces_sample_rate(0.0)` specifically so span fields like `oxy.sql`
//     "must not leave the process" — and an issue tracker is further out than a
//     transaction, not nearer. Which statement it was never helps here anyway:
//     the fault is that the connection closed.
//   * Ephemeral warehouse credentials. Airhouse mints a per-connection user and
//     the connector's error string carries it (`… as eph_d1zGKRkMGk2f: …`).
//     Short-lived is not the same as publishable.
//   * Addresses. Internal ids stay — they are how you find the run.
//
// Anything else is capped rather than cut, so one enormous field cannot push the
// findings off the bottom of the issue.
const MAX_FIELD_CHARS = 240;
const SQL_BEARING_FIELD = /^(sql|query|statement|sql_text|body)$/i;

function redact(text) {
  return String(text ?? "")
    .replace(/[\w.+-]+@[\w-]+\.[\w.-]+/g, "<redacted-email>")
    .replace(/\beph_[A-Za-z0-9]{6,}/g, "<redacted-credential>");
}

function redactField(key, value) {
  const text = redact(value);
  if (SQL_BEARING_FIELD.test(key)) return `<redacted sql, ${text.length} chars>`;
  return text.length > MAX_FIELD_CHARS
    ? `${text.slice(0, MAX_FIELD_CHARS)}… <truncated, ${text.length} chars>`
    : text;
}

function severityOf(text) {
  return SEVERITY_MARKERS.filter((m) => m.pattern.test(text.toLowerCase())).map((m) => m.label);
}

function expectedClientError(normalized) {
  return EXPECTED_CLIENT_ERRORS.find((n) => n.pattern.test(normalized));
}

function groupIssues(issues) {
  const groups = new Map();
  for (const issue of issues) {
    const key = normalizeTitle(issue.title);
    let group = groups.get(key);
    if (!group) {
      group = { key, title: issue.title, members: [], events: 0, users: 0 };
      groups.set(key, group);
    }
    group.members.push(issue);
    group.events += Number(issue.count || 0);
    group.users += Number(issue.userCount || 0);
    // Show the variant that happens most, and prefer a member that has a
    // culprit — that is the one whose event is most likely to carry a location.
    const better =
      Number(issue.count || 0) > Number(group.lead?.count || 0) ||
      (!group.lead?.culprit && issue.culprit);
    if (!group.lead || better) {
      group.lead = issue;
      group.title = issue.title;
    }
  }
  return [...groups.values()];
}

// The part the agent actually needs: where in OUR tree this fires and what the
// error text really said.
async function fetchDetail(token, issue, environment) {
  let event;
  try {
    event = await api(token, `/issues/${issue.id}/events/latest/`, { environment });
  } catch (err) {
    return { unavailable: `no ${environment} event could be fetched: ${err.message}` };
  }
  const contexts = event.contexts || {};
  const location = contexts["Rust Tracing Location"];
  const fields = contexts["Rust Tracing Fields"];
  const tag = (key) => event.tags?.find((t) => t.key === key)?.value;

  // Present when something actually threw — browser errors out of oxy-web, and
  // Rust panics. Keep only in-app frames; the tokio/axum scaffolding around
  // them is noise that sentry_config.rs already marks as out-of-app.
  const exception = (event.entries || []).find((e) => e.type === "exception");
  const frames = (exception?.data?.values || [])
    .flatMap((v) => v.stacktrace?.frames || [])
    .filter((f) => f.inApp)
    .slice(-8)
    .map((f) => ({
      file: f.filename || f.absPath || null,
      line: f.lineNo ?? null,
      fn: f.function || null,
    }));

  return {
    eventId: event.id,
    release: tag("release") || null,
    environment: tag("environment") || null,
    serverName: tag("server_name") || null,
    level: tag("level") || issue.level || null,
    // The one field worth reading before anything else — the title is the log
    // message, this is the error.
    error: fields?.error ? redactField("error", fields.error) : null,
    fields: fields
      ? Object.fromEntries(
          Object.entries(fields)
            .filter(([k, v]) => k !== "type" && k !== "error" && v !== "" && v != null)
            .map(([k, v]) => [k, redactField(k, v)]),
        )
      : null,
    location: location
      ? { file: location.file, line: location.line, module: location.module_path }
      : null,
    frames,
  };
}

function renderMarkdown(report) {
  const L = [];
  L.push(`# Sentry triage — \`${report.environment}\`, last ${report.statsPeriod}`);
  L.push("");
  L.push(
    `Org \`${report.org}\` · projects scanned: ${
      report.projectsScanned.map((p) => `\`${p}\``).join(", ") || "_none_"
    }` + (report.projectsSkipped.length ? ` · skipped (no \`${report.environment}\` environment): ${report.projectsSkipped.map((p) => `\`${p}\``).join(", ")}` : ""),
  );
  L.push("");
  L.push(
    `${report.totalIssues} unresolved issues collapsed into ${report.totalGroups} distinct faults. ` +
      `${report.candidates.length} triaged below; ${report.knownNoise.length} held back as expected client behaviour.`,
  );
  L.push("");

  if (!report.candidates.length) {
    L.push("**No fix candidates this run.** Everything unresolved was expected client behaviour.");
    L.push("");
  }

  report.candidates.forEach((c, i) => {
    const d = c.detail || {};
    L.push(`## ${i + 1}. ${c.title}`);
    L.push("");
    const bits = [
      `**${c.events}** events`,
      c.users ? `${c.users} users` : null,
      `level \`${d.level || "?"}\``,
      d.release ? `release \`${d.release}\`` : null,
    ].filter(Boolean);
    L.push(bits.join(" · "));
    if (c.severity.length) L.push("");
    if (c.severity.length) L.push(`> ⚠️ **${c.severity.join(", ")}** — flagged from the message text, outranks event count.`);
    L.push("");
    if (d.error) {
      L.push("**Error**");
      L.push("");
      L.push("```");
      L.push(d.error);
      L.push("```");
      L.push("");
    }
    if (d.location) {
      L.push(`**Emitted at** \`${d.location.file}:${d.location.line}\` (\`${d.location.module}\`)`);
      L.push("");
    }
    if (d.frames?.length) {
      L.push("**In-app frames**");
      L.push("");
      L.push("```");
      for (const f of d.frames) L.push(`${f.file}:${f.line ?? "?"}  ${f.fn ?? ""}`.trimEnd());
      L.push("```");
      L.push("");
    }
    if (d.fields && Object.keys(d.fields).length) {
      L.push(`**Fields** ${Object.entries(d.fields).map(([k, v]) => `\`${k}=${v}\``).join(" ")}`);
      L.push("");
    }
    if (d.unavailable) {
      L.push(`_${d.unavailable}_`);
      L.push("");
    }
    L.push(
      `**Sentry** ${c.members.map((m) => `[${m.shortId}](${m.permalink})`).join(", ")}` +
        (d.serverName ? ` · last seen on \`${d.serverName}\`` : ""),
    );
    L.push("");
  });

  if (report.knownNoise.length) {
    L.push("## Held back — expected client behaviour");
    L.push("");
    L.push("Not defects in our code. Listed with counts so a spike is still visible.");
    L.push("");
    L.push("| Events | Fault | Why it is held back |");
    L.push("| -----: | ----- | ------------------- |");
    for (const n of report.knownNoise) {
      L.push(`| ${n.events} | ${n.title.replace(/\|/g, "\\|")} | ${n.why} |`);
    }
    L.push("");
  }
  return L.join("\n");
}

async function main() {
  const token = process.env.SENTRY_AUTH_TOKEN;
  if (!token) {
    console.error("SENTRY_AUTH_TOKEN is not set.");
    process.exitCode = 1;
    return;
  }
  const opts = parseArgs(process.argv.slice(2));

  const projects = await api(token, `/organizations/${opts.org}/projects/`);
  // A project is in scope when Sentry has actually seen the target environment
  // on it. That is what keeps this from hard-coding `oxy`: the day oxy-web
  // reports its first production event it joins the scan on its own. Both
  // lists are printed, because a scope that silently narrows to nothing looks
  // exactly like a quiet week.
  const inScope = projects.filter((p) => (p.environments || []).includes(opts.environment));
  const skipped = projects.filter((p) => !(p.environments || []).includes(opts.environment));
  console.error(
    `scanning ${inScope.length}/${projects.length} projects for environment=${opts.environment}: ` +
      `[${inScope.map((p) => p.slug).join(", ")}] skipped [${skipped.map((p) => p.slug).join(", ")}]`,
  );

  const issues = [];
  for (const project of inScope) {
    const batch = await api(token, `/organizations/${opts.org}/issues/`, {
      query: "is:unresolved",
      statsPeriod: opts.statsPeriod,
      environment: opts.environment,
      project: project.id,
      limit: 100,
    });
    for (const issue of batch) issues.push({ ...issue, projectSlug: project.slug });
  }

  const groups = groupIssues(issues);
  const knownNoise = [];
  const candidates = [];
  for (const g of groups) {
    const expected = expectedClientError(g.key);
    if (expected) {
      knownNoise.push({ title: g.title, events: g.events, why: expected.why });
      continue;
    }
    candidates.push({ ...g, severity: severityOf(`${g.title} ${g.lead?.culprit || ""}`) });
  }

  knownNoise.sort((a, b) => b.events - a.events);
  // Severity first, volume second — the ordering the data argued for.
  candidates.sort((a, b) => b.severity.length - a.severity.length || b.events - a.events);
  const top = candidates.slice(0, opts.top);

  for (const c of top) c.detail = await fetchDetail(token, c.lead, opts.environment);

  const report = {
    generatedAt: new Date().toISOString(),
    org: opts.org,
    environment: opts.environment,
    statsPeriod: opts.statsPeriod,
    projectsScanned: inScope.map((p) => p.slug),
    projectsSkipped: skipped.map((p) => p.slug),
    totalIssues: issues.length,
    totalGroups: groups.length,
    knownNoise,
    candidates: top.map((c) => ({
      title: c.title,
      events: c.events,
      users: c.users,
      severity: c.severity,
      projectSlug: c.lead.projectSlug,
      members: c.members.map((m) => ({ shortId: m.shortId, permalink: m.permalink, count: Number(m.count || 0) })),
      detail: c.detail,
    })),
  };

  await mkdir(opts.out, { recursive: true });
  await writeFile(join(opts.out, "brief.json"), JSON.stringify(report, null, 2));
  await writeFile(join(opts.out, "brief.md"), renderMarkdown(report));
  console.error(
    `wrote ${opts.out}/brief.md — ${report.candidates.length} candidates, ${knownNoise.length} held back`,
  );
}

// `process.exitCode`, never `process.exit()`: stderr is a pipe under the Actions
// runner and an immediate exit can truncate the very line explaining the failure.
main().catch((err) => {
  console.error(err.stack || String(err));
  process.exitCode = 1;
});
