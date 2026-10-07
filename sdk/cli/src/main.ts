/**
 * `oxyc` — the command tree.
 *
 * Two halves under one root, joined by `createContext`:
 *
 *   the PLATFORM half   api / routes / schema / openapi / login / whoami
 *   the CUSTOMER half   list / path / new / import / doctor / …
 *
 * HELP IS SHORT ON PURPOSE. The Rust `oxy api --help` appended all ~600 routes
 * to its epilogue, which made `--help` a 400-line document — unreadable for a
 * human and, for the agent this tool is built for, several thousand tokens
 * spent before the first request. Discovery lives in `oxyc routes <filter>`,
 * which answers the same question and can be narrowed.
 *
 * This file is also the ONLY place that calls `process.exit`, so every path
 * out of the program goes through one renderer and one exit-code decision.
 */

import { Command } from "commander";
import { clearAllCaches, unknownCacheEntries } from "./api/cache.js";
import { runExitRevokes } from "./auth/exit-revoke.js";
import { runActivity } from "./commands/activity.js";
import { runApi } from "./commands/api.js";
import {
  runAppsBuilds,
  runAppsHealth,
  runAppsList,
  runAppsShow,
  runAppsUsage
} from "./commands/apps.js";
import { runAppsDrift } from "./commands/apps-drift.js";
import { runAssumeEnd, runAssumeStart, runAssumeStatus } from "./commands/assume.js";
import { runLogin, runLogout, runToken, runWhoami } from "./commands/auth.js";
import { runChecks } from "./commands/checks.js";
import { runList, runPath } from "./commands/customers.js";
import { runOpenApi, runRoutes, runSchema } from "./commands/discover.js";
import { runEnvCreate, runEnvDelete, runEnvList, runEnvShow } from "./commands/env.js";
import { runEnvSecretDelete, runEnvSecretList, runEnvSecretSet } from "./commands/env-secrets.js";
import { runFnCall } from "./commands/fn.js";
import { runGuide } from "./commands/guide.js";
import { runInitCi } from "./commands/init-ci.js";
import { runInvocationsHeld, runInvocationsList } from "./commands/invocations.js";
import { runLaunch } from "./commands/launch.js";
import { runLoginLink } from "./commands/login-link.js";
import { runLogs } from "./commands/logs.js";
import { runOltpProvision, runOltpStatus } from "./commands/oltp.js";
import { runOltpReset } from "./commands/oltp-branch.js";
import {
  runPreviewChecks,
  runPreviewCreate,
  runPreviewDelete,
  runPreviewList,
  runPreviewShow
} from "./commands/preview.js";
import { runPreviewRun, runPreviewRunShow, runPreviewRunsList } from "./commands/preview-runs.js";
import { runProxy } from "./commands/proxy.js";
import { runPublish } from "./commands/publish.js";
import { runImport, runNew, runRemove } from "./commands/registry.js";
import { runRepos } from "./commands/repos.js";
import { runSkillsInstall, runSkillsList } from "./commands/skills.js";
import { runTokensCreate, runTokensList, runTokensRevoke } from "./commands/tokens.js";
import { runTokensCreateSandboxAgent, runTokensRevokeCurrent } from "./commands/tokens-sandbox.js";
import { runValidate } from "./commands/validate.js";
import { runAdopt, runDoctor, runUpdate } from "./commands/workspace.js";
import { createContext, type GlobalFlags } from "./context/resolve.js";
import { VERSION } from "./generated/version.js";
import * as log from "./ui/log.js";
import { err } from "./ui/tty.js";
import { CliError, ExitCode, usageError } from "./util/errors.js";
import { didYouMeanCommand } from "./util/suggest.js";

/** Flags every command accepts, declared once and attached to each. */
function withGlobals(command: Command): Command {
  return (
    command
      .option("--env <name|url>", "environment or URL to target", "production")
      .option("--target <url>", "explicit base URL; overrides --env")
      // NO COMMANDER DEFAULT, on purpose: `createContext` has to tell "named
      // a variable" from "said nothing", because naming one turns the login
      // cache off as a fallback. A default here would make every run look named.
      .option(
        "--token-env <VAR>",
        "env var holding the bearer (default: OXY_TOKEN). Named, it is the only source: unset is an auth error, never your login"
      )
      .option(
        "--api-key-env <VAR>",
        "env var holding the legacy API key or API token for /external/api",
        "OXY_API_KEY"
      )
      .option(
        "--service-account <id>",
        "in GitHub Actions: the ID of the service account to act as via OIDC"
      )
      .option("--org <slug>", "value for the {org} placeholder")
      .option("--workspace <id>", "value for the {workspace} placeholder")
      .option("--project <id>", "value for the {project} placeholder")
      .option("--customer <name>", "act as though run inside this customer's repo")
      // No `-q` short form: `gh api` spells the jq filter `-q, --jq`, and that
      // spelling is worth more than a short flag for quiet.
      .option("--quiet", "suppress progress messages on stderr")
  );
}

/** Turn commander's parsed options into the shape the context wants. */
/**
 * `--login-env a --login-env b` and `--login-env a,b` are the same set.
 *
 * Commander appends per occurrence; splitting on commas inside the reducer is
 * what makes the two spellings equivalent, which is what the Rust `oxy login`
 * accepted and therefore what a reader coming from it will type.
 */
function collectEnvs(value: string, previous: string[]): string[] {
  const parts = value
    .split(",")
    .map((v) => v.trim())
    .filter(Boolean);
  return [...previous, ...parts];
}

function globals(opts: Record<string, unknown>): GlobalFlags {
  if (opts.quiet) process.env.OXYC_QUIET = "1";
  return {
    env: opts.env as string | undefined,
    target: opts.target as string | undefined,
    tokenEnv: opts.tokenEnv as string | undefined,
    apiKeyEnv: opts.apiKeyEnv as string | undefined,
    org: opts.org as string | undefined,
    workspace: opts.workspace as string | undefined,
    project: opts.project as string | undefined,
    customer: opts.customer as string | undefined,
    refresh: opts.refresh as boolean | undefined,
    serviceAccount: opts.serviceAccount as string | undefined
  };
}

