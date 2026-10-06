/**
 * `oxyc mcp` — the Oxy API as MCP tools, over stdio.
 *
 * NOT THE SAME THING AS `oxy mcp`. That one is workspace tooling: it takes a
 * local checkout and exposes the semantic model, automations and `.sql` files
 * in it. This one takes a TOKEN and exposes the deployment's HTTP API — the
 * same surface `oxyc api` reaches, for an agent that would rather call a tool
 * than shell out. Different input, different audience, no overlap.
 *
 * FOUR GENERIC TOOLS FOR THE WHOLE API — the design decision that makes most
 * of it affordable.
 *
 * The obvious shape is one tool per endpoint, and it is the wrong one: the API
 * has ~670 routes, an agent runtime ships every tool's JSON schema in every
 * request, and that is tens of kilobytes of context spent on each turn before
 * a single question is asked. It would also go stale on every deploy, since
 * the tool list would be baked into this package rather than read from the
 * deployment.
 *
 * So discovery stays a QUESTION the agent asks (`oxy_routes`, `oxy_schema`)
 * rather than a payload it carries, exactly as the CLI does it, for
 * everything a generic request can do safely.
 *
 * TWO NAMED EXCEPTIONS: `SANDBOX_TOOLS` and `PREVIEW_TOOLS` below. The sandbox
 * loop and the workspace-previews loop are meant to run UNSUPERVISED — create
 * a sandbox, publish into it, call a function, tear it down — and that needs
 * per-operation validation (the `dev-<handle>` grammar, a confirm gate before
 * a delete, a hard refusal of `--app-env` outside `dev-<handle>` on the
 * publish tool) that `oxy_request` has no way to carry. Each of those tools
 * calls its CLI verb's own request-building function (`env.ts`, `fn.ts`,
 * `checks.ts`, `preview.ts`, `preview-runs.ts`, …), so there is exactly one
 * implementation of each request, and a tool's result is the same JSON
 * document that verb's `--json` prints. `mcp.test.ts` pins the full tool list
 * and a raised (and justified) per-turn schema budget.
 *
 * Target resolution and placeholder substitution are the CLI's — the same
 * `Context`, so `{org}` / `{workspace}` resolve the same way.
 *
 * THE CREDENTIAL IS THE TOKEN VARIABLE AND NOTHING ELSE, unless the server is
 * started with `--login`. An agent runtime starts this with the agent's own
 * token in `OXY_TOKEN`; with the variable unset, the process exits `4` rather
 * than serving on whatever `oxyc login` cached for the machine's owner. A
 * person who does want their own login passes `--login`.
 *
 * A SANDBOX AGENT TOKEN (`oxy_sbx_…`) GETS A DIFFERENT, SMALLER LIST — the
 * sandbox loop, `oxy_whoami`, three secret tools and `oxy_token_revoke`; no
 * `oxy_request`, no discovery, no previews. See `mcp-sandbox.ts`.
 */

import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { CallToolRequestSchema, ListToolsRequestSchema } from "@modelcontextprotocol/sdk/types.js";
import { loadCatalog, loadOpenApi, searchRoutes } from "../api/catalog.js";
import { paramsToQuery, parseFields } from "../api/fields.js";
import { runJq } from "../api/output.js";
import { isExternalSurface, normalizePath, substitutePlaceholders } from "../api/paths.js";
import { parseJson, request } from "../api/request.js";
import { markMcpSession } from "../api/user-agent.js";
import { requireSandboxName } from "../apps/environment.js";
import { describeSandboxToken, forgetSandboxTokens } from "../apps/sandbox-token.js";
import { revokeCallingToken } from "../auth/token-api.js";
import { isSandboxAgentToken } from "../auth/token-kind.js";
import { type Context, createContext } from "../context/resolve.js";
import {
  CliError,
  ExitCode,
  type ExitCodeValue,
  exitCodeName,
  usageError
} from "../util/errors.js";
import { runChecksCore } from "./checks.js";
import { comparablePath } from "./discover.js";
import { envCreate, envDelete, envList, envShow } from "./env.js";
import { secretDelete, secretList, secretSet } from "./env-secrets.js";
import { fnCall } from "./fn.js";
import { invocationsHeld, invocationsList } from "./invocations.js";
import { fetchLogs } from "./logs.js";
import {
  notServed,
  sandboxAgentTools,
  startupToolSet,
  type ToolDef,
  type ToolSet
} from "./mcp-sandbox.js";
import {
  previewChecks,
  previewCreate,
  previewDelete,
  previewFailed,
  previewList,
  previewShow
} from "./preview.js";
import {
  parseVariables,
  previewRunGet,
  previewRunsList,
  previewSubmitRun,
  requireRunKind,
  runFailed
} from "./preview-runs.js";
import { publish } from "./publish.js";

/**
 * Package version, reported in the MCP handshake.
 *
 * Read from `package.json` rather than restated, so a release bump cannot
 * leave the server announcing a version nobody shipped.
 */
const VERSION: string = (() => {
  try {
    const here = dirname(fileURLToPath(import.meta.url));
    for (const candidate of [
      resolve(here, "..", "package.json"),
      resolve(here, "..", "..", "package.json")
    ]) {
      if (existsSync(candidate)) {
        return (
          (JSON.parse(readFileSync(candidate, "utf8")) as { version?: string }).version ?? "0.0.0"
        );
      }
    }
  } catch {
    // A packaging shape we did not anticipate. The handshake needs a string,
    // not an accurate one.
  }
  return "0.0.0";
})();

/**
 * A tool result. MCP wants content blocks; everything here is text, because
 * the caller is a language model and the payloads are JSON or markdown.
 */
function text(body: string, isError = false) {
  return { content: [{ type: "text" as const, text: body }], isError };
}

