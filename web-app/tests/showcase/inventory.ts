// What the seeded instance holds, read from its database, and the placeholder
// language plans use to name it. Workspace ids other than the Demo's are
// minted per seed, so a plan written against one boot must not carry them:
// `{ws:<org_slug>/<workspace name>}` resolves at capture time instead.

import { spawnSync } from "node:child_process";

export interface Inventory {
  workspaces: { org_slug: string; name: string; id: string }[];
  apps: { org_slug: string; slug: string; name: string }[];
}

const WORKSPACES_SQL = `
SELECT o.slug AS org_slug, w.name, w.id::text AS id
FROM workspaces w JOIN organizations o ON o.id = w.org_id
ORDER BY o.slug, w.name`;

const APPS_SQL = `
SELECT o.slug AS org_slug, a.slug, a.name
FROM apps a JOIN organizations o ON o.id = a.org_id
ORDER BY o.slug, a.slug`;

function queryJson<T>(databaseUrl: string, sql: string): T[] {
  const wrapped = `SELECT coalesce(json_agg(t), '[]'::json) FROM (${sql}) t`;
  const res = spawnSync("psql", [databaseUrl, "-XAtc", wrapped], { encoding: "utf-8" });
  if (res.status !== 0) {
    throw new Error(`psql failed: ${res.stderr || res.error?.message || `exit ${res.status}`}`);
  }
  return JSON.parse(res.stdout.trim() || "[]") as T[];
}

/** The inventory narrowed to one org — what a plan is allowed to reach. */
export function onlyOrg(inventory: Inventory, orgSlug: string): Inventory {
  return {
    workspaces: inventory.workspaces.filter((w) => w.org_slug === orgSlug),
    apps: inventory.apps.filter((a) => a.org_slug === orgSlug)
  };
}

export function readInventory(databaseUrl: string): Inventory {
  return {
    workspaces: queryJson(databaseUrl, WORKSPACES_SQL),
    apps: queryJson(databaseUrl, APPS_SQL)
  };
}

const WS_PLACEHOLDER = /\{ws:([^/}]+)\/([^}]+)\}/g;

/** Replace every `{ws:org/name}` with its id; throws naming the first unknown one. */
export function resolvePlaceholders(text: string, inventory: Inventory): string {
  return text.replace(WS_PLACEHOLDER, (_all, org: string, name: string) => {
    const ws = inventory.workspaces.find((w) => w.org_slug === org && w.name === name);
    if (!ws) throw new Error(`no workspace '${name}' in org '${org}' on this instance`);
    return ws.id;
  });
}

/** The inventory as the planner reads it: names and placeholders, never raw ids. */
export function describeInventory(inventory: Inventory): string {
  const ws = inventory.workspaces.map(
    (w) => `- org \`${w.org_slug}\`, workspace "${w.name}" → \`{ws:${w.org_slug}/${w.name}}\``
  );
  const apps = inventory.apps.map(
    (a) => `- org \`${a.org_slug}\`: custom app \`${a.slug}\` ("${a.name}")`
  );
  return [
    "Workspaces:",
    ...(ws.length ? ws : ["- (none)"]),
    "Custom apps:",
    ...(apps.length ? apps : ["- (none)"])
  ].join("\n");
}
