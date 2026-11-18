/**
 * `oxyc events` — the engineer's side of app events
 * (`internal-docs/2026-11-17-app-events-design.md` §6.8): what is published,
 * who listens and how healthy each listener is, where one event went, and the
 * tools to redeliver, replay, discard and purge.
 *
 * Every route is staff-only and lives under `/api/customer-apps/{org}/…`:
 *
 * - org-wide: `events/types`, `events/{id}`, `events?root=`, `events/find`,
 *   the redelivery / replay / discard / purge posts;
 * - per app: `{org}/{app}/events/feed` and `…/events/dead`, and the switch
 *   `{org}/{app}/environments/{env}/events`.
 *
 * The org comes from `--org` (or the one a pasted `--env` URL named); an app
 * argument is `<org>/<app>`. Replies are printed verbatim with `--json`.
 */

import { parseJson, request } from "../api/request.js";
import { parseAppEnv } from "../apps/environment.js";
import { CUSTOMER_APPS_API, ensureOk, resolveAppSlugs, staffCreds } from "../apps/resolve.js";
import type { Context } from "../context/resolve.js";
import { usageError } from "../util/errors.js";
import { confirm } from "./oltp-client.js";

type Json = Record<string, unknown>;

// ── plumbing ─────────────────────────────────────────────────────────────────

const isObject = (value: unknown): value is Json =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/** The first of `names` the object has, so a snake_case reply reads like a camelCase one. */
function field(row: unknown, ...names: string[]): unknown {
  if (!isObject(row)) return undefined;
  for (const name of names) if (row[name] !== undefined && row[name] !== null) return row[name];
  return undefined;
}

const text = (value: unknown, fallback = "-"): string =>
  typeof value === "string" || typeof value === "number" ? String(value) : fallback;

const list = (value: unknown): unknown[] => (Array.isArray(value) ? value : []);

function orgOf(ctx: Context, explicit?: string): string {
  const org = explicit ?? ctx.flags.org ?? ctx.env().orgSlug;
  if (!org) {
    throw usageError(
      "which org? app events are listed per org",
      "pass --org <slug>, or run inside an org's repo"
    );
  }
  return org;
}

const enc = encodeURIComponent;

function query(params: Array<[string, string | undefined]>): string {
  const search = new URLSearchParams();
  for (const [name, value] of params) if (value !== undefined) search.set(name, value);
  const qs = search.toString();
  return qs ? `?${qs}` : "";
}

async function call(
  ctx: Context,
  method: "GET" | "POST" | "PUT",
  path: string,
  body?: Json
): Promise<unknown> {
  const creds = staffCreds(ctx);
  const response = await request({
    target: creds.target,
    path: `${CUSTOMER_APPS_API}/${path}`,
    method,
    body: body === undefined ? undefined : JSON.stringify(body),
    bearer: creds.bearer,
    headers: creds.headers
  });
  ensureOk(response, creds);
  return parseJson(response.body);
}

/** `<org>/<app>` → the route prefix `{org}/{app}`, resolved with the caller's credential. */
async function appPrefix(ctx: Context, app: string): Promise<string> {
  const { orgSlug, appSlug } = await resolveAppSlugs(staffCreds(ctx), app);
  return `${orgSlug}/${appSlug}`;
}

function emit(payload: unknown): void {
  process.stdout.write(`${JSON.stringify(payload)}\n`);
}

/** A table: left-aligned columns, padded to the widest cell, the header first. */
function table(header: string[], rows: string[][]): string {
  const all = [header, ...rows];
  const widths = header.map((_, i) => Math.max(...all.map((r) => (r[i] ?? "").length)));
  return all
    .map((r) =>
      r
        .map((c, i) => c.padEnd(widths[i] ?? 0))
        .join("  ")
        .trimEnd()
    )
    .join("\n");
}

/**
 * A time bound as the server takes it: an ISO date-time, or a span back from
 * now (`30m`, `2h`, `7d`).
 */
export function parseWhen(value: string, flag: string): string {
  const span = /^(\d+)([smhd])$/.exec(value.trim());
  if (span) {
    const unit = { s: 1, m: 60, h: 3600, d: 86400 }[span[2] as "s" | "m" | "h" | "d"];
    return new Date(Date.now() - Number(span[1]) * unit * 1000).toISOString();
  }
  const at = Date.parse(value);
  if (Number.isNaN(at)) {
    throw usageError(
      `${flag} is not a time: ${value}`,
      "an ISO date-time, or a span like 2h or 7d"
    );
  }
  return new Date(at).toISOString();
}