function buildProgram(): Command {
  const program = new Command("oxyc")
    .description("The Oxy CLI — talk to the API, and work on a customer account.")
    // From `package.json`, via `scripts/emit-version.mjs`. It was a literal, so
    // `oxyc --version` was a second copy of the version that a release bump did
    // not touch — worst for the standalone binary, where `--version` is the
    // only way to tell what you have.
    .version(VERSION)
    .showHelpAfterError("(run `oxyc --help`)")
    .configureOutput({
      // ONLY `writeErr` is redirected, and the distinction matters. commander
      // uses `writeOut` for an explicit `--help` / `--version` — a SUCCESSFUL
      // invocation whose help text IS the answer, so `oxyc api --help | less`
      // and `oxyc routes --help | grep workspaces` have to work. It uses
      // `writeErr` for usage errors and help-after-error, which must not land
      // in the middle of piped JSON. `gh` splits them the same way.
      //
      // The stdout-purity rule this defends is about *data* output; no failure
      // path reaches `writeOut`.
      writeErr: (str) => process.stderr.write(str)
    });

  // ── the platform half ────────────────────────────────────────────────────

  withGlobals(
    program
      .command("api")
      .argument("<path>", "path, relative to /api (a leading / or api/ is fine)")
      .description("make an authenticated request to the Oxy API")
      .option("-X, --method <verb>", "HTTP method (default GET, or POST with a body)")
      .option("-f, --raw-field <key=value>", "string parameter", collect, [])
      .option("-F, --field <key=value>", 'typed parameter (true/3/["a"]/@file/@-)', collect, [])
      .option("-H, --header <name:value>", "extra header", collect, [])
      .option("--input <file|->", "raw request body from a file, or - for stdin")
      .option("-q, --jq <expr>", "filter the response through jq")
      .option("--md", "render an array of objects as a markdown table")
      .option("--paginate", "follow every page and return one document")
      .option("--paginate-key <field>", "the field holding the rows, when the guess is wrong")
      .option("--max-pages <n>", "stop after n pages (default 100)")
      .option("--slurp", "with --paginate, emit an array of pages instead of merging")
      .option("--cache <duration>", "reuse a recent successful GET (30s, 5m, 2h)")
      .option("-i, --include", "print the status line and response headers")
      .option("--silent", "make the request, print nothing")
      .option("--verbose", "log the request before making it")
      .option("--timeout <duration>", "request timeout (default 2m)")
      .addHelpText("after", API_HELP)
  ).action(async (path: string, opts: Record<string, unknown>) => {
    const ctx = createContext(globals(opts));
    await runApi(ctx, path, {
      method: opts.method as string | undefined,
      rawField: opts.rawField as string[],
      field: opts.field as string[],
      header: opts.header as string[],
      input: opts.input as string | undefined,
      jq: opts.jq as string | undefined,
      md: opts.md as boolean | undefined,
      paginate: opts.paginate as boolean | undefined,
      paginateKey: opts.paginateKey as string | undefined,
      maxPages: opts.maxPages as string | undefined,
      slurp: opts.slurp as boolean | undefined,
      cache: opts.cache as string | undefined,
      include: opts.include as boolean | undefined,
      silent: opts.silent as boolean | undefined,
      verbose: opts.verbose as boolean | undefined,
      timeout: opts.timeout as string | undefined
    });
  });

  withGlobals(
    program
      .command("routes")
      .argument("[filter]", "narrow by method, path, surface or description")
      .description("list the endpoints this deployment mounts")
      .option("--json", "emit the matching endpoints as a JSON array")
      .option("--all", "include ide-only and worker-only mounts")
      .option("--refresh", "ask the deployment again instead of using the cache")
  ).action(async (filter: string | undefined, opts: Record<string, unknown>) => {
    const ctx = createContext(globals(opts));
    await runRoutes(ctx, filter, {
      json: opts.json as boolean | undefined,
      all: opts.all as boolean | undefined,
      refresh: opts.refresh as boolean | undefined
    });
  });

  withGlobals(
    program
      .command("schema")
      .argument("<path>", "the endpoint whose request/response shape you want")
      .description("request and response schemas for one endpoint")
      .option("-X, --method <verb>", "narrow to one HTTP method")
  ).action(async (path: string, opts: Record<string, unknown>) => {
    await runSchema(createContext(globals(opts)), path, opts.method as string | undefined);
  });

  withGlobals(program.command("openapi").description("the whole OpenAPI document")).action(
    async (opts: Record<string, unknown>) => {
      await runOpenApi(createContext(globals(opts)));
    }
  );

  // ── auth ─────────────────────────────────────────────────────────────────

  withGlobals(
    program
      .command("login")
      .description("authenticate against a deployment, in the browser")
      // REPEATABLE AND COMMA-SPLIT, as the Rust `oxy login`'s `--env` was. The
      // browser opens once per env, in sequence — `--login-env dev,staging` is
      // three acts, not one act with three targets. It is a separate flag
      // rather than making `--env` repeatable because `--env` is global here
      // and every other command takes exactly one.
      .option(
        "--login-env <name|url...>",
        "extra deployments to log into (repeat, or comma-separate)",
        collectEnvs,
        [] as string[]
      )
      .option("--assume [slug|uuid|url]", "act as this org immediately after logging in")
      .option("-r, --reason <why>", "why you are acting as that org — recorded in the audit log")
  ).action(async (opts: Record<string, unknown>) => {
    const extra = (opts.loginEnv as string[]) ?? [];
    // The positional `--env` is one of the set, not a separate default. No
    // `??` here: `withGlobals` already defaults the option and `createContext`
    // defaults it again — a third spelling would be dead.
    const envs = extra.length > 0 ? [String(opts.env), ...extra] : [];
    const assumeFlag = opts.assume;

    // EVERY USAGE ERROR BEFORE ANY BROWSER OPENS. These were checked inside
    // `runLogin`, after each env had already been through its flow — so
    // `--login-env staging --assume acme -r why` opened two browsers, waited
    // for two callbacks, and then exited USAGE. Checked here because this is
    // the only place that sees the flags before the work.
    if (assumeFlag !== undefined && !opts.reason) {
      throw usageError("--assume requires --reason", "it is recorded in the impersonation log");
    }
    if (opts.reason && assumeFlag === undefined) {
      throw usageError(
        "--reason is only valid with --assume",
        "a reason with nothing to act as would have started a session for no org"
      );
    }
    if (envs.length > 1 && assumeFlag !== undefined) {
      throw usageError(
        "--assume is only valid when logging into a single env",
        "one session names one org on one deployment"
      );
    }
    if (envs.length > 1 && opts.target) {
      throw usageError(
        "--target is only valid when logging into a single env",
        "a single override would silently apply to all of them"
      );
    }

    const assume =
      assumeFlag === undefined
        ? undefined
        : {
            // A bare `--assume` means "the org `--env` already names".
            org: typeof assumeFlag === "string" && assumeFlag ? assumeFlag : undefined,
            reason: String(opts.reason)
          };
    await runLogin(createContext(globals(opts)), envs, assume);
  });

  const assume = program
    .command("assume")
    .description("act as an organization — staff and partner sessions, 60 minutes");

  withGlobals(
    assume
      .command("start")
      .description("begin acting as an org — --org takes a slug, a UUID, or an org URL")
      // `--org` comes from `withGlobals` — the placeholder flag and the org
      // being assumed are the same value, and two spellings for one idea is
      // how a user ends up passing the wrong one.
      .requiredOption("-r, --reason <why>", "why — recorded in the impersonation log")
  ).action(async (opts: Record<string, unknown>) => {
    await runAssumeStart(
      createContext(globals(opts)),
      opts.org as string | undefined,
      String(opts.reason)
    );
  });

  withGlobals(
    assume
      .command("status")
      .description("the assume-role sessions live for your account, and the time left")
      .option("--json", "emit the raw session objects")
  ).action(async (opts: Record<string, unknown>) => {
    await runAssumeStatus(createContext(globals(opts)), Boolean(opts.json));
  });

  withGlobals(
    assume
      .command("end")
      .description("stop acting — one org, or every live session")
      // Both together refuse. Silently preferring one is the wrong call when
      // the verb is destructive: a caller who typed both meant something, and
      // neither reading is safe to guess.
      .option("--all", "end every live session (refuses alongside --org)")
  ).action(async (opts: Record<string, unknown>) => {
    if (opts.all && opts.org) {
      throw usageError(
        "--all and --org name different sets",
        "drop one — ending every session and ending one are different acts"
      );
    }
    await runAssumeEnd(
      createContext(globals(opts)),
      opts.org as string | undefined,
      Boolean(opts.all)
    );
  });

  const oltp = program
    .command("oltp")
    .description(
      "an org's OLTP database (ctx.oltp) — status, provisioning and its staging branch, for staff"
    );

  withGlobals(
    oltp
      .command("status")
      .description("every org's OLTP state — or, with --org, one org's store and writers")
      .option("--json", "emit the server's response")
  ).action(async (opts: Record<string, unknown>) => {
    await runOltpStatus(
      createContext(globals(opts)),
      opts.org as string | undefined,
      Boolean(opts.json)
    );
  });

  withGlobals(
    oltp
      .command("provision")
      .description("create or reconcile an org's OLTP database and writers — a billable resource")
      // `--org` comes from `withGlobals`, as it does for `assume`.
      .option("--writer <app:slug|pipeline:source>", "a writer to ensure (repeatable)", collect, [])
      .option(
        "--branch <staging>",
        "also cut the org's staging branch of the database (one per org; manual by design)"
      )
      .option("--yes", "provision without asking")
      .option("--json", "emit the server's response")
  ).action(async (opts: Record<string, unknown>) => {
    await runOltpProvision(
      createContext(globals(opts)),
      opts.org as string | undefined,
      opts.writer as string[],
      {
        yes: opts.yes as boolean | undefined,
        json: opts.json as boolean | undefined,
        branch: opts.branch as string | undefined
      }
    );
  });

  withGlobals(
    oltp
      .command("reset")
      .description(
        "re-copy an org's staging branch from production — discards every app's staging data"
      )
      .option("--branch <staging>", "the branch to reset (required; production is never reset)")
      .option("--yes", "reset without asking")
      .option("--json", "emit the server's response")
  ).action(async (opts: Record<string, unknown>) => {
    await runOltpReset(createContext(globals(opts)), opts.org as string | undefined, {
      branch: opts.branch as string | undefined,
      yes: opts.yes as boolean | undefined,
      json: opts.json as boolean | undefined
    });
  });

  withGlobals(
    program.command("logout").description("revoke and drop the cached token for a deployment")
  ).action(async (opts: Record<string, unknown>) => {
    await runLogout(createContext(globals(opts)));
  });

  withGlobals(
    program
      .command("whoami")
      .description("who the cached token is, checked against the deployment")
      .option("--json", "emit the raw /api/user response")
  ).action(async (opts: Record<string, unknown>) => {
    await runWhoami(createContext(globals(opts)), Boolean(opts.json));
  });

  withGlobals(
    program.command("token").description("print the bearer token, for a raw curl")
  ).action(async (opts: Record<string, unknown>) => {
    await runToken(createContext(globals(opts)));
  });

  withGlobals(
    program
      .command("login-link")
      .description("print a one-time URL that signs a browser in as this token")
      .option("--next <path>", "where the browser lands once signed in, e.g. /ide")
      .option("--json", "emit { url, expires_at, session_seconds }")
      .option("--open", "also open it in your browser, which uses the link up")
      .addHelpText(
        "after",
        "\nFor a browser nobody is sitting at. An agent cannot finish Google/GitHub OAuth or\n" +
          "read a magic-link inbox; with this the whole sign-in is one navigation:\n\n" +
          '    browser_navigate("$(oxyc login-link --env dev --next /ide)")\n\n' +
          "Stdout is the URL and nothing else. The link works once and expires in minutes.\n" +
          "The session it opens can do what the token can do and nothing more, cannot manage\n" +
          "tokens, and ends when the token is revoked. It needs a personal access token\n" +
          "(oxy_pat_…) — what `oxyc login` stores.\n"
      )
  ).action(async (opts: Record<string, unknown>) => {
    await runLoginLink(createContext(globals(opts)), {
      next: opts.next as string | undefined,
      json: Boolean(opts.json),
      open: Boolean(opts.open)
    });
  });

  // The PLURAL manages; the singular above prints the one in use.
  const tokens = program
    .command("tokens")
    .description("personal access tokens — list, revoke, or open the page that creates one");

  withGlobals(
    tokens
      .command("list")
      .description("your personal access tokens (needs a browser-session login)")
      .option("--json", "emit the raw /api/user/tokens response")
  ).action(async (opts: Record<string, unknown>) => {
    await runTokensList(createContext(globals(opts)), Boolean(opts.json));
  });

  withGlobals(
    tokens
      .command("revoke")
      .argument("[id]", "the token's id, from `oxyc tokens list`")
      .description(
        "revoke one of your tokens by id (needs a browser-session login), or --current: the one in use"
      )
      .option("--current", "revoke the token this command is running with (OXY_TOKEN)")
  ).action(async (id: string | undefined, opts: Record<string, unknown>) => {
    const ctx = createContext(globals(opts));
    if (opts.current) {
      if (id !== undefined) {
        throw usageError("pass an <id> or --current, not both", "--current is the token in use");
      }
      await runTokensRevokeCurrent(ctx);
      return;
    }
    if (id === undefined) {
      throw usageError(
        "oxyc tokens revoke needs an <id>, or --current",
        "`oxyc tokens list` shows the ids; --current ends the token this command runs with"
      );
    }
    await runTokensRevoke(ctx, id);
  });

  withGlobals(
    tokens
      .command("create")
      .description(
        "open Account → Personal access tokens in the browser — or, with --sandbox-agent, mint an agent's token"
      )
      .option(
        "--sandbox-agent",
        "mint a sandbox agent token (oxy_sbx_…): approved once in the browser, printed as `export OXY_TOKEN=…`"
      )
      .option("--app <org>/<app>", "with --sandbox-agent: an app it reaches (1 to 5)", collect, [])
      .option("--hours <n>", "with --sandbox-agent: its lifetime, 1 to 168 (default 8)")
      .option(
        "--name <label>",
        "with --sandbox-agent: its name in the token list and the audit log"
      )
      .addHelpText(
        "after",
        "\nA sandbox agent token does the sandbox loop on the named apps and nothing else:\n" +
          "create up to three dev-<handle> sandboxes, publish into them, call their functions,\n" +
          "run their checks, read them back, set their secrets, delete them. It is never\n" +
          "stored on this machine and it revokes no other token.\n\n" +
          '    eval "$(oxyc tokens create --sandbox-agent --app acme/store --env dev)"\n'
      )
  ).action(async (opts: Record<string, unknown>) => {
    const ctx = createContext(globals(opts));
    const apps = opts.app as string[];
    if (opts.sandboxAgent) {
      await runTokensCreateSandboxAgent(ctx, {
        apps,
        hours: opts.hours as string | undefined,
        name: opts.name as string | undefined
      });
      return;
    }
    // The three flags mean nothing without it, and silently opening the page
    // would look like the mint had been asked for.
    if (apps.length > 0 || opts.hours !== undefined || opts.name !== undefined) {
      throw usageError(
        "--app, --hours and --name are only valid with --sandbox-agent",
        "plain `oxyc tokens create` opens the page that creates a personal access token"
      );
    }
    runTokensCreate(ctx);
  });

  const checks = program
    .command("checks")
    .description('run an app\'s checks (functions marked "check": true)');
  withGlobals(
    checks
      .command("run <app>")
      .description("run every check of <org>/<app> (or an app id) and wait for results")
      .option("--json", "emit results as JSON")
      .option("--timeout <seconds>", "per-check timeout", "300")
      .option("--app-env <environment>", "run against this environment instead of production")
  ).action(async (app: string, opts: Record<string, unknown>) => {
    await runChecks(createContext(globals(opts)), app, {
      json: Boolean(opts.json),
      timeoutSeconds: Number(opts.timeout),
      appEnv: opts.appEnv as string | undefined
    });
  });

  const env = program
    .command("env")
    .description("a custom app's environments — production, staging, and dev-<handle> sandboxes");
  withGlobals(
    env
      .command("create <app> <name>")
      .description("create a sandbox (dev-<handle>) — starts with no build")
      .option("--json", "emit the Environment as JSON")
  ).action(async (app: string, name: string, opts: Record<string, unknown>) => {
    await runEnvCreate(createContext(globals(opts)), app, name, { json: Boolean(opts.json) });
  });
  withGlobals(
    env
      .command("list <app>")
      .description("list an app's environments — production, staging, every sandbox")
      .option("--json", "emit {environments} as JSON")
  ).action(async (app: string, opts: Record<string, unknown>) => {
    await runEnvList(createContext(globals(opts)), app, { json: Boolean(opts.json) });
  });
  withGlobals(
    env
      .command("show <app> <name>")
      .description("one environment's detail — production, staging or dev-<handle>")
      .option("--json", "emit the Environment as JSON")
  ).action(async (app: string, name: string, opts: Record<string, unknown>) => {
    await runEnvShow(createContext(globals(opts)), app, name, { json: Boolean(opts.json) });
  });
  withGlobals(
    env
      .command("delete <app> <name>")
      .description("delete a sandbox — tears down its storage, secrets and Airhouse sibling")
      .option("--yes", "delete without asking")
      .option("--wait [seconds]", "poll until the teardown finishes (default 120s)")
      .option("--json", "emit the result as JSON")
  ).action(async (app: string, name: string, opts: Record<string, unknown>) => {
    await runEnvDelete(createContext(globals(opts)), app, name, {
      yes: opts.yes as boolean | undefined,
      waitSeconds:
        opts.wait === undefined ? undefined : opts.wait === true ? 120 : Number(opts.wait),
      json: Boolean(opts.json)
    });
  });

  const envSecret = env
    .command("secret")
    .description("a sandbox's own secrets — keys and flags, set, delete; never a value");
  withGlobals(
    envSecret
      .command("list <app>")
      .description("the sandbox's secret keys: which are set, required, or read from staging")
      .requiredOption("--app-env <environment>", "the dev-<handle> sandbox")
      .option("--json", "emit the server's list as JSON")
  ).action(async (app: string, opts: Record<string, unknown>) => {
    await runEnvSecretList(createContext(globals(opts)), app, {
      appEnv: opts.appEnv as string | undefined,
      json: Boolean(opts.json)
    });
  });
  withGlobals(
    envSecret
      .command("set <app> <key>")
      .description("set one secret in the sandbox, e.g. a third party's sandbox key")
      .requiredOption("--app-env <environment>", "the dev-<handle> sandbox")
      .option("--value <text>", "the value")
      .option(
        "--value-env <VAR>",
        "read the value from this env var, keeping it off the command line"
      )
      .option("--json", "emit {key, environment, status} as JSON")
  ).action(async (app: string, key: string, opts: Record<string, unknown>) => {
    await runEnvSecretSet(createContext(globals(opts)), app, key, {
      appEnv: opts.appEnv as string | undefined,
      value: opts.value as string | undefined,
      valueEnv: opts.valueEnv as string | undefined,
      json: Boolean(opts.json)
    });
  });
  withGlobals(
    envSecret
      .command("delete <app> <key>")
      .description("delete the sandbox's own value; reads fall back to staging's")
      .requiredOption("--app-env <environment>", "the dev-<handle> sandbox")
      .option("--json", "emit {key, environment, status} as JSON")
  ).action(async (app: string, key: string, opts: Record<string, unknown>) => {
    await runEnvSecretDelete(createContext(globals(opts)), app, key, {
      appEnv: opts.appEnv as string | undefined,
      json: Boolean(opts.json)
    });
  });

  const fn = program.command("fn").description("call a custom app's Oxy Function directly");
  withGlobals(
    fn
      .command("call <app> <function>")
      .description("POST .../fn/<function> (SSE) and print its result")
      .option("--app-env <environment>", "call it in this environment instead of production")
      .option("--data <json|@file|->", 'the request body (default "{}")')
      .option("--timeout <seconds>", "client-side timeout", "60")
      .option("--json", "emit the result as JSON")
  ).action(async (app: string, fnName: string, opts: Record<string, unknown>) => {
    await runFnCall(createContext(globals(opts)), app, fnName, {
      appEnv: opts.appEnv as string | undefined,
      data: opts.data as string | undefined,
      json: Boolean(opts.json),
      timeoutSeconds: Number(opts.timeout)
    });
  });

  const apps = program
    .command("apps")
    .description("custom apps on a deployment: what is registered, live, healthy and used");

  withGlobals(
    apps
      .command("list")
      .description("every custom app you can see, across organizations")
      // `--org` comes from `withGlobals`, as it does for `assume` and `oltp`.
      .option("--published", "only apps with a live build")
      .option("--draft", "only apps with nothing published")
      .option("--builds", "add each app's live build: id, commit, publisher (one request per app)")
      .option("--json", "emit the rows as a JSON array")
      .addHelpText("after", "\n--org <slug> limits the list to one organization.\n")
  ).action(async (opts: Record<string, unknown>) => {
    await runAppsList(createContext(globals(opts)), {
      org: opts.org as string | undefined,
      published: opts.published as boolean | undefined,
      draft: opts.draft as boolean | undefined,
      builds: opts.builds as boolean | undefined,
      json: opts.json as boolean | undefined
    });
  });

  withGlobals(
    apps
      .command("show <app>")
      .description("one app (<org>/<app> or an app id): builds, health, availability, usage")
      .option("--json", "emit one JSON object")
  ).action(async (app: string, opts: Record<string, unknown>) => {
    await runAppsShow(createContext(globals(opts)), app, Boolean(opts.json));
  });

  withGlobals(
    apps
      .command("builds <app>")
      .description("an app's build history, newest first, marking the live and the draft build")
      .option("--json", "emit the server's response")
  ).action(async (app: string, opts: Record<string, unknown>) => {
    await runAppsBuilds(createContext(globals(opts)), app, Boolean(opts.json));
  });

  withGlobals(
    apps
      .command("health [app]")
      .description(
        "every published app's health — or, with <app>, its integrity checks, availability and errors"
      )
      .option("--needs-attention", "only apps that are down, degraded or not measured")
      .option("--json", "emit the report as JSON")
  ).action(async (app: string | undefined, opts: Record<string, unknown>) => {
    await runAppsHealth(createContext(globals(opts)), app, {
      needsAttention: opts.needsAttention as boolean | undefined,
      json: opts.json as boolean | undefined
    });
  });

  withGlobals(
    apps
      .command("usage <app>")
      .description("an app's last 7 days: views, visitors and tracked events")
      .option("--json", "emit one JSON object")
  ).action(async (app: string, opts: Record<string, unknown>) => {
    await runAppsUsage(createContext(globals(opts)), app, Boolean(opts.json));
  });

  withGlobals(
    apps
      .command("drift [app]")
      .description("commits in a local checkout that touch an app after its live build's commit")
      .option("--dir <path>", "use this checkout instead of the one found on this machine")
      .option("--refresh", "rescan for checkouts instead of using the cache")
      .option("--json", "emit the report as JSON")
      .addHelpText(
        "after",
        "\nReports, per app: in sync, N commits ahead, or unknown with the reason — the\n" +
          "commit was not recorded, the repository is not checked out here, the checkout\n" +
          "does not have the commit, no oxy-app.json matches, the commit is not on the\n" +
          "checked-out branch, or the app directory has uncommitted changes. Unknown is\n" +
          "never reported as in sync.\n" +
          "\nCompares against the checkout's CURRENT branch as it is on disk. Nothing is\n" +
          "fetched and no checkout is changed: `git fetch` first if you want the remote's\n" +
          "state. With no <app>, every published app is compared (--org <slug> for one\n" +
          "organization), and --dir applies to the apps built from that checkout's origin.\n"
      )
  ).action(async (app: string | undefined, opts: Record<string, unknown>) => {
    await runAppsDrift(createContext(globals(opts)), app, {
      dir: opts.dir as string | undefined,
      org: opts.org as string | undefined,
      refresh: opts.refresh as boolean | undefined,
      json: opts.json as boolean | undefined
    });
  });

  const invocations = program
    .command("invocations")
    .description("what ran in an app's environment, and what a sandbox held");
  withGlobals(
    invocations
      .command("list <app>")
      .description("every function's invocations, newest first")
      .option("--app-env <environment>", "narrow to one environment")
      .option("--build <id>", "narrow to one build (its id, or the build UUID)")
      .option("--function <name>", "narrow to one function")
      .option("--limit <n>", "max rows (default 50)")
      .option("--json", "emit {invocations} as JSON")
  ).action(async (app: string, opts: Record<string, unknown>) => {
    await runInvocationsList(createContext(globals(opts)), app, {
      appEnv: opts.appEnv as string | undefined,
      build: opts.build as string | undefined,
      fn: opts.function as string | undefined,
      limit: opts.limit === undefined ? undefined : Number(opts.limit),
      json: Boolean(opts.json)
    });
  });
  withGlobals(
    invocations
      .command("held <app> <invocation-id>")
      .description("the writes the non-production policy held, instead of performing them")
      .option("--json", "emit the held list as JSON")
  ).action(async (app: string, invocationId: string, opts: Record<string, unknown>) => {
    await runInvocationsHeld(createContext(globals(opts)), app, invocationId, {
      json: Boolean(opts.json)
    });
  });

  withGlobals(
    program
      .command("logs <app>")
      .description("an app's persisted ctx.log() / console.* output")
      .option("--app-env <environment>", "narrow to one environment (default: production only)")
      .option("--invocation <id>", "narrow to one invocation")
      .option("--request <id>", "narrow to one request")
      .option("--hours <n>", "window size in hours (default 24, max 168)")
      .option("--limit <n>", "max rows (default 100, max 500)")
      .option("--json", "emit {logs} as JSON")
  ).action(async (app: string, opts: Record<string, unknown>) => {
    await runLogs(createContext(globals(opts)), app, {
      appEnv: opts.appEnv as string | undefined,
      invocation: opts.invocation as string | undefined,
      request: opts.request as string | undefined,
      hours: opts.hours === undefined ? undefined : Number(opts.hours),
      limit: opts.limit === undefined ? undefined : Number(opts.limit),
      json: Boolean(opts.json)
    });
  });

  const preview = program
    .command("preview")
    .description(
      "preview a workspace branch on real data without it being live (staff only; --workspace)"
    );
  withGlobals(
    preview
      .command("create <branch>")
      .description("compile a branch's head into a staging revision and start serving it")
      .option("--wait [seconds]", "block until the compile is ready or failed (default 120s)")
      .option("--json", "emit the PreviewItem as JSON")
  ).action(async (branch: string, opts: Record<string, unknown>) => {
    await runPreviewCreate(createContext(globals(opts)), branch, {
      waitSeconds:
        opts.wait === undefined ? undefined : opts.wait === true ? 120 : Number(opts.wait),
      json: Boolean(opts.json)
    });
  });
  withGlobals(
    preview
      .command("list")
      .description("every branch staff are previewing in this workspace")
      .option("--json", "emit {items} as JSON")
  ).action(async (opts: Record<string, unknown>) => {
    await runPreviewList(createContext(globals(opts)), { json: Boolean(opts.json) });
  });
  withGlobals(
    preview
      .command("show <branch>")
      .description("one branch's preview — status and checks summary")
      .option("--json", "emit the PreviewItem as JSON")
  ).action(async (branch: string, opts: Record<string, unknown>) => {
    await runPreviewShow(createContext(globals(opts)), branch, { json: Boolean(opts.json) });
  });
  withGlobals(
    preview
      .command("delete <branch>")
      .description("stop previewing a branch — cancels its queued runs, releases its revision")
      .option("--yes", "delete without asking")
      .option("--json", "emit the result as JSON")
  ).action(async (branch: string, opts: Record<string, unknown>) => {
    await runPreviewDelete(createContext(globals(opts)), branch, {
      yes: opts.yes as boolean | undefined,
      json: Boolean(opts.json)
    });
  });
  withGlobals(
    preview
      .command("checks <branch>")
      .description("the Airway change check of the preview's current revision")
      .option("--json", "emit the ChecksResponse as JSON")
  ).action(async (branch: string, opts: Record<string, unknown>) => {
    await runPreviewChecks(createContext(globals(opts)), branch, { json: Boolean(opts.json) });
  });
  withGlobals(
    preview
      .command("run <branch> <kind> <ref>")
      .description("start a held dry run — kind is procedure or airway_sample")
      .option("--variables <json|@file|->", "procedure: the automation's variables")
      .option("--read-live-only", "procedure: read live tables even where the preview holds a copy")
      .option("--window-from <iso>", "airway_sample: window start, RFC 3339")
      .option("--window-to <iso>", "airway_sample: window end, RFC 3339")
      .option("--resource <name>", "airway_sample: a resource to read (repeatable)", collect, [])
      .option("--wait [seconds]", "block until the run finishes (default 120s)")
      .option("--json", "emit the result as JSON")
      .addHelpText(
        "after",
        "\ntransform_build and compare runs are queued automatically by the server's change\n" +
          "check, not started here — read them back with `oxyc preview runs show <run-id>`.\n"
      )
  ).action(async (branch: string, kind: string, ref: string, opts: Record<string, unknown>) => {
    await runPreviewRun(createContext(globals(opts)), branch, kind, ref, {
      variables: opts.variables as string | undefined,
      readLiveOnly: opts.readLiveOnly as boolean | undefined,
      windowFrom: opts.windowFrom as string | undefined,
      windowTo: opts.windowTo as string | undefined,
      resource: opts.resource as string[],
      waitSeconds:
        opts.wait === undefined ? undefined : opts.wait === true ? 120 : Number(opts.wait),
      json: Boolean(opts.json)
    });
  });

  const previewRuns = preview.command("runs").description("a previewed branch's held runs");
  withGlobals(
    previewRuns
      .command("list <branch>")
      .description(
        "a branch's runs, newest first — procedure, transform_build, compare, airway_sample"
      )
      .option("--json", "emit {runs} as JSON")
  ).action(async (branch: string, opts: Record<string, unknown>) => {
    await runPreviewRunsList(createContext(globals(opts)), branch, { json: Boolean(opts.json) });
  });
  withGlobals(
    previewRuns
      .command("show <run-id>")
      .description("one run's detail — steps, holds, redirects, and its compare or sample")
      .option("--wait [seconds]", "block until the run finishes (default 120s)")
      .option("--json", "emit the RunDetail as JSON")
  ).action(async (runId: string, opts: Record<string, unknown>) => {
    await runPreviewRunShow(createContext(globals(opts)), runId, {
      waitSeconds:
        opts.wait === undefined ? undefined : opts.wait === true ? 120 : Number(opts.wait),
      json: Boolean(opts.json)
    });
  });

  // ── the customer half ────────────────────────────────────────────────────

  withGlobals(
    program
      .command("list")
      .description("the customers, from the GitHub topic that registers them")
      .option("--refresh", "ask GitHub again rather than serving the hour-long cache")
      .option("--json", "emit the customers as a JSON array")
  ).action((opts: Record<string, unknown>) => {
    runList(createContext(globals(opts)), {
      refresh: opts.refresh as boolean | undefined,
      json: opts.json as boolean | undefined
    });
  });

  withGlobals(
    program
      .command("path")
      .argument("<customer>")
      .description("print where a customer's repo is, and stop")
      .option("--refresh", "ask GitHub again rather than serving the cache")
  ).action((name: string, opts: Record<string, unknown>) => {
    runPath(createContext(globals(opts)), name, { refresh: opts.refresh as boolean | undefined });
  });

  withGlobals(
    program
      .command("new")
      .argument("<customer>", "the repo name to create, e.g. acme-oxy")
      .description("create a new customer workspace repo, tagged and scaffolded")
      .option("--display <name>", "the display name (repo description)")
  ).action((name: string, opts: Record<string, unknown>) => {
    runNew(createContext(globals(opts)), name, { display: opts.display as string | undefined });
  });

  withGlobals(
    program
      .command("import")
      .argument("<org/repo>", "an existing repo to register as a customer workspace")
      .description("tag an EXISTING repo as a customer workspace")
      .option("--clone", "also clone it locally")
  ).action((slug: string, opts: Record<string, unknown>) => {
    runImport(createContext(globals(opts)), slug, { clone: opts.clone as boolean | undefined });
  });

  withGlobals(
    program
      .command("remove")
      .alias("rm")
      .argument("<customer>")
      .description("untag a customer's repo (the repo itself is never deleted)")
      .option("--purge", "also delete the local clone")
      .option("--yes", "confirm --purge without a prompt")
  ).action((name: string, opts: Record<string, unknown>) => {
    runRemove(createContext(globals(opts)), name, {
      purge: opts.purge as boolean | undefined,
      yes: opts.yes as boolean | undefined
    });
  });

  withGlobals(
    program
      .command("update")
      .argument("<customer>")
      .description("report how far a customer's repo has drifted from the template")
      .option("--apply", "rewrite the MANAGED files, instead of only reporting them")
      .option("--diff-all", "show the diff for every role, not just mixed files")
  ).action((name: string, opts: Record<string, unknown>) => {
    runUpdate(createContext(globals(opts)), name, {
      apply: opts.apply as boolean | undefined,
      diffAll: opts.diffAll as boolean | undefined
    });
  });

  withGlobals(
    program
      .command("adopt")
      .argument("<customer>")
      .description("install the managed files an IMPORTED repo lacks, then stamp it")
      .option("--apply", "install them, instead of only reporting them")
  ).action((name: string, opts: Record<string, unknown>) => {
    runAdopt(createContext(globals(opts)), name, { apply: opts.apply as boolean | undefined });
  });

  withGlobals(
    program
      .command("doctor")
      .argument("[customer]")
      .description("report the state of a customer's repo, changing nothing")
      .option("--all", "report every customer")
      .option("--refresh", "ask GitHub again rather than serving the cache")
  ).action((name: string | undefined, opts: Record<string, unknown>) => {
    runDoctor(createContext(globals(opts)), name, { all: opts.all as boolean | undefined });
  });

  withGlobals(
    program
      .command("activity")
      .argument("<customer>")
      .description("merged pull requests for a customer, from their repo and ours")
      .option("--since <YYYY-MM-DD>", "only pull requests merged on or after this date")
      .option("--repo <org/name>", "also search this repo", collect, [])
      .option("--write", "file the record into the customer's dossier")
      .option("--json", "emit the records as JSON")
  ).action((name: string, opts: Record<string, unknown>) => {
    runActivity(createContext(globals(opts)), name, {
      since: opts.since as string | undefined,
      repo: opts.repo as string[],
      write: opts.write as boolean | undefined,
      json: opts.json as boolean | undefined
    });
  });

  withGlobals(
    program
      .command("repos")
      .description("where OUR repos are checked out on this machine")
      .option("--refresh", "rescan instead of using the cache")
  ).action((opts: Record<string, unknown>) => {
    runRepos({ refresh: opts.refresh as boolean | undefined });
  });

  withGlobals(
    program
      .command("launch", { isDefault: false })
      .argument("<customer>")
      .argument("[claude-args...]", "arguments passed through to claude")
      .description("start a Claude Code session scoped to one customer")
      .option("--here", "run in the current directory, granting the customer's repo")
      .option("--dry-run", "print the command instead of running it")
  ).action((name: string, passthrough: string[], opts: Record<string, unknown>) => {
    runLaunch(createContext(globals(opts)), name, {
      here: opts.here as boolean | undefined,
      dryRun: opts.dryRun as boolean | undefined,
      passthrough
    });
  });

  const skills = program.command("skills").description("the Claude skills this package ships");
  skills.command("install").description("link them into ~/.claude/skills").action(runSkillsInstall);
  skills.command("list").description("what ships, and what is linked").action(runSkillsList);

  // ── housekeeping ─────────────────────────────────────────────────────────

  const cache = program.command("cache").description("the on-disk caches");
  cache
    .command("clear")
    .description("drop every cache: responses, route catalogs, customer listings, repo scans")
    .action(() => {
      const cleared = clearAllCaches();
      // Named rather than a count, because "cleared" alone is what let this
      // command claim three caches while clearing one.
      log.info(cleared.length === 0 ? "nothing cached" : `cleared: ${cleared.sort().join(", ")}`);

      // Left alone deliberately — see `unknownCacheEntries`. Reported, because
      // a cache this command does not know about is exactly the omission the
      // old whole-root sweep existed to catch.
      const strays = unknownCacheEntries();
      if (strays.length > 0) {
        log.warn(`left alone (not written by oxyc): ${strays.sort().join(", ")}`);
        log.hint(
          "remove them by hand if they are stale — oxyc will not delete what it did not write"
        );
      }
    });

  withGlobals(
    program
      .command("proxy")
      .description("run a local outbound proxy so a custom app in `pnpm dev` hits cloud data")
      .option("--port <n>", "local port to listen on", "3000")
      .option("--allow-writes", "forward side-effecting calls instead of holding them")
      .option("--allow-events", "forward tracking events instead of dropping them")
      .option("--yes", "confirm proxying to a production target")
      .addHelpText(
        "after",
        "\nGuardrails, on by default: side-effecting calls are HELD, tracking events are\n" +
          "DROPPED, auth endpoints reach the backend unauthenticated so sign-in works, and\n" +
          "the cached token is a fallback that never overrides a real browser session.\n" +
          "\nEach Oxy Function call prints `↳ <status> fn <name>  request_id=…  trace_id=…`.\n"
      )
  ).action(async (opts: Record<string, unknown>) => {
    await runProxy(createContext(globals(opts)), {
      port: opts.port as string | undefined,
      allowWrites: opts.allowWrites as boolean | undefined,
      allowEvents: opts.allowEvents as boolean | undefined,
      yes: opts.yes as boolean | undefined
    });
  });

  withGlobals(
    program
      .command("publish")
      .description("build a custom app and publish its bundle (a draft, unless --promote)")
      .option(
        "--app <slug>",
        "app slug (default: OXY_APP, then oxy-app.json slug, then apps/<org>/<app>/)"
      )
      .option("--build-id <id>", "unique per publish (default: the CI run, else random)")
      .option("--dir <path>", "publish this pre-built directory instead of running the build")
      .option("--promote", "publish straight to the live channel")
      .option("--name <name>", "display name override for the app")
      .option("--repo <url>", "git remote to record (default: the checkout's origin)")
      .option("--commit <sha>", "commit to record (default: HEAD, or GITHUB_SHA)")
      .option("--branch <name>", "branch to record (default: the checkout's, or GITHUB_REF_NAME)")
      .option("--build-only", "build and bundle functions, then stop — no credential needed")
      .option(
        "--prebuilt",
        "with --dir: its functions are already bundled; check them, don't rebuild"
      )
      .option("--json", "print the server's result as JSON")
      .option(
        "--semantic-branch <branch>",
        "draft only: compile this WORKSPACE branch and pin the build's staging preview to it"
      )
      .option(
        "--allow-function-lint",
        "publish past an Oxy Function lint finding; each prints as a warning naming its rule"
      )
      .option(
        "--app-env <dev-handle>",
        "publish to this sandbox instead of the draft/live channel — never --promote"
      )
      .addHelpText(
        "after",
        "\n--org takes a slug or a UUID (default: OXY_ORG, then oxy-app.json orgSlug, then\n" +
          "the apps/<org>/<app>/ directory). --project pins the workspace; otherwise it is\n" +
          "resolved from the target. .env.local and .env are loaded without overriding.\n" +
          "\nAuth: OXY_TOKEN, then `oxyc login`'s cache — or, in a GitHub Actions job\n" +
          "with `id-token: write` and neither, the job's OIDC identity: a\n" +
          "service account's trust policy first, then the app's registered publisher.\n" +
          "\nBefore the build, each Oxy Function's source is linted for what the host refuses\n" +
          "at the first call: a `ctx.*` call whose capability the manifest lacks, a global\n" +
          "the isolate does not have (Buffer, TextEncoder, process, …), a write outside\n" +
          "`destinations`; before the upload, with the target's database list, an `upsert`\n" +
          "or `ctx.tx` on an engine that refuses it and a customer-warehouse write with no\n" +
          "`customerWarehouseWrites` reason. `--allow-function-lint` is the way past a\n" +
          "false positive — please open an issue naming the rule.\n" +
          "\n--app-env dev-<handle> moves only that sandbox's build pointer — a staff\n" +
          "credential only, never a publish token, and never combined with --promote.\n"
      )
  ).action(async (opts: Record<string, unknown>) => {
    await runPublish(createContext(globals(opts)), {
      app: opts.app as string | undefined,
      buildId: opts.buildId as string | undefined,
      dir: opts.dir as string | undefined,
      promote: opts.promote as boolean | undefined,
      name: opts.name as string | undefined,
      repo: opts.repo as string | undefined,
      commit: opts.commit as string | undefined,
      branch: opts.branch as string | undefined,
      buildOnly: opts.buildOnly as boolean | undefined,
      prebuilt: opts.prebuilt as boolean | undefined,
      json: opts.json as boolean | undefined,
      semanticBranch: opts.semanticBranch as string | undefined,
      allowFunctionLint: opts.allowFunctionLint as boolean | undefined,
      appEnv: opts.appEnv as string | undefined
    });
  });

  withGlobals(
    program
      .command("init-ci")
      .description(
        "write a GitHub Actions workflow that publishes this app with no stored secret, and register its trust policy"
      )
      .option("--app <org/app>", "the app to publish (default: this directory's oxy-app.json)")
      .option("--environment <name>", "GitHub environment the publish job runs in", "oxy-publish")
      .option("--force", "overwrite an existing workflow")
      .option(
        "--promote",
        "publish straight to the live channel, and verify it with the app's checks"
      )
      .option("--no-register", "write the workflow only; create nothing on the deployment")
      .option(
        "--setup-action",
        "get the credential from the oxy-hq/setup-oxyc action (not published yet); by default oxyc exchanges the OIDC token itself"
      )
      .addHelpText(
        "after",
        "\nThe publish job acts as a service account of the app's org — --service-account\n" +
          "<org>/<name>, else `<org>/deployer`, which is created if it does not exist. Here\n" +
          "(and only here) the account is given by NAME: its ID is looked up as you and\n" +
          "written into the workflow, with the name beside it as a comment. As an\n" +
          "org admin with a browser-session login this also creates the trust policy (this\n" +
          "repository, the workflow file, the environment), granted publishing this one app.\n" +
          "When it cannot, it prints the exact steps instead; the workflow is written either way.\n"
      )
  ).action(async (opts: Record<string, unknown>) => {
    await runInitCi(createContext(globals(opts)), {
      app: opts.app as string | undefined,
      environment: opts.environment as string | undefined,
      force: opts.force as boolean | undefined,
      promote: opts.promote as boolean | undefined,
      register: opts.register as boolean | undefined,
      setupAction: opts.setupAction as boolean | undefined
    });
  });

  program
    .command("validate")
    .description("check a workspace's YAML against the schemas, and oxy-app.json's data placement")
    .option("-f, --file <path>", "validate one file instead of the whole workspace")
    .option("--json", "emit findings as JSON")
    .addHelpText(
      "after",
      "\nStructural checks only. `oxy validate` additionally resolves `databases:` and\n" +
        "`llm.ref` against config.yml, which needs the workspace loaded — where the two\n" +
        "disagree, that one is right. The schemas here are generated from the same Rust\n" +
        "types, so they cannot drift from it.\n" +
        "\nEach oxy-app.json is checked for where its data goes: customerWarehouseWrites,\n" +
        "airhouse, airhouseMigrations (DuckLake-safe, schema-qualified SQL) and secrets\n" +
        "used as state. Its functions' sources are linted for a `ctx.*` call whose\n" +
        "capability the manifest lacks, a global the isolate does not have (Buffer,\n" +
        "TextEncoder, process, …) and a write outside `destinations`; the engine half\n" +
        "(upsert / ctx.tx dialect, customer-warehouse writes) needs the server and runs\n" +
        "in `oxyc publish`. Warnings go to stderr; the server is the authority.\n"
    )
    .action((opts: Record<string, unknown>) => {
      runValidate({
        file: opts.file as string | undefined,
        json: opts.json as boolean | undefined
      });
    });

  withGlobals(
    program
      .command("mcp")
      .description("serve the Oxy API as MCP tools over stdio, for an agent runtime")
      .option(
        "--login",
        "serve on your own `oxyc login` when the token variable is unset (without it, unset is exit 4)"
      )
      .addHelpText(
        "after",
        "\nFour tools — oxy_routes, oxy_schema, oxy_request, oxy_whoami — not one per\n" +
          "endpoint: an agent runtime ships every tool's schema on every turn, and ~670 of\n" +
          "them would cost tens of KB per request. Discovery stays a question the agent\n" +
          "asks, so this reaches endpoints added after the package was published.\n\n" +
          "The credential is OXY_TOKEN (or the --token-env variable) and nothing else: an\n" +
          "agent must not inherit the login of whoever owns the machine. --login opts in.\n" +
          "A sandbox agent token (oxy_sbx_…) is served the sandbox loop only.\n\n" +
          "An agent, on its own token:  claude mcp add oxyc -e OXY_TOKEN=oxy_sbx_… -- npx -y @oxy-hq/cli mcp --env dev\n" +
          "You, on your login:          claude mcp add oxyc -- npx -y @oxy-hq/cli mcp --login --env production\n"
      )
  ).action(async (opts: Record<string, unknown>) => {
    const flags = globals(opts);
    if (opts.login && flags.tokenEnv !== undefined) {
      throw usageError(
        "--login and --token-env cannot be combined",
        "--token-env names the only place the bearer may come from; --login says your cached login may stand in"
      );
    }
    // IMPORTED HERE, not at the top. `@modelcontextprotocol/sdk` pulls in
    // express, ajv and zod, and a static import loads all of it on every
    // `oxyc` invocation — including `oxyc --help`. Worse, `zod` is a required
    // non-optional peer that only resolves through workspace hoisting, so on a
    // non-hoisting install a static import breaks EVERY command rather than
    // just this one.
    //
    // THIS FIXES LOAD TIME, NOT INSTALL WEIGHT. The SDK is still a plain
    // dependency, so `npx @oxy-hq/cli routes` downloads it and its tree for
    // someone who never runs `oxyc mcp`. An optional peer would move that cost
    // but make `oxyc mcp` fail at the import on a normal install, which is a
    // worse trade for the command an agent runtime is configured to start.
    const { runMcp } = await import("./commands/mcp.js");
    await runMcp(createContext({ ...flags, requireTokenEnv: !opts.login }));
  });

  program
    .command("guide")
    .description("a compact page to paste into AGENTS.md / CLAUDE.md so an agent knows this tool")
    .action(runGuide);

  program
    .command("exit-codes")
    .description("what each exit code means")
    .action(() => {
      process.stdout.write(EXIT_CODE_HELP);
    });

  return program;
}

