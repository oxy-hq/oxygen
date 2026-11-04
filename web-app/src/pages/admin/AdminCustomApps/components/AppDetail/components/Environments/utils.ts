import type { AppEnvironment } from "@/services/api/customApps";

/** Every plane of app state, in the order the design lists them. */
export const PLANE_ORDER = [
  "build",
  "semantic",
  "reads",
  "oltp",
  "airhouse",
  "warehouse",
  "storage",
  "secrets",
  "messages",
  "fetch",
  "schedules",
  "workspace-actions"
] as const;

/** What each plane covers, in a few words. */
export const PLANE_LABEL: Record<string, string> = {
  build: "Build",
  semantic: "Semantic model",
  reads: "Warehouse reads",
  oltp: "Records (ctx.oltp)",
  airhouse: "Facts (ctx.airhouse)",
  warehouse: "Warehouse writes",
  storage: "Files (ctx.storage)",
  secrets: "Secrets (ctx.env)",
  messages: "Email and notifications",
  fetch: "Third-party writes",
  schedules: "Schedules",
  "workspace-actions": "Workspace actions"
};

/** What a reset of each plane throws away, for its confirm. */
const RESET_COPY: Record<string, string> = {
  airhouse: "Every Airhouse table it copied is dropped, and a snapshot moves to now.",
  storage: "Every file it wrote is deleted.",
  secrets: "Every secret value set on it is deleted.",
  messages: "Its outbox is emptied."
};

/** The confirm's sentence for resetting `plane` of `env`. */
export const resetCopy = (plane: string, env: AppEnvironment): string => {
  if (plane === "oltp") {
    return env.planes?.oltp === "empty"
      ? "Its database branch is cut again with production's tables and none of their rows. Every row it wrote is lost."
      : "Its database branch is cut again from production now. Every row it wrote is lost.";
  }
  return RESET_COPY[plane] ?? "It goes back to a clean copy of its source.";
};

/** A plane's chosen mode: a string, or the `mode` of an object choice. */
const modeOf = (choice: unknown): string => {
  if (typeof choice === "string") return choice;
  if (choice && typeof choice === "object" && "mode" in choice) {
    return String((choice as { mode: unknown }).mode);
  }
  return "set";
};

/** The planes this environment chose itself (not defaults), in display order. */
export const chosenPlanes = (env: AppEnvironment): { plane: string; mode: string }[] => {
  const set = env.planes_set ?? {};
  return PLANE_ORDER.filter((plane) => plane in set).map((plane) => ({
    plane,
    mode: modeOf(set[plane])
  }));
};

/** The provider-sandbox hosts a `fetch=allow` choice lists. */
export const fetchHosts = (env: AppEnvironment): string[] => {
  const choice = env.planes_set?.fetch;
  if (!choice || typeof choice !== "object" || !("hosts" in choice)) return [];
  const hosts = (choice as { hosts: unknown }).hosts;
  return Array.isArray(hosts) ? hosts.map(String) : [];
};

const DAY_MS = 24 * 60 * 60 * 1000;

export type Deletion = { at: Date; reason: string };

/**
 * When a sandbox is deleted, and why: the earliest of its idle expiry, its OLTP
 * branch's expiry, and the day the server deletes it for holding a copy of
 * production (`copies_expire_at`, the maintenance sweep's own rule). `null` for
 * production and staging, which are never deleted.
 */
export const deletion = (env: AppEnvironment): Deletion | null => {
  if (env.kind !== "dev") return null;
  const candidates: Deletion[] = [];
  if (env.expires_at) candidates.push({ at: new Date(env.expires_at), reason: "idle" });
  for (const at of [env.oltp_branch?.expires_at, env.copies_expire_at]) {
    if (at) candidates.push({ at: new Date(at), reason: "30 days of copied data" });
  }
  const valid = candidates.filter((c) => !Number.isNaN(c.at.getTime()));
  if (valid.length === 0) return null;
  return valid.reduce((soonest, c) => (c.at < soonest.at ? c : soonest));
};

/** OLTP branch states a worker is moving through; `ctx.oltp` refuses until `ready`. */
const BRANCH_BUSY = new Set(["cutting", "migrating", "resetting"]);

/**
 * A re-cut of the sandbox's OLTP branch that the server queued and the worker has
 * not started: the branch still reads `ready` until then. The server reads it off
 * the queue. Comparing the sandbox's `fork_at` with the branch's would not do: an
 * Airhouse change that starts copying moves the sandbox's too.
 */
export const oltpRecutQueued = (env: AppEnvironment): boolean =>
  env.oltp_branch?.status === "ready" && env.oltp_branch.recut_queued;

/** Whether the server is still working on `env`, so the list keeps polling. */
export const settling = (env: AppEnvironment): boolean =>
  env.status === "deleting" ||
  BRANCH_BUSY.has(env.oltp_branch?.status ?? "") ||
  oltpRecutQueued(env);

/** "today", "tomorrow", "in 12 days". */
export const untilText = (at: Date, now: Date = new Date()): string => {
  const days = Math.ceil((at.getTime() - now.getTime()) / DAY_MS);
  if (days <= 0) return "today";
  if (days === 1) return "tomorrow";
  return `in ${days} days`;
};

/**
 * The planes a person may reset here. A sandbox resets its own copies; staging
 * resets only its outbox, because the team shares the rest.
 */
export const resettablePlanes = (env: AppEnvironment): string[] => {
  if (env.kind === "staging") return env.planes?.messages === "capture" ? ["messages"] : [];
  // A sandbox being deleted refuses every reset.
  if (env.kind !== "dev" || env.status === "deleting") return [];
  const planes = env.planes ?? {};
  const out: string[] = [];
  if (planes.oltp === "branch" || planes.oltp === "empty") out.push("oltp");
  // On every mode: a sandbox moved off branch or snapshot keeps the rows it
  // copied, and this reset is what drops them.
  out.push("airhouse", "storage", "secrets", "messages");
  return out;
};
