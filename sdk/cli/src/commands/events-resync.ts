/**
 * `oxyc events resync <org>/<app> <function>`: the way out of every state a
 * table subscription can be stuck in (app events design §7.3). The server
 * moves each watched table's cursor to now, so what changed meanwhile is
 * skipped, not replayed, and clears "behind horizon", "broken" and "paused".
 * With `--backfill` the function then runs once with an event whose
 * `data.backfill` is true, to re-derive what it keeps from the whole table.
 */

import { parseAppEnv } from "../apps/environment.js";
import type { Context } from "../context/resolve.js";
import { usageError } from "../util/errors.js";
import { appPrefix, call, emit, enc } from "./events.js";

interface ResyncedTable {
  table?: string;
  from?: number | null;
  to?: number;
  state?: string;
}

export async function runEventsResync(
  ctx: Context,
  app: string,
  fn: string,
  opts: { appEnv?: string; backfill?: boolean; dryRun?: boolean; json: boolean }
): Promise<void> {
  if (fn.trim() === "") {
    throw usageError("which function? `oxyc events resync <org>/<app> <function>`");
  }
  const prefix = await appPrefix(ctx, app);
  const environment = opts.appEnv === undefined ? "production" : parseAppEnv(opts.appEnv);
  const body: Record<string, unknown> = { function: fn, environment };
  if (opts.backfill) body.backfill = true;
  if (opts.dryRun) body.dryRun = true;
  const payload = await call(ctx, "POST", `${prefix}/events/resync`, body);
  if (opts.json) {
    emit(payload);
    return;
  }
  const reply = (typeof payload === "object" && payload !== null ? payload : {}) as {
    tables?: ResyncedTable[];
    queued?: number;
    dryRun?: boolean;
  };
  const tables = reply.tables ?? [];
  const verb = reply.dryRun ? "would move" : "moved";
  for (const t of tables) {
    process.stdout.write(
      `${verb} ${t.table ?? "?"}: snapshot ${t.from ?? "-"} -> ${t.to ?? "?"} (was ${t.state ?? "?"})\n`
    );
  }
  if (reply.dryRun) {
    process.stdout.write("dry run: nothing changed\n");
  } else if (opts.backfill) {
    process.stdout.write(`backfill: ${reply.queued ?? 0} run(s) queued for ${enc(fn)}\n`);
  }
}