/**
 * A tool result carrying one JSON document — the same shape the matching
 * verb's `--json` prints, so a tool's result and the CLI's output never
 * drift apart. On failure the exit-code class is appended, same spelling as
 * the generic catch-all below (`[exit N NAME]`), so an agent branches on it
 * the way it would on `oxyc`'s own exit code.
 */
function jsonResult(data: unknown, opts: { isError?: boolean; code?: ExitCodeValue } = {}) {
  const body = JSON.stringify(data);
  if (!opts.isError) return text(body);
  const code = opts.code ?? ExitCode.FAILURE;
  return text(`${body}\n\n[exit ${code} ${exitCodeName(code)}]`, true);
}

const TOOLS = [
  {
    name: "oxy_routes",
    description:
      "List the API endpoints this Oxy deployment mounts, with what each one does. " +
      "ALWAYS call this before oxy_request rather than guessing a path. " +
      "Pass a filter (matched against method, path, surface and description) to narrow it — " +
      "unfiltered is ~670 routes. Above 60 matches the descriptions are dropped and only " +
      "method, path and credential come back, so narrow enough to stay under that when you " +
      "need to know what an endpoint does; past ~400 it is refused outright.",
    inputSchema: {
      type: "object",
      properties: {
        filter: {
          type: "string",
          description: "Substring to narrow by, e.g. 'threads', 'sql', 'admin', 'semantic'."
        },
        all: {
          type: "boolean",
          description:
            "Include ide-only and worker-only routes. Off by default: those are mounted on one " +
            "instance of the fleet, so a caller hitting the load balancer cannot reach them directly."
        }
      }
    }
  },
  {
    name: "oxy_schema",
    description:
      "The request and response schema for ONE endpoint, so a body can be built correctly. " +
      "Covers the data plane (SQL, semantic query, and the org/workspace lookups); a blank " +
      "result means undocumented, not nonexistent — use oxy_routes to confirm the endpoint exists.",
    inputSchema: {
      type: "object",
      properties: {
        path: { type: "string", description: "Endpoint path, e.g. '{workspace}/sql/query'." },
        method: { type: "string", description: "Narrow to one HTTP method." }
      },
      required: ["path"]
    }
  },
  {
    name: "oxy_request",
    description:
      "Make an authenticated request to the Oxy API. The credential is picked from the path " +
      "(/api/** takes a bearer, /external/api/** an API key). Placeholders {org}, {workspace}, " +
      "{project}, {customer} and {me} are substituted from context. " +
      "Read freely; ask the human before any mutating request against production.",
    inputSchema: {
      type: "object",
      properties: {
        path: { type: "string", description: "Path relative to /api, e.g. '{org}/workspaces'." },
        method: {
          type: "string",
          description: "HTTP method. Defaults to GET, or POST with fields."
        },
        fields: {
          type: "object",
          description:
            "Request parameters. Sent as a JSON body on POST/PUT/PATCH, or as query parameters " +
            "on GET/HEAD/DELETE. Values keep their JSON type."
        },
        jq: {
          type: "string",
          description:
            "A jq program to reduce the response before it is returned. Use it — a full list " +
            "response is far more context than the two fields you need."
        }
      },
      required: ["path"]
    }
  },
  {
    name: "oxy_whoami",
    description:
      "Which deployment this is pointed at and who the token belongs to. " +
      "Use it when a call returns 401/403, or a 200 with a null body — on this API an expired " +
      "session answers 200 null rather than 401, and this tells the two apart.",
    inputSchema: { type: "object", properties: {} }
  }
];

/**
 * THE SANDBOX LOOP, AS TOOLS — the deliberate exception to "four, not one per
 * endpoint" above. A generic `oxy_request` can call these same routes, but it
 * cannot carry the per-operation validation that makes the loop safe for an
 * agent to drive unsupervised: the dev-<handle> grammar, the confirm-before-
 * delete gate, and the hard refusal of a sandbox publish outside dev-<handle>.
 * Every one of these reuses its CLI verb's own request-building function —
 * see `env.ts` / `fn.ts` / `checks.ts` / `invocations.ts` / `logs.ts` /
 * `publish.ts` — so there is exactly one implementation of each request, and
 * the tool result is the same JSON document that verb's `--json` prints.
 *
 * Each description says what it returns AND what to call next, so the loop
 * reads off the tool list alone: env_create → publish_sandbox → fn_call /
 * checks_run → invocations_list / invocations_held / logs → iterate from
 * publish_sandbox → env_delete.
 */
