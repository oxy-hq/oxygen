/**
 * `oxyc looker test <name> --workspace <id>` — check one Looker integration's
 * credentials on the deployment.
 *
 *   POST /api/{workspace_id}/integrations/looker/{name}/test
 *
 * Behind `WorkspaceAdmin`, served by the workspace-config surface.
 * It replaced the Rust `oxy looker test`, which read `config.yml` from a local
 * checkout and the credentials from the shell; the server reads the served
 * revision's compiled config and the workspace's secrets, so this checks what
 * an automation's `looker_query` step will actually use.
 *
 * The route answers a check that RAN with `200` and its verdict in `ok` (as
 * `…/oltp/test-connection` does). A verdict of no exits `9`, the "a check ran
 * and failed" code — the Rust command printed the failure and exited `0`.
 */

import type { Command } from "commander";
import { normalizePath, substitutePlaceholders } from "../api/paths.js";
import { type Context, createContext } from "../context/resolve.js";
import { out } from "../ui/tty.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";
import type { Wiring } from "./flags-cli.js";
import { call } from "./oltp-client.js";

/** `oxy_api_workspace_config::integration::looker_test::LookerConnectionTest`. */
export interface LookerConnectionTest {
  ok: boolean;
  /** `resolve` | `client` | `auth` | `connect` | `timeout`; null when ok. */
  stage: string | null;
  error: string | null;
  integration: string;
  base_url: string;
  models: Array<{ name: string; explores: number }>;
  elapsed_ms: number;
}

/** How many models the human output lists before "… and N more", as the Rust did. */
const SHOWN_MODELS = 5;

/** Why the route refused, in the terms someone acts on. */
export function lookerRefusal(status: number): string | undefined {
  switch (status) {
    case 401:
      return "not authenticated for this target";
    case 403:
      return (
        "checking an integration needs a workspace admin — staff act inside a tenant through " +
        "`oxyc assume`"
      );
    case 404:
      return (
        "no Looker integration by that name in the workspace's served config (or no such " +
        "workspace here) — `oxyc api {workspace}/integrations/looker` lists the names"
      );
    case 503:
      return "the deployment cannot serve this workspace's config yet (not compiled) — retryable";
    default:
      return undefined;
  }
}

export function lookerTestPath(ctx: Context, name: string): string {
  return substitutePlaceholders(
    normalizePath(`{workspace}/integrations/looker/${encodeURIComponent(name)}/test`),
    ctx.placeholders()
  );
}

export async function runLookerTest(
  ctx: Context,
  integration: string,
  opts: { json: boolean }
): Promise<void> {
  const name = integration.trim();
  if (!name) throw usageError("name the integration: oxyc looker test <name> --workspace <id>");
  const path = lookerTestPath(ctx, name);
  const result = await call<LookerConnectionTest>(ctx, "POST", path, {
    refusal: lookerRefusal,
    routesArea: "integrations"
  });

  if (opts.json) process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
  else printResult(result);

  if (!result.ok) {
    throw new CliError(`Looker integration \`${name}\` failed its check (${result.stage})`, {
      code: ExitCode.CHECK_FAILED,
      detail: opts.json ? undefined : (result.error ?? undefined),
      hint: hintFor(result)
    });
  }
}

function printResult(r: LookerConnectionTest): void {
  const lines = [`Looker integration ${r.integration} (${r.base_url})`];
  if (r.ok) {
    lines.push(out.green(`  Connection successful (${r.elapsed_ms} ms)`));
    lines.push(`  Available models: ${r.models.length}`);
    for (const model of r.models.slice(0, SHOWN_MODELS)) {
      lines.push(`    - ${model.name} (${model.explores} explores)`);
    }
    if (r.models.length > SHOWN_MODELS) {
      lines.push(`    ... and ${r.models.length - SHOWN_MODELS} more`);
    }
  }
  process.stdout.write(`${lines.join("\n")}\n`);
}

function hintFor(r: LookerConnectionTest): string | undefined {
  switch (r.stage) {
    case "resolve":
      return "add the secret in Workspace Settings → Secrets, under the name config.yml gives it";
    case "auth":
      return "Looker refused the client ID and secret — check them, and that the API key is not rotated";
    case "connect":
    case "timeout":
      return "check base_url in config.yml, including the API port (typically :19999)";
    default:
      return undefined;
  }
}

const HELP = `
Checks the integration the deployment SERVES: config.yml from the workspace's compiled revision,
the client ID and secret from the workspace's secrets (Settings → Secrets) — not a local checkout
or your shell's environment. It signs in to Looker and lists the LookML models.

  oxyc looker test my_looker --workspace <id>          the verdict, and the first models
  oxyc looker test my_looker --workspace <id> --json   {ok, stage, error, integration, base_url, models, elapsed_ms}

Exit codes: 9 the check ran and failed (stage resolve: a secret is missing; auth: Looker refused
the credentials; connect / timeout: base_url did not answer); 5 no Looker integration by that
name (or no such workspace); 4 not signed in, or not a workspace admin; 7 retryable.`;

export function registerLooker(program: Command, w: Wiring): void {
  const looker = program
    .command("looker")
    .description("Looker integrations on a deployment: check one's credentials")
    .addHelpText("after", HELP);

  w.withGlobals(
    looker
      .command("test <name>")
      .description("check a Looker integration's credentials: sign in and list its models")
      .option("--json", "emit the server's response")
  ).action(async (name: string, opts: Record<string, unknown>) => {
    await runLookerTest(createContext(w.globals(opts)), name, { json: Boolean(opts.json) });
  });
}