/** commander's repeatable-option accumulator. */
function collect(value: string, previous: string[]): string[] {
  return [...previous, value];
}

const API_HELP = `
PLACEHOLDERS
  {org} {workspace} {project} {customer} {me} are substituted from context —
  the customer repo you are standing in, a pasted --env URL, or the flags above.

    oxyc api {org}/workspaces
    oxyc api {workspace}/threads --jq '.threads[].title'

BODIES
  -f key=value   string        -F key=value   typed (true / 3 / ["a"] / @file / @-)
  -F 'ids[]=a' -F 'ids[]=b'    repeats accumulate into an array
  --input @body.json           raw body from a file, or - for stdin
  On GET/HEAD/DELETE, fields become query parameters instead of a body.

SURFACES
  /api/**            bearer: $OXY_TOKEN, else \`oxyc login\`, else GitHub OIDC in Actions
  /external/api/**   X-API-Key: $OXY_API_KEY (a legacy API key or an API token),
                     else either of those in $OXY_TOKEN

FINDING AN ENDPOINT
  oxyc routes threads          what exists, and what each one does
  oxyc schema {workspace}/threads -X POST     the body it expects
`;

const EXIT_CODE_HELP = `0  success
1  failure with nothing more specific to say
2  usage error — a bad flag, a missing argument, a malformed value
4  not authenticated, or the token was rejected (401/403)
5  not found (404), or an unknown customer
6  the request was malformed (4xx other than 401/403/404)
7  unavailable — 5xx, a timeout, or the network failed. Retryable.
8  refused — the operation would have destroyed or overwritten something
9  a check ran and failed or timed out (\`oxyc checks run\`)
`;