const SANDBOX_TOOLS = [
  {
    name: "oxy_env_create",
    description:
      "Create a dev-<handle> SANDBOX of a custom app — its own build pointer, storage silo, " +
      "secrets and Airhouse sibling, isolated from production and from every other sandbox. " +
      "Starts with no build. Returns the Environment (status, build, url). Call " +
      "oxy_publish_sandbox next to give it one.",
    inputSchema: {
      type: "object",
      properties: {
        app: { type: "string", description: '"<org-slug>/<app-slug>" or the app UUID.' },
        name: {
          type: "string",
          description: "dev-<handle> — 1-12 lowercase letters, digits and single hyphens."
        }
      },
      required: ["app", "name"]
    }
  },
  {
    name: "oxy_env_list",
    description:
      "List a custom app's environments — production, staging and every dev-<handle> sandbox " +
      "— each with its status and build. Use it to find a sandbox's name before oxy_env_show, " +
      "oxy_publish_sandbox or oxy_env_delete.",
    inputSchema: {
      type: "object",
      properties: {
        app: { type: "string", description: '"<org-slug>/<app-slug>" or the app UUID.' }
      },
      required: ["app"]
    }
  },
  {
    name: "oxy_env_show",
    description:
      "One environment's detail — status, build, url. Call it after oxy_publish_sandbox to " +
      "confirm a build landed, or before oxy_fn_call to confirm one exists.",
    inputSchema: {
      type: "object",
      properties: {
        app: { type: "string", description: '"<org-slug>/<app-slug>" or the app UUID.' },
        name: { type: "string", description: "production, staging, or dev-<handle>." }
      },
      required: ["app", "name"]
    }
  },
  {
    name: "oxy_env_delete",
    description:
      "DESTRUCTIVE — tears down a sandbox's storage silo, secrets and Airhouse sibling for " +
      "good. Requires confirm=true: there is no terminal to ask on here, so the delete refuses " +
      "without it, the same way `oxyc env delete` refuses without --yes off a TTY. Set " +
      "waitSeconds to avoid polling oxy_env_show yourself.",
    inputSchema: {
      type: "object",
      properties: {
        app: { type: "string", description: '"<org-slug>/<app-slug>" or the app UUID.' },
        name: {
          type: "string",
          description: "dev-<handle> — production and staging can never be deleted."
        },
        confirm: { type: "boolean", description: "Must be true, or the call is refused." },
        waitSeconds: {
          type: "number",
          description: "Block until torn down, or this many seconds pass (no wait by default)."
        }
      },
      required: ["app", "name", "confirm"]
    }
  },
  {
    name: "oxy_publish_sandbox",
    description:
      "DESTRUCTIVE (to the sandbox's build, not production) — build the app in `dir` (default: " +
      "the current directory) and publish it to a dev-<handle> sandbox's build pointer. " +
      "REQUIRES a dev-<handle> appEnv and refuses production, staging, or any attempt to " +
      "promote — a sandbox build is never promoted; publish the same tree to staging and " +
      "promote that instead, outside this tool. Runs the app's own build command, so it " +
      "executes whatever package scripts oxy-app.json declares. Returns the publish result " +
      "(build id, url). Call oxy_fn_call or oxy_checks_run with the same appEnv next.",
    inputSchema: {
      type: "object",
      properties: {
        appEnv: {
          type: "string",
          description: "dev-<handle> — anything else (production, staging) is refused."
        },
        dir: { type: "string", description: "App directory to publish (default: cwd)." },
        app: {
          type: "string",
          description: "App slug (default: oxy-app.json's slug, or OXY_APP)."
        },
        org: {
          type: "string",
          description: "Org slug (default: oxy-app.json's orgSlug, or OXY_ORG)."
        }
      },
      required: ["appEnv"]
    }
  },
  {
    name: "oxy_fn_call",
    description:
      "Call a custom app's Oxy Function directly (POST .../fn/<function>) and return its " +
      "result: {ok, status, body, logs, invocationId, error}. isError is set when the " +
      "FUNCTION failed (distinct from a transport failure, which throws). Needs a build in " +
      "that environment — oxy_publish_sandbox or oxy_env_show confirms one exists. Call " +
      "oxy_logs or oxy_invocations_held with the returned invocationId to see why it failed.",
    inputSchema: {
      type: "object",
      properties: {
        app: { type: "string", description: '"<org-slug>/<app-slug>" or the app UUID.' },
        function: { type: "string", description: "The Oxy Function's name." },
        appEnv: { type: "string", description: "production (default), staging, or dev-<handle>." },
        data: { type: "string", description: 'JSON request body (default "{}").' },
        timeoutSeconds: { type: "number", description: "Client-side timeout (default 60)." }
      },
      required: ["app", "function"]
    }
  },
  {
    name: "oxy_checks_run",
    description:
      'Run every function the app marked "check": true in one environment, and wait for each ' +
      "to a terminal status. Returns {app, checks:[{name, status, passed, error, durationMs}]}; " +
      "isError is set when any check failed or timed out. Run this right after " +
      "oxy_publish_sandbox to verify a build.",
    inputSchema: {
      type: "object",
      properties: {
        app: { type: "string", description: '"<org-slug>/<app-slug>" or the app UUID.' },
        appEnv: { type: "string", description: "production (default), staging, or dev-<handle>." },
        timeoutSeconds: { type: "number", description: "Per-check timeout (default 300)." }
      },
      required: ["app"]
    }
  },
  {
    name: "oxy_invocations_list",
    description:
      "List what ran in an app's environment — every function invocation, newest first, with " +
      "its status. Use it to confirm oxy_fn_call or oxy_checks_run reached the sandbox, or to " +
      "find an invocation id for oxy_invocations_held.",
    inputSchema: {
      type: "object",
      properties: {
        app: { type: "string", description: '"<org-slug>/<app-slug>" or the app UUID.' },
        appEnv: { type: "string", description: "Narrow to one environment." },
        build: { type: "string", description: "Narrow to one build (its id, or the build UUID)." },
        function: { type: "string", description: "Narrow to one function." },
        limit: { type: "number", description: "Max rows (default 50)." }
      },
      required: ["app"]
    }
  },
  {
    name: "oxy_invocations_held",
    description:
      "What a non-production invocation's write policy HELD instead of performing — " +
      "production's would-be effect, recorded rather than run. Empty for a production " +
      "invocation, since nothing is held there. Call with an id from oxy_invocations_list or " +
      "oxy_fn_call's invocationId.",
    inputSchema: {
      type: "object",
      properties: {
        app: { type: "string", description: '"<org-slug>/<app-slug>" or the app UUID.' },
        invocationId: { type: "string", description: "From oxy_invocations_list or oxy_fn_call." }
      },
      required: ["app", "invocationId"]
    }
  },
  {
    name: "oxy_logs",
    description:
      "An app's persisted ctx.log() / console.* output, newest-relevant window first. Use it " +
      "to debug a function that oxy_fn_call or oxy_checks_run reported as failed.",
    inputSchema: {
      type: "object",
      properties: {
        app: { type: "string", description: '"<org-slug>/<app-slug>" or the app UUID.' },
        appEnv: {
          type: "string",
          description: "Narrow to one environment (default: production only)."
        },
        invocation: { type: "string", description: "Narrow to one invocation id." },
        request: { type: "string", description: "Narrow to one request id." },
        hours: { type: "number", description: "Window size in hours (default 24, max 168)." },
        limit: { type: "number", description: "Max rows (default 100, max 500)." }
      },
      required: ["app"]
    }
  }
];