/** `3s`, `14m`, `2h 5m`, `3d` — an age in seconds, for a health column. */
function age(seconds: unknown): string {
  if (typeof seconds !== "number") return "-";
  if (seconds < 120) return `${Math.round(seconds)}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 120) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  return hours < 48 ? `${hours}h${minutes % 60}m` : `${Math.floor(hours / 24)}d${hours % 24}h`;
}

// ── ls ───────────────────────────────────────────────────────────────────────

interface ListenerRow {
  type: string;
  publisher: string;
  schemaRevision: string;
  lastPublishedAt: string;
  listener: string;
  environment: string;
  pending: number;
  retrying: number;
  dead: number;
  held: number;
  oldest: number | undefined;
  lastSuccessAt: string;
  lastFailureAt: string;
  lastFailureOutcome: string;
  inFlight: number;
  buildId: string;
  reconciledAt: string;
}

const count = (value: unknown): number => (typeof value === "number" ? value : 0);

/** One row per type × listener × environment; a type nobody listens to keeps one bare row. */
function listenerRows(payload: unknown): ListenerRow[] {
  const types = Array.isArray(payload) ? payload : list(field(payload, "types"));
  const rows: ListenerRow[] = [];
  for (const type of types) {
    const base = {
      type: text(field(type, "type")),
      publisher: text(field(type, "publisher")),
      schemaRevision: text(field(type, "schemaRevision", "schema_revision")),
      lastPublishedAt: text(field(type, "lastPublishedAt", "last_published_at"))
    };
    const listeners = list(field(type, "listeners"));
    if (listeners.length === 0) {
      rows.push({
        ...base,
        listener: "-",
        environment: "-",
        pending: 0,
        retrying: 0,
        dead: 0,
        held: 0,
        oldest: undefined,
        lastSuccessAt: "-",
        lastFailureAt: "-",
        lastFailureOutcome: "-",
        inFlight: 0,
        buildId: "-",
        reconciledAt: "-"
      });
      continue;
    }
    for (const l of listeners) {
      const oldest = field(l, "oldestPendingAgeSeconds", "oldest_pending_age_seconds");
      rows.push({
        ...base,
        listener: text(
          field(l, "listener", "to") ??
            (field(l, "app") ? `${text(field(l, "app"))}/${text(field(l, "function"))}` : undefined)
        ),
        environment: text(field(l, "environment")),
        pending: count(field(l, "pending")),
        retrying: count(field(l, "retrying")),
        dead: count(field(l, "dead")),
        held: count(field(l, "held")),
        oldest: typeof oldest === "number" ? oldest : undefined,
        lastSuccessAt: text(field(l, "lastSuccessAt", "last_success_at")),
        lastFailureAt: text(field(l, "lastFailureAt", "last_failure_at")),
        lastFailureOutcome: text(field(l, "lastFailureOutcome", "last_failure_outcome")),
        inFlight: count(field(l, "inFlight", "in_flight")),
        buildId: text(field(l, "buildId", "build_id")),
        reconciledAt: text(field(l, "reconciledAt", "reconciled_at"))
      });
    }
  }
  return rows;
}

/**
 * Oldest pending age first: a stuck listener is a growing age, not a high
 * count. A listener with nothing pending comes after, dead letters first.
 */
function byOldestPending(a: ListenerRow, b: ListenerRow): number {
  return (b.oldest ?? -1) - (a.oldest ?? -1) || b.dead - a.dead;
}

export async function runEventsLs(
  ctx: Context,
  opts: { org?: string; json: boolean }
): Promise<void> {
  const payload = await call(ctx, "GET", `${orgOf(ctx, opts.org)}/events/types`);
  if (opts.json) {
    emit(payload);
    return;
  }
  const rows = listenerRows(payload).sort(byOldestPending);
  if (rows.length === 0) {
    process.stderr.write("no event types\n");
    return;
  }
  const header = [
    "TYPE",
    "PUBLISHER",
    "REV",
    "LAST PUBLISHED",
    "LISTENER",
    "ENV",
    "PEND",
    "RETRY",
    "DEAD",
    "HELD",
    "OLDEST",
    "FLIGHT",
    "LAST OK",
    "LAST FAIL",
    "BUILD",
    "RECONCILED"
  ];
  process.stdout.write(
    `${table(
      header,
      rows.map((r) => [
        r.type,
        r.publisher,
        r.schemaRevision,
        r.lastPublishedAt,
        r.listener,
        r.environment,
        String(r.pending),
        String(r.retrying),
        String(r.dead),
        String(r.held),
        age(r.oldest),
        String(r.inFlight),
        r.lastSuccessAt,
        r.lastFailureAt === "-" ? "-" : `${r.lastFailureAt} (${r.lastFailureOutcome})`,
        r.buildId,
        r.reconciledAt
      ])
    )}\n`
  );
}

// ── show / find ──────────────────────────────────────────────────────────────

function attemptLine(attempt: unknown): string {
  const n = text(field(attempt, "n", "attempt", "number"), "?");
  const outcome = text(field(attempt, "outcome", "status"), "?");
  const at = field(attempt, "at", "startedAt", "started_at", "createdAt", "created_at");
  const invocation = field(attempt, "invocationId", "invocation_id");
  const error = field(attempt, "error");
  return [
    `      attempt ${n}: ${outcome}`,
    at !== undefined ? ` at ${text(at)}` : "",
    invocation !== undefined ? ` invocation ${text(invocation)}` : "",
    error !== undefined ? ` — ${text(error)}` : ""
  ].join("");
}

function printEvent(event: unknown): void {
  const lines = [
    `event ${text(field(event, "id", "eventId", "event_id"))}  ${text(field(event, "type"))}` +
      `  key ${text(field(event, "key"))}  env ${text(field(event, "environment"))}`
  ];
  const published = field(event, "publishedAt", "published_at", "time");
  if (published !== undefined) lines.push(`  published ${text(published)}`);
  for (const d of list(field(event, "deliveries"))) {
    lines.push(
      `  delivery ${text(field(d, "listener", "to"))} [${text(field(d, "environment"))}] ${text(field(d, "status"))}`
    );
    for (const a of list(field(d, "attempts"))) lines.push(attemptLine(a));
  }
  for (const [label, key] of [
    ["caused by", "parents"],
    ["caused", "children"]
  ] as const) {
    const related = list(field(event, key));
    if (related.length > 0) {
      lines.push(
        `  ${label}: ${related.map((e) => `${text(field(e, "id", "eventId"))} (${text(field(e, "type"))})`).join(", ")}`
      );
    }
  }
  process.stdout.write(`${lines.join("\n")}\n`);
}

/** `events show <id>`: the event and its deliveries; `--chain`, every event of its root. */
export async function runEventsShow(
  ctx: Context,
  eventId: string,
  opts: { org?: string; chain?: boolean; json: boolean }
): Promise<void> {
  const org = orgOf(ctx, opts.org);
  const event = await call(ctx, "GET", `${org}/events/${enc(eventId)}`);
  if (!opts.chain) {
    if (opts.json) emit(event);
    else printEvent(event);
    return;
  }
  const root = text(field(event, "rootId", "root_id", "root", "id"), eventId);
  const chain = await call(ctx, "GET", `${org}/events${query([["root", root]])}`);
  if (opts.json) {
    emit(chain);
    return;
  }
  const events = Array.isArray(chain) ? chain : list(field(chain, "events"));
  process.stdout.write(`root ${root}: ${events.length} event(s), in publish order\n`);
  for (const e of events) printEvent(e);
}

/** `events find <type> --key <key>`: the same as `show`, from the business key. */
export async function runEventsFind(
  ctx: Context,
  type: string,
  opts: { org?: string; key: string; appEnv?: string; json: boolean }
): Promise<void> {
  const environment = opts.appEnv === undefined ? undefined : parseAppEnv(opts.appEnv);
  const event = await call(
    ctx,
    "GET",
    `${orgOf(ctx, opts.org)}/events/find${query([
      ["type", type],
      ["key", opts.key],
      ["environment", environment]
    ])}`
  );
  if (opts.json) emit(event);
  else printEvent(event);
}

// ── tail ─────────────────────────────────────────────────────────────────────

const TAIL_POLL_MS = 2000;

function feedLine(item: unknown): string {
  const when = text(field(item, "at", "time", "createdAt", "created_at"), "");
  const kind = text(field(item, "kind", "event"), "");
  const type = text(field(item, "type"), "");
  const to = text(field(item, "listener", "to"), "");
  const status = text(field(item, "status", "outcome"), "");
  const id = text(field(item, "eventId", "event_id", "id"), "");
  return [when, kind, type, to, status, id].filter(Boolean).join("  ");
}

/**
 * `events tail <org>/<app>`: poll the feed every two seconds and print what is
 * new — publishes, relays and deliveries. Runs until interrupted; `maxPolls`
 * and `sleep` exist for the tests.
 */
export async function runEventsTail(
  ctx: Context,
  app: string,
  opts: {
    appEnv?: string;
    json: boolean;
    maxPolls?: number;
    sleep?: (ms: number) => Promise<void>;
  }
): Promise<void> {
  const prefix = await appPrefix(ctx, app);
  const environment = opts.appEnv === undefined ? "production" : parseAppEnv(opts.appEnv);
  const sleep = opts.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
  let since = new Date().toISOString();
  for (let poll = 0; opts.maxPolls === undefined || poll < opts.maxPolls; poll++) {
    if (poll > 0) await sleep(TAIL_POLL_MS);
    const feed = await call(
      ctx,
      "GET",
      `${prefix}/events/feed${query([
        ["since", since],
        ["environment", environment]
      ])}`
    );
    const items = Array.isArray(feed) ? feed : list(field(feed, "items", "events"));
    for (const item of items) {
      process.stdout.write(`${opts.json ? JSON.stringify(item) : feedLine(item)}\n`);
    }
    const next =
      field(feed, "cursor", "next", "until") ??
      (items.length > 0
        ? field(items[items.length - 1], "at", "time", "createdAt", "created_at")
        : undefined);
    if (typeof next === "string") since = next;
  }
}

// ── dead ─────────────────────────────────────────────────────────────────────

/** `events dead <org>/<app>`: dead deliveries, one block per error signature. */
export async function runEventsDead(
  ctx: Context,
  app: string,
  opts: { appEnv?: string; json: boolean }
): Promise<void> {
  const prefix = await appPrefix(ctx, app);
  const environment = opts.appEnv === undefined ? "production" : parseAppEnv(opts.appEnv);
  const payload = await call(
    ctx,
    "GET",
    `${prefix}/events/dead${query([["environment", environment]])}`
  );
  if (opts.json) {
    emit(payload);
    return;
  }
  const groups = Array.isArray(payload) ? payload : list(field(payload, "groups"));
  if (groups.length === 0) {
    process.stderr.write("no dead deliveries\n");
    return;
  }
  const blocks = groups.map((group) => {
    const deliveries = list(field(group, "deliveries"));
    const total = field(group, "count") ?? deliveries.length;
    const lines = [`${text(field(group, "signature"), "(no error)")}  (${text(total)} dead)`];
    for (const d of deliveries) {
      lines.push(
        `  ${text(field(d, "eventId", "event_id", "id"))}  ${text(field(d, "listener", "to"), "")}`.trimEnd()
      );
      for (const a of list(field(d, "attempts"))) lines.push(attemptLine(a));
    }
    return lines.join("\n");
  });
  process.stdout.write(`${blocks.join("\n\n")}\n`);
}

// ── redeliver / replay ───────────────────────────────────────────────────────

const REDELIVER_STATUSES = ["dead", "expired", "held"] as const;

function reportCount(payload: unknown, verb: string, dryRun: boolean | undefined): void {
  const n = text(field(payload, "count"), "?");
  process.stdout.write(
    dryRun
      ? `dry run: would ${verb} ${n} delivery(ies); nothing changed\n`
      : `${verb}: ${n} queued\n`
  );
}

/**
 * `events redeliver <event-id> [--to app/fn]`, or in bulk with no id:
 * `--to app/fn --status dead|expired|held [--since <time>]`. `--dry-run` only
 * counts.
 */
export async function runEventsRedeliver(
  ctx: Context,
  eventId: string | undefined,
  opts: {
    org?: string;
    to?: string;
    status?: string;
    since?: string;
    dryRun?: boolean;
    json: boolean;
  }
): Promise<void> {
  const org = orgOf(ctx, opts.org);
  let path: string;
  const body: Json = {};
  if (eventId !== undefined) {
    path = `${org}/events/${enc(eventId)}/redeliver`;
    if (opts.to !== undefined) body.to = opts.to;
  } else {
    if (opts.to === undefined || opts.status === undefined) {
      throw usageError(
        "a bulk redeliver needs --to <app>/<fn> and --status",
        "or name one event: `oxyc events redeliver <event-id>`"
      );
    }
    if (!(REDELIVER_STATUSES as readonly string[]).includes(opts.status)) {
      throw usageError(`--status is ${REDELIVER_STATUSES.join(", ")}, not ${opts.status}`);
    }
    path = `${org}/events/redeliver`;
    body.to = opts.to;
    body.status = opts.status;
    if (opts.since !== undefined) body.since = parseWhen(opts.since, "--since");
  }
  if (opts.dryRun) body.dryRun = true;
  const payload = await call(ctx, "POST", path, body);
  if (opts.json) emit(payload);
  else reportCount(payload, "redeliver", opts.dryRun);
}

/** `events replay <type> --since <time> --to app/fn`: a seek, for a listener that joined late. */
export async function runEventsReplay(
  ctx: Context,
  type: string,
  opts: {
    org?: string;
    since: string;
    until?: string;
    to: string;
    appEnv?: string;
    dryRun?: boolean;
    json: boolean;
  }
): Promise<void> {
  const body: Json = {
    type,
    since: parseWhen(opts.since, "--since"),
    to: opts.to,
    environment: opts.appEnv === undefined ? "production" : parseAppEnv(opts.appEnv)
  };
  if (opts.until !== undefined) body.until = parseWhen(opts.until, "--until");
  if (opts.dryRun) body.dryRun = true;
  const payload = await call(ctx, "POST", `${orgOf(ctx, opts.org)}/events/replay`, body);
  if (opts.json) emit(payload);
  else reportCount(payload, "replay", opts.dryRun);
}

// ── discard / purge ──────────────────────────────────────────────────────────

/** `events discard --until <time> [--type] [--to]`: pending and held deliveries become discarded. */
export async function runEventsDiscard(
  ctx: Context,
  opts: {
    org?: string;
    type?: string;
    to?: string;
    until: string;
    yes?: boolean;
    json: boolean;
  }
): Promise<void> {
  const body: Json = { until: parseWhen(opts.until, "--until") };
  if (opts.type !== undefined) body.type = opts.type;
  if (opts.to !== undefined) body.to = opts.to;
  if (!opts.yes) {
    await confirm(
      `Discard pending and held deliveries${opts.type ? ` of ${opts.type}` : ""}${opts.to ? ` to ${opts.to}` : ""} up to ${String(body.until)}?`,
      {
        verb: "a discard",
        why: "discarded deliveries are never delivered, and the discard is audited",
        declined: "nothing discarded — the confirmation was declined"
      }
    );
  }
  const payload = await call(ctx, "POST", `${orgOf(ctx, opts.org)}/events/discard`, body);
  if (opts.json) emit(payload);
  else process.stdout.write(`discarded: ${text(field(payload, "count"), "?")}\n`);
}

/** `events purge <type> [--key] [--before]`: hard-deletes the events and their outbox rows. */
export async function runEventsPurge(
  ctx: Context,
  type: string,
  opts: { org?: string; key?: string; before?: string; yes?: boolean; json: boolean }
): Promise<void> {
  const body: Json = { type };
  if (opts.key !== undefined) body.key = opts.key;
  if (opts.before !== undefined) body.before = parseWhen(opts.before, "--before");
  if (!opts.yes) {
    await confirm(
      `Permanently delete ${type}${opts.key ? ` events keyed ${opts.key}` : " events"}${opts.before ? ` before ${String(body.before)}` : ""}? This cannot be undone.`,
      {
        verb: "a purge",
        why: "purge hard-deletes events and their outbox rows, and is audited",
        declined: "nothing purged — the confirmation was declined"
      }
    );
  }
  const payload = await call(ctx, "POST", `${orgOf(ctx, opts.org)}/events/purge`, body);
  if (opts.json) emit(payload);
  else process.stdout.write(`purged: ${text(field(payload, "count"), "?")}\n`);
}

// ── env events ───────────────────────────────────────────────────────────────

/**
 * `env events <app> --app-env <env> --on|--off`: the environment's events
 * switch. Off by default outside production, where events are recorded and held;
 * production always delivers, and the server refuses a flip there.
 */
export async function runEnvEvents(
  ctx: Context,
  app: string,
  opts: { appEnv?: string; on?: boolean; off?: boolean; json: boolean }
): Promise<void> {
  if (opts.on && opts.off) throw usageError("--on and --off together do nothing", "drop one");
  if (!opts.on && !opts.off) {
    throw usageError("say which: --on or --off", "the events switch has no read-back route");
  }
  if (opts.appEnv === undefined) {
    throw usageError("--app-env is required", "staging or a dev-<handle> sandbox");
  }
  const environment = parseAppEnv(opts.appEnv);
  if (environment === "production") {
    throw usageError(
      "production has no events switch",
      "its events are always delivered — name staging or a dev-<handle> sandbox"
    );
  }
  const enabled = Boolean(opts.on);
  const prefix = await appPrefix(ctx, app);
  const payload = await call(ctx, "PUT", `${prefix}/environments/${enc(environment)}/events`, {
    enabled
  });
  if (opts.json) {
    emit(payload);
    return;
  }
  process.stdout.write(`${environment}: events ${enabled ? "on" : "off"}\n`);
}