/**
 * Render a failure and pick the exit code.
 *
 * The one rule that matters: errors go to STDERR and the exit code is never 0.
 * A tool that prints "ERROR" on stdout and exits 0 is invisible to an agent,
 * which branches on the code alone and will happily carry on with garbage.
 */
function reportAndExit(cause: unknown): never {
  if (cause instanceof CliError) {
    log.error(cause.message);
    if (cause.detail) {
      for (const line of cause.detail.split("\n")) process.stderr.write(`  ${err.dim(line)}\n`);
    }
    if (cause.hint) {
      for (const line of cause.hint.split("\n")) log.hint(line);
    }
    // Through `log.remedy`, so an error's remedy reads the way a warning's
    // does. Handed to `hint` it wore `→` and joined the run of elaborations.
    // Whole, not split: `log.remedy`'s arguments are REMEDIES, and it splits
    // each one itself so every line keeps the marker. Splitting here made the
    // parameter mean two things depending on the caller.
    if (cause.remedy) log.remedy(cause.remedy);
    process.exit(cause.code);
  }
  log.error((cause as Error)?.message ?? String(cause));
  if (process.env.OXYC_DEBUG && cause instanceof Error && cause.stack) {
    process.stderr.write(`${err.dim(cause.stack)}\n`);
  }
  process.exit(ExitCode.FAILURE);
}

