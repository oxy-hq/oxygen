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
export const RESET_COPY: Record<string, string> = {
  oltp: "Its database branch is cut again from production now. Every row it wrote is lost.",
  airhouse: "Its copied Airhouse tables are dropped and its snapshot moves to now.",
  storage: "Every file it wrote is deleted.",
  secrets: "Every secret value set on it is deleted.",
  messages: "Its outbox is emptied."
};

const DAY_MS = 24 * 60 * 60 * 1000;

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

export type Deletion = { at: Date; reason: string };

/**
 * When a sandbox is deleted, and why: the earliest of its idle expiry, its OLTP
 * branch's expiry, and 30 days past its fork when Airhouse holds a copy of
 * production. `null` for production and staging, which are never deleted.
 */
export const deletion = (env: AppEnvironment): Deletion | null => {
  if (env.kind !== "dev") return null;
  const candidates: Deletion[] = [];
  if (env.expires_at) candidates.push({ at: new Date(env.expires_at), reason: "idle" });
  if (env.oltp_branch?.expires_at) {
    candidates.push({ at: new Date(env.oltp_branch.expires_at), reason: "30 days of copied data" });
  }
  const airhouse = env.planes?.airhouse;
  if (env.fork_at && (airhouse === "branch" || airhouse === "snapshot")) {
    const at = new Date(new Date(env.fork_at).getTime() + 30 * DAY_MS);
    candidates.push({ at, reason: "30 days of copied data" });
  }
  const valid = candidates.filter((c) => !Number.isNaN(c.at.getTime()));
  if (valid.length === 0) return null;
  return valid.reduce((soonest, c) => (c.at < soonest.at ? c : soonest));
};

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
  if (env.kind !== "dev") return [];
  const planes = env.planes ?? {};
  const out: string[] = [];
  if (planes.oltp === "branch" || planes.oltp === "empty") out.push("oltp");
  if (planes.airhouse === "branch" || planes.airhouse === "snapshot") out.push("airhouse");
  out.push("storage", "secrets", "messages");
  return out;
};
