#!/usr/bin/env node
// Classic-agent census (internal-docs/2026-11-09-classic-agent-retirement.md § 5.2).
//
// Lists, for every workspace on a deployment, what would fail once
// `compile_refuse_classic_agents` is on:
//   - classic agent files (`*.agent.yml` / `*.agent.yaml`) in the working copy,
//   - automations whose `agent_ref` names one,
//   - schedules whose target names one.
// It reads only; it changes nothing. Run it with your own `oxyc` login and a
// reason, since staff reach into a tenant needs an assume-role session per org:
//
//   oxyc login --env prod
//   node scripts/classic-agents-census.mjs --env prod -r "classic-agent census before the flag flip"
//   node scripts/classic-agents-census.mjs --env prod -r "…" --org acme --org beta   # some orgs only
//   node scripts/classic-agents-census.mjs --env prod -r "…" --json > census.json
//
// The file listing is served by the Factory (an IDE route), so a workspace the
// Factory cannot open is reported as `unreadable`, not as clean.
//
// Not covered here, because no API exposes it: Slack saved agent paths and
// classic threads. Run these against the production database:
//   SELECT count(*) FROM slack_user_preferences WHERE default_agent_path ~ '\.agent\.ya?ml';
//   SELECT count(*) FROM slack_threads WHERE agent_path ~ '\.agent\.ya?ml';
//   SELECT count(*), max(created_at) FROM threads WHERE source_type = 'agent';
// A saved Slack path to a classic agent already falls back to the workspace
// default (integrations/slack/resolution/saved_agent.rs), so those rows need no
// script unless someone wants them tidied.

import { execFileSync } from "node:child_process";

const CLASSIC = /\.agent\.ya?ml$/i;
const CLASSIC_REF = /agent_ref\s*:\s*["']?([^\s"'#]+\.agent\.ya?ml)/gi;
const SKIP_DIR = /^(\.|node_modules$|dist$|build$|target$|__pycache__$)/;

function parseArgs(argv) {
  const args = { env: "prod", reason: "", orgs: [], json: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--env") args.env = argv[++i];
    else if (a === "-r" || a === "--reason") args.reason = argv[++i];
    else if (a === "--org") args.orgs.push(argv[++i]);
    else if (a === "--json") args.json = true;
    else throw new Error(`unknown argument ${a}`);
  }
  if (!args.reason) throw new Error("a reason is required: -r \"why you are reading tenant files\"");
  return args;
}

function oxyc(env, args) {
  return execFileSync("oxyc", [...args, "--env", env], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    maxBuffer: 64 * 1024 * 1024,
  });
}

function api(env, path, extra = []) {
  return JSON.parse(oxyc(env, ["api", path, ...extra]));
}

/** Every classic agent path in a file tree, skipping what discovery skips. */
function classicFiles(nodes, out = []) {
  for (const node of nodes ?? []) {
    if (node.is_dir) {
      if (!SKIP_DIR.test(node.name)) classicFiles(node.children, out);
    } else if (CLASSIC.test(node.name) && !/\.test\./i.test(node.name)) {
      out.push(node.path);
    }
  }
  return out;
}

/** The first line of a failed `oxyc` call's stderr, or its message. */
function firstLine(e) {
  return String(e.stderr || e.message).trim().split("\n")[0];
}

function emptyFinding(ws) {
  return { workspace: ws.name, id: ws.id, files: [], automationRefs: [], schedules: [] };
}

function scanWorkspace(env, ws) {
  const found = emptyFinding(ws);
  try {
    const tree = api(env, `/api/${ws.id}/files`);
    found.files = classicFiles(tree.primary);
    for (const repo of tree.repositories ?? []) found.files.push(...classicFiles(repo.tree ?? repo.children));
  } catch (e) {
    found.unreadable = `file tree: ${firstLine(e)}`;
  }
  try {
    for (const file of api(env, `/api/${ws.id}/agentic-automations/files`)) {
      const { content } = api(env, `/api/${ws.id}/agentic-automations/files/${file.path_b64}`);
      for (const m of String(content).matchAll(CLASSIC_REF)) {
        found.automationRefs.push({ automation: file.path, agent_ref: m[1] });
      }
    }
  } catch (e) {
    found.automationsError = firstLine(e);
  }
  try {
    const schedules = api(env, `/api/${ws.id}/agentic-schedules`);
    for (const s of Array.isArray(schedules) ? schedules : schedules.schedules ?? []) {
      if (CLASSIC.test(String(s.target_ref ?? ""))) found.schedules.push({ id: s.id, target_ref: s.target_ref });
    }
  } catch (e) {
    found.schedulesError = firstLine(e);
  }
  return found;
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const workspaces = api(args.env, "/api/admin/workspaces-meta?page_size=200", ["--paginate"]);
  const byOrg = new Map();
  for (const ws of workspaces) {
    const org = ws.org_slug ?? "(no org)";
    if (args.orgs.length && !args.orgs.includes(org)) continue;
    if (!byOrg.has(org)) byOrg.set(org, []);
    byOrg.get(org).push(ws);
  }

  const report = [];
  for (const [org, list] of byOrg) {
    const assumed = org !== "(no org)";
    if (assumed) {
      // One org this login cannot act as is reported, not fatal: the rest of
      // the census, and what was already gathered, still prints.
      try {
        oxyc(args.env, ["assume", "start", "--org", org, "-r", args.reason]);
      } catch (e) {
        const why = `assume start: ${firstLine(e)}`;
        for (const ws of list) report.push({ org, ...emptyFinding(ws), unreadable: why });
        continue;
      }
    }
    try {
      for (const ws of list) report.push({ org, ...scanWorkspace(args.env, ws) });
    } finally {
      if (assumed) {
        try {
          oxyc(args.env, ["assume", "end", "--org", org]);
        } catch (e) {
          console.error(`warning: could not end the assume session for ${org}: ${firstLine(e)}`);
        }
      }
    }
  }

  if (args.json) {
    process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
    return;
  }
  let affected = 0;
  for (const r of report) {
    const n = r.files.length + r.automationRefs.length + r.schedules.length;
    const problems = [r.unreadable, r.automationsError, r.schedulesError].filter(Boolean);
    if (!n && !problems.length) continue;
    if (n) affected++;
    console.log(`\n${r.org} / ${r.workspace} (${r.id})`);
    for (const f of r.files) console.log(`  file      ${f}`);
    for (const a of r.automationRefs) console.log(`  automation ${a.automation} -> ${a.agent_ref}`);
    for (const s of r.schedules) console.log(`  schedule  ${s.id} -> ${s.target_ref}`);
    for (const p of problems) console.log(`  could not check: ${p}`);
  }
  console.log(`\n${report.length} workspace(s) scanned, ${affected} with classic agents.`);
  if (affected) {
    console.log("Fix each with `oxy migrate-agents --dry-run --json <dir>` in its repository, then without --dry-run.");
  }
}

main();