/**
 * Make commander exit with OUR codes, on every command in the tree.
 *
 * `exitOverride` applies only to the command it is called on — it does NOT
 * propagate to subcommands. Setting it on the root alone left
 * `oxyc api user --nonsense` exiting 1 (commander's default) instead of 2,
 * which is exactly the distinction an agent branches on: 1 means "the request
 * failed, maybe retry", 2 means "you called it wrong, stop". So it is applied
 * recursively, and `main.test.ts` pins it.
 */
function applyExitOverride(command: Command): void {
  command.exitOverride((error) => {
    // `--help` and `--version` are successes that commander signals by
    // throwing. Exiting non-zero on them would make `oxyc api --help` look
    // like a failed command in any script that checks.
    if (
      error.code === "commander.helpDisplayed" ||
      error.code === "commander.help" ||
      error.code === "commander.version"
    ) {
      process.exit(ExitCode.OK);
    }
    process.exit(ExitCode.USAGE);
  });
  for (const child of command.commands) applyExitOverride(child);
}

/**
 * `oxyc pokehouse` means `oxyc launch pokehouse`.
 *
 * The bare form is the flagship interaction of the tooling this absorbed —
 * what people type all day — so it has to survive the port. commander has no
 * first-class "unknown verb is an argument" hook, so the rewrite happens here,
 * before parsing, and only for a token that is neither a known command nor a
 * flag.
 *
 * THE COST, and the reason `didYouMeanCommand` exists: this makes every
 * mistyped command look like a customer. `oxyc rotues` would rewrite to
 * `launch rotues` and come back "unknown customer rotues" — an error about
 * the wrong thing entirely, which is the failure mode that makes a default
 * command a bad idea in the first place. So a token CLOSE to a real command is
 * treated as the typo it almost certainly is, and a token close to nothing is
 * treated as a customer name.
 */