/**
 * WORKSPACE PREVIEWS, AS TOOLS — the same exception as `SANDBOX_TOOLS` above,
 * for the same reason: the run-kind grammar and the create/wait/timeout
 * contract are per-operation validation a generic `oxy_request` cannot carry.
 * Each reuses its `oxyc preview` verb's own request-building function (see
 * `preview.ts` / `preview-runs.ts`), so there is one implementation of each
 * request and the tool result matches that verb's `--json` output.
 *
 * The loop: preview_create (waits for the compile) → preview_checks → " +
 * preview_run (procedure or airway_sample) → preview_run_show (waits for the " +
 * run) → preview_runs_list to see transform_build/compare runs the server " +
 * queued on its own → preview_delete when done.
 */
const PREVIEW_TOOLS = [
  {
    name: "oxy_preview_create",
    description:
      "Preview a workspace branch: compile its head into a staging revision (or reuse the " +
      "ready one for that commit) and start serving it under x-oxy-preview-revision. Returns " +
      "the PreviewItem ({branch, status, revision_id, checks, ...}) — idempotent, so calling it " +
      "again on the same commit is free. Set waitSeconds to avoid polling oxy_preview_show " +
      "yourself; isError is then set on a compile that failed. Call oxy_preview_checks next.",
    inputSchema: {
      type: "object",
      properties: {
        branch: { type: "string", description: "The branch to preview. Not the default branch." },
        waitSeconds: {
          type: "number",
          description:
            "Block until the compile is ready or failed, up to this many seconds (no wait by default)."
        }
      },
      required: ["branch"]
    }
  },
  {
    name: "oxy_preview_list",
    description:
      "List every branch staff are previewing in this workspace, most recently touched first, " +
      "each with its compile status and checks summary. Use it to find a branch name before " +
      "the other preview tools.",
    inputSchema: { type: "object", properties: {} }
  },
  {
    name: "oxy_preview_show",
    description:
      "One branch's preview — status and checks summary (PreviewItem). Not a server route: " +
      "filters oxy_preview_list's result by branch, and fails NOT_FOUND when the branch has " +
      "no preview.",
    inputSchema: {
      type: "object",
      properties: { branch: { type: "string", description: "The previewed branch." } },
      required: ["branch"]
    }
  },
  {
    name: "oxy_preview_delete",
    description:
      "DESTRUCTIVE — stop previewing a branch: cancels its queued runs and releases its " +
      "staging revision. Requires confirm=true (there is no terminal to ask on here).",
    inputSchema: {
      type: "object",
      properties: {
        branch: { type: "string", description: "The previewed branch to stop previewing." },
        confirm: { type: "boolean", description: "Must be true, or the call is refused." }
      },
      required: ["branch", "confirm"]
    }
  },
  {
    name: "oxy_preview_checks",
    description:
      "The Airway change check of a preview's current revision: each changed .airway.yml " +
      "pipeline and whether merging it needs a Reset schema, and each changed automation " +
      '("auto" transforms are built in the preview and compared with live; "manual" ones say ' +
      'why not). "pending" with no pipelines until the check has run — call oxy_preview_show ' +
      "or retry shortly.",
    inputSchema: {
      type: "object",
      properties: { branch: { type: "string", description: "The previewed branch." } },
      required: ["branch"]
    }
  },
  {
    name: "oxy_preview_run",
    description:
      "Start a held dry run of a previewed branch: a procedure (every write held or, on the " +
      "managed Airhouse, redirected into the preview's own schemas) or an airway_sample (a " +
      "bounded window of a pipeline into the preview's schemas). transform_build and compare " +
      "runs are queued by the server's own change check and cannot be started here — read " +
      "them back with oxy_preview_runs_list / oxy_preview_run_show instead. Returns " +
      "{run_id, state}. Set waitSeconds to avoid polling oxy_preview_run_show yourself; isError " +
      "is then set if the run failed or was cancelled. Needs OXY_PREVIEW_RUNS enabled on the " +
      "deployment.",
    inputSchema: {
      type: "object",
      properties: {
        branch: { type: "string", description: "The previewed branch to run against." },
        kind: { type: "string", description: '"procedure" or "airway_sample".' },
        ref: {
          type: "string",
          description: "The automation path (procedure) or .airway.yml path (airway_sample)."
        },
        variables: {
          type: "string",
          description: "procedure: JSON object of the automation's variables."
        },
        readLiveOnly: {
          type: "boolean",
          description: "procedure: read live tables even where the preview holds a copy."
        },
        windowFrom: { type: "string", description: "airway_sample: window start, RFC 3339." },
        windowTo: { type: "string", description: "airway_sample: window end, RFC 3339." },
        resources: {
          type: "array",
          items: { type: "string" },
          description: "airway_sample: the resources to read, for a multi-resource source."
        },
        waitSeconds: {
          type: "number",
          description: "Block until the run finishes, up to this many seconds (no wait by default)."
        }
      },
      required: ["branch", "kind", "ref"]
    }
  },
  {
    name: "oxy_preview_runs_list",
    description:
      "A branch's held runs, newest first — procedure, transform_build, compare and " +
      "airway_sample alike, with each one's state and outcome. Use it to find a run_id for " +
      "oxy_preview_run_show, including the transform_build/compare runs the server queues on " +
      "its own that oxy_preview_run cannot start.",
    inputSchema: {
      type: "object",
      properties: { branch: { type: "string", description: "The previewed branch." } },
      required: ["branch"]
    }
  },
  {
    name: "oxy_preview_run_show",
    description:
      "One run's detail: its steps (with what each held or redirected), and — for " +
      "transform_build/compare — the compare-with-live outcome, or — for airway_sample — the " +
      "sample's ask and result. Set waitSeconds to avoid polling yourself; isError is then set " +
      "if the run failed or was cancelled.",
    inputSchema: {
      type: "object",
      properties: {
        runId: { type: "string", description: "From oxy_preview_run or oxy_preview_runs_list." },
        waitSeconds: {
          type: "number",
          description: "Block until the run finishes, up to this many seconds (no wait by default)."
        }
      },
      required: ["runId"]
    }
  }
];

