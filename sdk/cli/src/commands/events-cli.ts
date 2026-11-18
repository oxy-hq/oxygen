/**
 * Wires `oxyc events …` onto the program: who publishes what, listener
 * health, redelivery and replay.
 *
 * Registered by `main.ts` since the staff endpoints shipped (the server's
 * `app_events::api`); `main.test.ts` pins that `oxyc --help` offers the
 * group. The verbs themselves (`./events.ts`) are tested on their own, and
 * the server pins the JSON keys they read (`app_events_api::contract`).
 */

import type { Command } from "commander";
import { createContext, type GlobalFlags } from "../context/resolve.js";
import {
  runEventsDead,
  runEventsDiscard,
  runEventsFind,
  runEventsLs,
  runEventsPurge,
  runEventsRedeliver,
  runEventsReplay,
  runEventsShow,
  runEventsTail
} from "./events.js";
import { runEventsResync } from "./events-resync.js";

export interface Wiring {
  withGlobals: (command: Command) => Command;
  globals: (opts: Record<string, unknown>) => GlobalFlags;
}

export function registerEvents(program: Command, w: Wiring): void {
  const events = program
    .command("events")
    .description("app events: who publishes what, listener health, redelivery and replay");
  w.withGlobals(
    events
      .command("ls [org]")
      .description(
        "every event type with its publisher and each listener's health, stuck ones first"
      )
      .option("--json", "emit the server's reply as JSON")
  ).action(async (org: string | undefined, opts: Record<string, unknown>) => {
    await runEventsLs(createContext(w.globals(opts)), { org, json: Boolean(opts.json) });
  });
  w.withGlobals(
    events
      .command("show <event-id>")
      .description("an event's deliveries with every attempt, and its causation chain")
      .option("--chain", "list every event that shares this event's root")
      .option("--json", "emit the server's reply as JSON")
  ).action(async (eventId: string, opts: Record<string, unknown>) => {
    await runEventsShow(createContext(w.globals(opts)), eventId, {
      chain: Boolean(opts.chain),
      json: Boolean(opts.json)
    });
  });
  w.withGlobals(
    events
      .command("find <type>")
      .description("show, starting from the type and business key (statement:123)")
      .requiredOption("--key <key>", "the event's business key")
      .option("--app-env <environment>", "the environment (default: production)")
      .option("--json", "emit the server's reply as JSON")
  ).action(async (type: string, opts: Record<string, unknown>) => {
    await runEventsFind(createContext(w.globals(opts)), type, {
      key: opts.key as string,
      appEnv: opts.appEnv as string | undefined,
      json: Boolean(opts.json)
    });
  });
  w.withGlobals(
    events
      .command("tail <app>")
      .description("live publishes, relays and deliveries of <org>/<app> (polls every 2s)")
      .option("--app-env <environment>", "the environment (default: production)")
      .option("--json", "one JSON object per line")
  ).action(async (app: string, opts: Record<string, unknown>) => {
    await runEventsTail(createContext(w.globals(opts)), app, {
      appEnv: opts.appEnv as string | undefined,
      json: Boolean(opts.json)
    });
  });
  w.withGlobals(
    events
      .command("dead <app>")
      .description("dead deliveries of <org>/<app>'s listeners, grouped by error signature")
      .option("--app-env <environment>", "the environment (default: production)")
      .option("--json", "emit the server's reply as JSON")
  ).action(async (app: string, opts: Record<string, unknown>) => {
    await runEventsDead(createContext(w.globals(opts)), app, {
      appEnv: opts.appEnv as string | undefined,
      json: Boolean(opts.json)
    });
  });
  w.withGlobals(
    events
      .command("redeliver [event-id]")
      .description(
        "queue dead, expired or held deliveries again: one event, or in bulk with --to and --status"
      )
      .option("--to <app/fn>", "only this listener (required for a bulk redeliver)")
      .option("--status <status>", "bulk: dead, expired or held")
      .option("--since <time>", "bulk: only deliveries since an ISO time or a span (2h, 7d)")
      .option("--dry-run", "print the count and change nothing")
      .option("--json", "emit the server's reply as JSON")
  ).action(async (eventId: string | undefined, opts: Record<string, unknown>) => {
    await runEventsRedeliver(createContext(w.globals(opts)), eventId, {
      to: opts.to as string | undefined,
      status: opts.status as string | undefined,
      since: opts.since as string | undefined,
      dryRun: Boolean(opts.dryRun),
      json: Boolean(opts.json)
    });
  });
  w.withGlobals(
    events
      .command("replay <type>")
      .description("deliver retained events of a type to a listener that joined late")
      .requiredOption("--since <time>", "an ISO time or a span (2h, 7d)")
      .option("--until <time>", "an ISO time or a span")
      .requiredOption("--to <app/fn>", "the listener")
      .option("--app-env <environment>", "the environment (default: production)")
      .option("--dry-run", "print the count and change nothing")
      .option("--json", "emit the server's reply as JSON")
  ).action(async (type: string, opts: Record<string, unknown>) => {
    await runEventsReplay(createContext(w.globals(opts)), type, {
      since: opts.since as string,
      until: opts.until as string | undefined,
      to: opts.to as string,
      appEnv: opts.appEnv as string | undefined,
      dryRun: Boolean(opts.dryRun),
      json: Boolean(opts.json)
    });
  });
  w.withGlobals(
    events
      .command("resync <app> <function>")
      .description(
        "move <org>/<app>'s function's table cursors to now, skipping what changed meanwhile, and clear behind-horizon, broken and paused"
      )
      .option("--app-env <environment>", "the environment (default: production)")
      .option("--backfill", "then run the function once with an event whose data.backfill is true")
      .option("--dry-run", "print where each cursor would move and change nothing")
      .option("--json", "emit the server's reply as JSON")
  ).action(async (app: string, fn: string, opts: Record<string, unknown>) => {
    await runEventsResync(createContext(w.globals(opts)), app, fn, {
      appEnv: opts.appEnv as string | undefined,
      backfill: Boolean(opts.backfill),
      dryRun: Boolean(opts.dryRun),
      json: Boolean(opts.json)
    });
  });
  w.withGlobals(
    events
      .command("discard")
      .description("mark pending and held deliveries discarded (audited)")
      .requiredOption("--until <time>", "an ISO time or a span (2h, 7d)")
      .option("--type <type>", "only this event type")
      .option("--to <app/fn>", "only this listener")
      .option("--yes", "act without asking — needed off a terminal")
      .option("--json", "emit the server's reply as JSON")
  ).action(async (opts: Record<string, unknown>) => {
    await runEventsDiscard(createContext(w.globals(opts)), {
      type: opts.type as string | undefined,
      to: opts.to as string | undefined,
      until: opts.until as string,
      yes: Boolean(opts.yes),
      json: Boolean(opts.json)
    });
  });
  w.withGlobals(
    events
      .command("purge <type>")
      .description("hard-delete a type's events and outbox rows (audited, permanent)")
      .option("--key <key>", "only the event with this business key")
      .option("--before <time>", "only events before an ISO time or a span (2h, 7d)")
      .option("--yes", "act without asking — needed off a terminal")
      .option("--json", "emit the server's reply as JSON")
  ).action(async (type: string, opts: Record<string, unknown>) => {
    await runEventsPurge(createContext(w.globals(opts)), type, {
      key: opts.key as string | undefined,
      before: opts.before as string | undefined,
      yes: Boolean(opts.yes),
      json: Boolean(opts.json)
    });
  });
}
