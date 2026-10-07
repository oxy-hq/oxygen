/**
 * `oxyc guide` — teach an agent this tool, in one command, in any harness.
 *
 * THE GAP THIS FILLS. There are already three ways an LLM can learn `oxyc`,
 * and each reaches a different audience:
 *
 *   1. the tool describes itself — `--help`, `routes`, `schema`, `exit-codes`.
 *      Works for ANY agent with no setup, and is the foundation. But it is
 *      pull-based: the agent has to already suspect `oxyc` is the right tool.
 *   2. the bundled Claude skill (`oxyc skills install`). Rich, but Claude-only
 *      and behind an install step.
 *   3. error messages that name the next command. The highest-bandwidth
 *      channel, because it arrives exactly when the agent is stuck — but only
 *      once it is already stuck.
 *
 * None of them puts "here is what this tool is for" into an agent's context
 * BEFORE it needs it, in a harness-agnostic way. That is what this is: a
 * compact page a human pastes into `AGENTS.md`, `CLAUDE.md`, `.cursorrules`,
 * a system prompt, or whatever their agent reads.
 *
 * COMPLEMENTS `oxyc mcp` rather than competing with it. MCP reaches an agent
 * whose runtime can launch a server; this reaches every other one — a plain
 * shell, a CI job, a harness with no MCP support — and it costs nothing per
 * turn beyond the lines it occupies.
 *
 * KEPT SHORT ON PURPOSE. Something that lands in a context window on every
 * turn has to earn each line; the detail lives behind `routes` and `schema`.
 */