function expandBareCustomer(argv: string[], program: Command): string[] {
  const [node, script, first, ...rest] = argv;
  if (!first || first.startsWith("-")) return argv;

  const known = program.commands.flatMap((c) => [c.name(), ...c.aliases()]);
  if (known.includes(first)) return argv;

  const meant = didYouMeanCommand(first, known);
  if (meant) {
    throw new CliError(`unknown command "${first}"`, {
      code: ExitCode.USAGE,
      hint: `did you mean \`oxyc ${meant}\`?   (for a customer of that name: \`oxyc launch ${first}\`)`
    });
  }

  return [node as string, script as string, "launch", first, ...rest];
}

async function main(): Promise<void> {
  const program = buildProgram();
  applyExitOverride(program);
  // `expandBareCustomer` can throw (a near-miss command name), so it is inside
  // `main` rather than at the call site — `main().catch(reportAndExit)` is what
  // turns that into the usage exit code and the suggestion.
  try {
    await program.parseAsync(expandBareCustomer(process.argv, program));
  } finally {
    // A token this process minted from a GitHub OIDC token is revoked on the
    // way out, success or failure. In a `finally` so it runs BEFORE
    // `reportAndExit` on the failure path; it never throws and never changes
    // the exit code. A no-op for every process that minted nothing.
    await runExitRevokes();
  }
}

// A rejected promise anywhere in the tree has to end the process non-zero —
// node's default for an unhandled rejection is a warning and exit 0, which is
// the exact "printed an error, reported success" shape this tool refuses.
process.on("unhandledRejection", (cause) => {
  void runExitRevokes().finally(() => reportAndExit(cause));
});

// Unconditional. There was an `isEntryPoint()` guard here so that importing
// this module (which the test did, to reach `didYouMeanCommand`) would not
// start the CLI — but an npm bin is a SYMLINK, `resolve()` does not follow
// links, and the comparison would have been false for every `npm i -g`
// install: `oxyc` would print nothing and exit 0. The function moved to
// `util/suggest.ts` instead, so there is nothing left to guard against.
main().catch(reportAndExit);