const ALL_TOOLS: ToolDef[] = [...TOOLS, ...SANDBOX_TOOLS, ...PREVIEW_TOOLS];

/** The list a session serves — decided once, from its credential (`mcp-sandbox.ts`). */
function toolsFor(set: ToolSet): ToolDef[] {
  return set === "sandbox_agent" ? sandboxAgentTools(SANDBOX_TOOLS) : ALL_TOOLS;
}

/**
 * Above this many matches, `oxy_routes` drops the descriptions.
 *
 * DEGRADES, IT DOES NOT REFUSE — the same shape as `oxyc routes`, which picks
 * `renderDetailed` under 40 matches and the compact method+path table above it
 * (`discover.ts`). The expensive field is `description`, not the row: a trimmed
 * row is a few tens of bytes, so several hundred still fit in a result an agent
 * can read, while the descriptions are what turn a broad match into hundreds of
 * KB of context spent before a question is asked.
 *
 * The threshold is on the COUNT, not on whether a `filter` was supplied — a
 * filter is not evidence of narrowing. `searchRoutes` matches a substring of
 * method, path, surface or description, so `"admin"`, `"workspace"` and
 * `"query"` are all filters a model reaches for in good faith that clear this.
 */
const MAX_DESCRIBED_ROUTES = 60;

/**
 * And above THIS many, there is nothing useful to return at all.
 *
 * Deliberately far above the threshold that drops descriptions: refusing is the
 * answer only when even the trimmed listing would be a context dump, which for
 * a deployment of ~670 routes means "you asked for effectively all of them".
 */
const MAX_ROUTES_PER_RESULT = 400;

/** `{ a: 1 }` → the `-f`/`-F` pairs `parseFields` understands. */
function fieldPairs(fields: Record<string, unknown> | undefined): string[] {
  if (!fields) return [];
  return Object.entries(fields).map(([k, v]) => `${k}=${JSON.stringify(v)}`);
}

/**
 * Check the arguments the schema says are required.
 *
 * `inputSchema.required` is ADVISORY on the low-level `Server` — it is handed
 * to the model, not enforced by the transport. Without this, `oxy_schema` with
 * no `path` matched every path and returned the whole OpenAPI document, and
 * `oxy_request` with none built a request to `/api/`.
 */
function requireArgs(name: string, args: Record<string, unknown>, required: string[]): void {
  const missing = required.filter((k) => {
    const v = args[k];
    return v === undefined || v === null || (typeof v === "string" && v.trim() === "");
  });
  if (missing.length > 0) {
    throw new CliError(`${name} needs ${missing.join(", ")}`, { code: ExitCode.USAGE });
  }
}