import { basename, dirname, extname, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { stdoutIsTty } from "../ui/tty.js";

/**
 * How to invoke THIS binary, resolved at runtime.
 *
 * The guide is a page a human pastes verbatim into `AGENTS.md`, so a `<repo>`
 * placeholder is a line an agent runs as-is and gets
 * `Cannot find module '<repo>/…'`. Until the package is published there is no
 * short spelling, so the honest one is the absolute path this process was
 * started from — which the binary knows.
 *
 * `process.argv[1]` is that path, and after a global install it is the
 * `bin/oxyc` symlink rather than `dist/main.mjs`. `node <symlink>` works on
 * POSIX because npm links the JS file itself, so the line stays runnable —
 * it just will not be the path the prose above it describes.
 */
function selfInvocation(): string {
  // A compiled single-file binary IS the executable: there is no `node` and no
  // `dist/main.mjs`. Printing one would hand an agent a path that does not
  // exist, which is the one thing this page cannot afford to do.
  const runner = basename(process.execPath, extname(process.execPath));
  if (runner !== "node" && runner !== "bun") return "oxyc";

  const self = fileURLToPath(import.meta.url);
  // From `dist/main.mjs` at runtime; from `src/commands/guide.ts` under vitest.
  const dist = self.includes(`${sep}dist${sep}`)
    ? process.argv[1] || self
    : resolve(dirname(self), "..", "..", "dist", "main.mjs");
  return `node ${dist}`;
}

const GUIDE = () => `## oxyc — the Oxy CLI

Authenticated HTTP client for the Oxy platform (\`gh api\`-shaped), plus the
tooling that scopes work to one customer. Use it to get real data out of a
deployment: query a customer's warehouse or semantic layer, read threads, runs
and apps, or reproduce a reported bug against live data.

    ${selfInvocation()} <command>

If your runtime speaks MCP, \`oxyc mcp\` serves the whole API as four tools
(\`oxy_routes\`/\`oxy_schema\`/\`oxy_request\`/\`oxy_whoami\`), plus purpose-built tools for
the sandbox loop (\`oxy_env_*\`, \`oxy_publish_sandbox\`, \`oxy_fn_call\`, \`oxy_checks_run\`,
\`oxy_invocations_*\`, \`oxy_logs\`) and workspace previews (\`oxy_preview_*\`) below. Its token
is \`OXY_TOKEN\` only — unset is exit 4; \`oxyc mcp --login\` serves on your own login.

### Never guess a path

    oxyc routes <filter>           # what exists, and what each endpoint does
    oxyc schema <path> [-X POST]   # the body it expects
    oxyc api <path> [flags]        # call it

### Getting data out

    oxyc api orgs --md                                  # org ids
    oxyc api {org}/workspaces --md                      # workspace ids
    oxyc api {workspace}/databases --jq '.[].name'      # connection names
    oxyc api {workspace}/sql/query -f 'sql=select 1' -f database=<name> --md

\`{org}\` \`{workspace}\` \`{project}\` \`{customer}\` \`{me}\` fill themselves from the customer
repo you are in, or from \`--org\`/\`--workspace\`/\`--project\`; unresolved errors, naming the flag.

### Flags worth knowing

    -f k=v / -F k=v   string / JSON-typed field    --input @file   raw body
    --jq '<expr>'     filter server-side shape     --md            markdown table
    --paginate        walk every page              --cache 5m      reuse a recent GET
    --env local|dev|staging|production, or paste a URL

Prefer \`--jq\`/\`--md\` before reading a large response — \`--md\` is far fewer tokens than JSON, which repeats every field name per row.

### Branch on the exit code

    0 ok · 1 it ran and found problems · 2 you called it wrong, stop
    4 not authenticated — a person logs in (\`oxyc login --env <env>\`); an agent on its own token STOPS and reports: the token ended
    5 not found · 6 malformed request · 7 retryable (5xx/timeout) · 8 refused · 9 a check failed (\`oxyc checks run\`)

### An agent's own token — never the person's \`oxyc login\`

    eval "$(oxyc tokens create --sandbox-agent --app <org>/<app> --env <deployment>)"   # to build a custom app in a sandbox: those sandboxes, nothing else
    eval "$(oxyc tokens create --agent --env <deployment>)"                             # everything else: what your operator can reach, for hours
    oxyc tokens revoke --current --env <deployment>                                     # when the task is done: end it

Your operator approves once in the browser. Then pass \`--token-env OXY_TOKEN\` to every command: one that lost the variable exits 4 instead of running on their cached login.

### Sandboxes — try a custom-app change on real data (\`--env <deployment>\` says where; omitted, it is production)

    oxyc env create <app> dev-x --env <deployment>                           # starts with no build
    oxyc publish --app-env dev-x --env <deployment>                          # build + publish to it
    oxyc fn call <app> <fn> --app-env dev-x --data '{}' --env <deployment>   # call a function in it
    oxyc checks run <app> --app-env dev-x --env <deployment>                 # run its checks
    oxyc invocations held <app> <invocation-id> --env <deployment>           # what it held, not wrote
    oxyc env delete <app> dev-x --yes --wait --env <deployment>              # done; tears its homes down

### Workspace previews — open a branch on real data without it being live (staff)

    oxyc preview create <branch> --wait              # compile it, wait for the revision
    oxyc preview checks <branch>                     # Airway change check of that revision
    oxyc preview run <branch> procedure <automation.yml> --wait   # held dry run
    oxyc preview runs list <branch>                  # includes transform_build/compare too
    oxyc preview delete <branch> --yes                # done

### Beyond the API

    oxyc validate                  # check the workspace YAML — no network, no token
    oxyc proxy --env dev           # local app dev against cloud data
    oxyc login-link --next /ide    # one-time URL that signs a browser in as your token — navigate to it, no OAuth
    oxyc apps list|show|builds|health|usage|drift   # custom apps: live build, health, usage — read-only
    oxyc <customer>                # a session scoped to one customer
    oxyc assume start --org <o> -r "why"   # staff/partner session, 60 min, not renewable

\`oxyc validate\` is the only one offline; \`oxy validate\` (Rust) also resolves \`databases:\`/\`llm.ref\`.

### Traps

- \`200\` with a body of \`null\` can mean an EXPIRED SESSION — \`oxyc whoami\` tells it from "no such thing".
- \`/sql/query\` returns arrays of strings, HEADER ROW FIRST, not an object — \`--md\` renders it.
- \`oxyc schema\` covers the data plane only; blank means undocumented — \`oxyc routes <path>\` confirms it's real.
- A listed route can still 404 if it is \`ide-only\`; \`oxyc routes --all\` shows those.
- Read freely. Ask before a mutating request against production, or \`--app-env\` other than your own sandbox.
- On your own token (\`--sandbox-agent\` or \`--agent\`), exit 4 means it expired or was revoked. Never fall back to another credential, a cached \`oxyc login\` included.
`;

/**
 * Print the guide.
 *
 * Markdown either way — it is meant to be pasted into a file, and a terminal
 * reader is going to copy it rather than read it in place.
 */
export function runGuide(): void {
  process.stdout.write(GUIDE());
  if (stdoutIsTty()) {
    process.stderr.write("\nPaste that into AGENTS.md / CLAUDE.md, or: oxyc guide >> AGENTS.md\n");
  }
}