async function callTool(ctx: Context, name: string, args: Record<string, unknown>) {
  switch (name) {
    case "oxy_routes": {
      const catalog = await loadCatalog({ target: ctx.target(), bearer: await ctx.maybeBearer() });
      const filter = (args.filter as string | undefined)?.trim();
      let matches = searchRoutes(catalog, filter || undefined);
      if (!args.all) matches = matches.filter((r) => r.role === "fleet-ok");

      // A cache stale enough to be wrong is worth saying INSIDE the result:
      // `loadCatalog` warns on stderr, which an MCP client never shows anyone.
      const staleNote = catalog.stale
        ? "NOTE: this route table came from a stale local cache; the deployment was unreachable.\n\n"
        : "";

      const asked = filter ? ` ${JSON.stringify(filter)}` : "";

      // Only a match set too large even to LIST is refused. Below that the
      // result degrades instead, because a filter like "admin" or "query" can
      // clear the description threshold in good faith and "guess something
      // narrower" is a dead end for a model with no way to see what it hit.
      if (matches.length > MAX_ROUTES_PER_RESULT) {
        return text(
          `${staleNote}${matches.length} routes match${asked}, which is effectively the whole ` +
            `deployment. Call oxy_routes again with a narrower \`filter\` (matched against ` +
            `method, path, surface and description), e.g. "threads", "sql", "semantic", "admin".`,
          true
        );
      }

      if (matches.length === 0) {
        // An empty array reads as "this deployment has no such endpoint",
        // which is usually false — the filter was wrong, or the route is
        // ide-only and hidden by default.
        return text(
          `${staleNote}No route matches ${JSON.stringify(filter ?? "")}. Try a broader filter, ` +
            `or pass all=true to include ide-only and worker-only mounts.`,
          true
        );
      }
      // Trimmed to the fields that help a caller build a request. The full
      // record carries the handler path and mount notes, which are for a
      // maintainer reading source, not for an agent composing a call.
      const described = matches.length <= MAX_DESCRIBED_ROUTES;
      // `credential` SURVIVES THE TRIM. It is what separates the bearer mount
      // from the `/external/api` API-key one, so a caller composing a request
      // needs it in either form — and `renderCompact`, the CLI shape this
      // mirrors, keeps it as a per-surface heading. It is also not the
      // expensive field: ~25 bytes a row, ~10 KB across the whole ceiling,
      // against descriptions that run to hundreds of KB.
      const trimmed = matches.map((r) =>
        described
          ? { method: r.method, path: r.path, credential: r.credential, description: r.description }
          : { method: r.method, path: r.path, credential: r.credential }
      );
      // The note says the listing is trimmed AND how to get the descriptions
      // back. Without it a model reads a description-less row as an endpoint
      // that has no documentation, rather than one it did not ask narrowly
      // enough to see.
      const trimNote = described
        ? ""
        : `NOTE: ${matches.length} routes match${asked} — too many to describe, so each row is ` +
          `method, path and credential WITHOUT its description. Narrow the \`filter\` to ` +
          `${MAX_DESCRIBED_ROUTES} or fewer matches to get the descriptions back.\n\n`;
      return text(`${staleNote}${trimNote}${JSON.stringify(trimmed)}`);
    }

    case "oxy_schema": {
      requireArgs("oxy_schema", args, ["path"]);
      const doc = (await loadOpenApi({
        target: ctx.target(),
        bearer: await ctx.maybeBearer()
      })) as {
        paths?: Record<string, Record<string, unknown>>;
        components?: unknown;
      };
      const wanted = typeof args.path === "string" ? args.path : "";
      const method = args.method as string | undefined;
      // `comparablePath` is imported from `discover.ts` rather than copied:
      // a second reduction of the same two spellings is a second thing to keep
      // in step, and the copy here was untested. Exact first, then substring —
      // the order `runSchema` uses, so the two commands answer alike.
      const paths = Object.entries(doc.paths ?? {});
      const exact = paths.filter(([p]) => comparablePath(p) === comparablePath(wanted));
      const hit =
        exact.length > 0
          ? exact
          : paths.filter(([p]) => comparablePath(p).includes(comparablePath(wanted)));
      if (hit.length === 0) {
        return text(
          `No documented schema for ${wanted}. The OpenAPI document covers the data plane only — ` +
            `call oxy_routes with a filter to confirm the endpoint exists.`,
          true
        );
      }
      const selected = Object.fromEntries(
        hit.map(([p, ops]) => [
          p,
          method
            ? Object.fromEntries(
                Object.entries(ops).filter(([verb]) => verb.toLowerCase() === method.toLowerCase())
              )
            : ops
        ])
      );
      return text(JSON.stringify({ paths: selected, components: doc.components }, null, 2));
    }

    case "oxy_request": {
      requireArgs("oxy_request", args, ["path"]);
      const raw = typeof args.path === "string" ? args.path : "";
      const method =
        (typeof args.method === "string" ? args.method.toUpperCase() : "") || undefined;
      const fields = parseFields([], fieldPairs(args.fields as Record<string, unknown>));
      const verb = method ?? (fields.present ? "POST" : "GET");
      const carriesBody = !["GET", "HEAD", "DELETE"].includes(verb);

      let path = substitutePlaceholders(normalizePath(raw), ctx.placeholders());
      let body: string | undefined;
      if (fields.present) {
        if (carriesBody) body = JSON.stringify(fields.params);
        else {
          const query = paramsToQuery(fields.params);
          if (query) path += (path.includes("?") ? "&" : "?") + query;
        }
      }

      const apiKey = ctx.apiKey();
      const external = isExternalSurface(path);
      const response = await request({
        target: ctx.target(),
        path,
        method: verb,
        body,
        bearer: external && apiKey ? ctx.storedBearer() : await ctx.bearer(),
        apiKey
      });

      if (response.status < 200 || response.status >= 300) {
        // The STATUS and the body, not a thrown error: the model can act on
        // "403, here is what the server said" and cannot act on a transport
        // exception. It is marked `isError` so the runtime shows it as one.
        return text(`HTTP ${response.status} ${response.statusText}\n${response.body}`, true);
      }

      if (response.body.trim() === "null") {
        return text(
          "null\n\n(NOTE: a 200 with a null body can mean an expired session on this API, " +
            "not 'no such thing'. Call oxy_whoami to tell them apart.)"
        );
      }

      const jq = args.jq as string | undefined;
      if (jq) return text(runJq(response.body, jq));
      return text(response.body);
    }

    case "oxy_whoami": {
      const target = ctx.target();
      const bearer = await ctx.bearer();
      // `/api/user` answers a sandbox agent token 404. Its own description is
      // the answer, asked fresh: this tool is what tells a dead token from a
      // live one.
      if (isSandboxAgentToken(bearer)) {
        const described = await describeSandboxToken(target, bearer, { fresh: true });
        return text(JSON.stringify({ target, token: parseJson(described.body) }, null, 2));
      }
      const response = await request({
        target,
        path: "/api/user",
        method: "GET",
        bearer
      });
      const payload = parseJson(response.body);
      if (payload === null || payload === undefined) {
        return text(
          `target: ${target}\nThe token no longer resolves to a user — the session has expired. ` +
            `Ask the human to run \`oxyc login\`.`,
          true
        );
      }
      return text(JSON.stringify({ target, user: payload }, null, 2));
    }

    // ── sandbox loop ───────────────────────────────────────────────────────

    case "oxy_env_create": {
      requireArgs("oxy_env_create", args, ["app", "name"]);
      return jsonResult(await envCreate(ctx, String(args.app), String(args.name)));
    }

    case "oxy_env_list": {
      requireArgs("oxy_env_list", args, ["app"]);
      return jsonResult({ environments: await envList(ctx, String(args.app)) });
    }

    case "oxy_env_show": {
      requireArgs("oxy_env_show", args, ["app", "name"]);
      return jsonResult(await envShow(ctx, String(args.app), String(args.name)));
    }

    case "oxy_env_delete": {
      requireArgs("oxy_env_delete", args, ["app", "name"]);
      if (args.confirm !== true) {
        throw usageError(
          "oxy_env_delete needs confirm=true",
          "there is no terminal to ask on here — pass confirm=true once you mean it"
        );
      }
      const result = await envDelete(ctx, String(args.app), String(args.name), {
        yes: true,
        waitSeconds: args.waitSeconds === undefined ? undefined : Number(args.waitSeconds)
      });
      return jsonResult(result);
    }

    case "oxy_publish_sandbox": {
      requireArgs("oxy_publish_sandbox", args, ["appEnv"]);
      // Client-side, before any build or request — the same grammar `oxyc
      // publish --app-env` validates, and the same refusal of "production" /
      // "staging": a sandbox publish tool that accepted either would be the
      // promote path this tool exists to foreclose.
      const appEnv = requireSandboxName(String(args.appEnv));
      // `org` is a GLOBAL flag (`resolveIdentity` reads `ctx.flags.org`), not a
      // `PublishFlags` field — a fresh context carries it the way `--org`
      // would on the CLI, without mutating the one `runMcp` was given.
      const org = args.org as string | undefined;
      const publishCtx = org === undefined ? ctx : createContext({ ...ctx.flags, org }, ctx.cwd);
      const outcome = await publish(publishCtx, {
        appEnv,
        dir: args.dir as string | undefined,
        app: args.app as string | undefined
      });
      return jsonResult(outcome.result ?? { bundle_dir: outcome.bundleDir });
    }

    case "oxy_fn_call": {
      requireArgs("oxy_fn_call", args, ["app", "function"]);
      const result = await fnCall(ctx, String(args.app), String(args.function), {
        appEnv: args.appEnv as string | undefined,
        data: args.data as string | undefined,
        timeoutSeconds: args.timeoutSeconds === undefined ? 60 : Number(args.timeoutSeconds)
      });
      return jsonResult(result, { isError: !result.ok, code: ExitCode.FAILURE });
    }

    case "oxy_checks_run": {
      requireArgs("oxy_checks_run", args, ["app"]);
      const report = await runChecksCore(ctx, String(args.app), {
        timeoutSeconds: args.timeoutSeconds === undefined ? 300 : Number(args.timeoutSeconds),
        appEnv: args.appEnv as string | undefined
      });
      const failed = report.checks.some((c) => !c.passed);
      return jsonResult(report, { isError: failed, code: ExitCode.CHECK_FAILED });
    }

    case "oxy_invocations_list": {
      requireArgs("oxy_invocations_list", args, ["app"]);
      const invocations = await invocationsList(ctx, String(args.app), {
        appEnv: args.appEnv as string | undefined,
        build: args.build as string | undefined,
        fn: args.function as string | undefined,
        limit: args.limit === undefined ? undefined : Number(args.limit)
      });
      return jsonResult({ invocations });
    }

    case "oxy_invocations_held": {
      requireArgs("oxy_invocations_held", args, ["app", "invocationId"]);
      return jsonResult(await invocationsHeld(ctx, String(args.app), String(args.invocationId)));
    }

    case "oxy_logs": {
      requireArgs("oxy_logs", args, ["app"]);
      const logs = await fetchLogs(ctx, String(args.app), {
        appEnv: args.appEnv as string | undefined,
        invocation: args.invocation as string | undefined,
        request: args.request as string | undefined,
        hours: args.hours === undefined ? undefined : Number(args.hours),
        limit: args.limit === undefined ? undefined : Number(args.limit)
      });
      return jsonResult({ logs });
    }

    // ── sandbox agent token only (`mcp-sandbox.ts`) ─────────────────────────

    case "oxy_env_secret_list": {
      requireArgs(name, args, ["app", "appEnv"]);
      return jsonResult(await secretList(ctx, String(args.app), String(args.appEnv)));
    }

    case "oxy_env_secret_set": {
      requireArgs(name, args, ["app", "appEnv", "key", "value"]);
      return jsonResult(
        await secretSet(
          ctx,
          String(args.app),
          String(args.appEnv),
          String(args.key),
          String(args.value)
        )
      );
    }

    case "oxy_env_secret_delete": {
      requireArgs(name, args, ["app", "appEnv", "key"]);
      return jsonResult(
        await secretDelete(ctx, String(args.app), String(args.appEnv), String(args.key))
      );
    }

    case "oxy_token_revoke": {
      if (args.confirm !== true) {
        throw usageError(
          "oxy_token_revoke needs confirm=true",
          "it ends this token for good — pass confirm=true once the sandbox is deleted and the task is done"
        );
      }
      const target = ctx.target();
      const outcome = await revokeCallingToken(target, await ctx.bearer());
      forgetSandboxTokens();
      if (outcome === "revoked" || outcome === "already_invalid") {
        return jsonResult({ revoked: true, already: outcome === "already_invalid" });
      }
      throw new CliError(`${target} did not confirm the revoke`, {
        code: ExitCode.UNAVAILABLE,
        hint: "call oxy_token_revoke once more; the token also ends on its own at its expiry"
      });
    }

    // ── workspace previews ──────────────────────────────────────────────────

    case "oxy_preview_create": {
      requireArgs("oxy_preview_create", args, ["branch"]);
      const item = await previewCreate(ctx, String(args.branch), {
        waitSeconds: args.waitSeconds === undefined ? undefined : Number(args.waitSeconds)
      });
      return jsonResult(item, { isError: previewFailed(item), code: ExitCode.FAILURE });
    }

    case "oxy_preview_list":
      return jsonResult({ items: await previewList(ctx) });

    case "oxy_preview_show": {
      requireArgs("oxy_preview_show", args, ["branch"]);
      return jsonResult(await previewShow(ctx, String(args.branch)));
    }

    case "oxy_preview_delete": {
      requireArgs("oxy_preview_delete", args, ["branch"]);
      if (args.confirm !== true) {
        throw usageError(
          "oxy_preview_delete needs confirm=true",
          "there is no terminal to ask on here — pass confirm=true once you mean it"
        );
      }
      return jsonResult(await previewDelete(ctx, String(args.branch), { yes: true }));
    }

    case "oxy_preview_checks": {
      requireArgs("oxy_preview_checks", args, ["branch"]);
      return jsonResult(await previewChecks(ctx, String(args.branch)));
    }

    case "oxy_preview_run": {
      requireArgs("oxy_preview_run", args, ["branch", "kind", "ref"]);
      const kind = requireRunKind(String(args.kind));
      const windowFrom = args.windowFrom as string | undefined;
      const windowTo = args.windowTo as string | undefined;
      const submitted = await previewSubmitRun(ctx, {
        branch: String(args.branch),
        kind,
        ref: String(args.ref),
        variables: parseVariables(args.variables as string | undefined),
        readLiveOnly: args.readLiveOnly as boolean | undefined,
        window:
          windowFrom === undefined && windowTo === undefined
            ? undefined
            : { from: windowFrom, to: windowTo },
        resources: (args.resources as string[] | undefined) ?? []
      });
      const waitSeconds = args.waitSeconds === undefined ? undefined : Number(args.waitSeconds);
      if (waitSeconds === undefined) return jsonResult(submitted);
      const detail = await previewRunGet(ctx, submitted.run_id, { waitSeconds });
      return jsonResult(detail, { isError: runFailed(detail), code: ExitCode.FAILURE });
    }

    case "oxy_preview_runs_list": {
      requireArgs("oxy_preview_runs_list", args, ["branch"]);
      return jsonResult({ runs: await previewRunsList(ctx, String(args.branch)) });
    }

    case "oxy_preview_run_show": {
      requireArgs("oxy_preview_run_show", args, ["runId"]);
      const detail = await previewRunGet(ctx, String(args.runId), {
        waitSeconds: args.waitSeconds === undefined ? undefined : Number(args.waitSeconds)
      });
      return jsonResult(detail, { isError: runFailed(detail), code: ExitCode.FAILURE });
    }

    default:
      return text(`unknown tool: ${name}`, true);
  }
}

/**
 * Serve MCP on stdio until the client disconnects.
 *
 * STDIO IS THE WHOLE TRANSPORT, deliberately. An HTTP/SSE server would need a
 * port, a lifetime and an auth story of its own; stdio is what every agent
 * runtime already launches, and the process inherits the caller's environment
 * — which is where the token and the target come from.
 *
 * NOTHING MAY BE WRITTEN TO STDOUT except protocol frames. `log.info` and
 * friends already go to stderr, which is why this can share the CLI's own
 * plumbing without corrupting the stream.
 */
export async function runMcp(ctx: Context): Promise<void> {
  // Every request from here on is a tool call's, and its user agent says so
  // (`… mcp`) — the startup check below included.
  markMcpSession();

  // BEFORE THE TRANSPORT IS UP. A missing or dead credential ends the process
  // with exit 4 here, where an agent runtime reports "the server failed to
  // start" — rather than a session whose every tool fails, or one that runs
  // on a credential nobody chose (`startupToolSet`).
  const set = await startupToolSet(ctx);
  const tools = toolsFor(set);
  const served = new Set(tools.map((tool) => tool.name));

  const server = new Server({ name: "oxyc", version: VERSION }, { capabilities: { tools: {} } });

  server.setRequestHandler(ListToolsRequestSchema, async () => ({ tools }));

  server.setRequestHandler(CallToolRequestSchema, async (req) => {
    // A tool left off the list is not callable by name either: the list is the
    // surface, not a suggestion.
    if (!served.has(req.params.name)) return text(notServed(req.params.name, set), true);
    try {
      return await callTool(
        ctx,
        req.params.name,
        (req.params.arguments ?? {}) as Record<string, unknown>
      );
    } catch (cause) {
      // A throw here would kill the session. The model can act on a message;
      // it cannot act on a dead transport.
      const message = (cause as Error).message ?? String(cause);
      // BOTH CHANNELS. A model gets one string, so there is no marker to
      // distinguish them — but reading `hint` alone drops the one line that
      // says what to DO, which is the half a model can act on.
      const { hint, remedy } = cause as { hint?: string; remedy?: string };
      // Blank line between them, the way every other renderer of these two
      // separates them — a model given two consecutive imperatives with no
      // boundary loses the distinction the second field exists to make.
      // `!== undefined` for the narrowing TS gives it, and a length check
      // because an empty string would open the block with a blank line —
      // `filter(Boolean)` covered that and cost the narrowing, so both.
      const extra = [hint, remedy].filter((line) => line !== undefined && line !== "").join("\n\n");
      // THE EXIT-CODE CLASS, APPENDED — not substituted for the message, and
      // not only on the sandbox/preview tools: every CliError carries a code,
      // so every tool's failure now lets a caller branch the way it would on
      // `oxyc`'s own exit code, by NAME rather than a digit it has to look up.
      // The server's own `code` field (previews, sandboxes) rides along when
      // the response carried one.
      const codeLine =
        cause instanceof CliError
          ? `[exit ${cause.code} ${exitCodeName(cause.code)}]` +
            (cause.serverCode ? ` server code: ${cause.serverCode}` : "")
          : undefined;
      const body = [message, extra, codeLine]
        .filter((s) => s !== undefined && s !== "")
        .join("\n\n");
      return text(body, true);
    }
  });

  await server.connect(new StdioServerTransport());

  // `connect` returns once the transport is wired; the process must stay up
  // until the client closes stdin.
  await new Promise<void>((resolve) => {
    process.stdin.on("close", resolve);
    server.onclose = resolve;
  });
}
