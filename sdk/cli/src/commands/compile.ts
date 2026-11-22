/**
 * `oxyc compile <branch> [--wait] [--json]` — compile one branch of a
 * workspace, the way the deployment that serves it compiles, and say whether it
 * compiled. The customer repo's CI gate (`template/.github/workflows/validate.yaml`).
 *
 *   POST /api/{workspace_id}/compile/staging?branch=<b>        queue (or reuse) a staging compile
 *   GET  /api/{workspace_id}/compile/staging/status?git_sha=<s> where it stands
 *
 * It replaced the template's `oxy compile --skip-migrations --json`, which
 * compiled a checkout into a throwaway Postgres the job had to boot, migrate and
 * seed with a `workspaces` row by hand. The server compiles the commit GitHub
 * has for the branch, as the workspace's registered subdirectory — what will
 * actually be served — into a **staging** revision, which is never promoted. The
 * Rust `oxy compile` stays as dev tooling for a local database.
 *
 * Who may: a person who may use the Factory and edit the workspace, or — the CI
 * gate — the org's service account through a CI run token (GitHub OIDC trusted
 * access, `OXY_SERVICE_ACCOUNT`), whose trust policy grants it `viewer` on the
 * workspace (`Action::CompileCheck`, decided 2026-11-21).
 *
 * `--json` keeps the shape a CI script parsed out of `oxy compile --json`: a
 * `failures` array of `{path, kind, message}`, always an array, plus `status`
 * and the file counts. Exit codes: 0 compiled; 9 it compiled and recorded
 * failures (the check ran and the answer was no); 1 the compile failed with no
 * per-file failures (the commit could not be fetched, the run died); 7 `--wait`
 * ran out, or the server is unavailable; 4/5/6 as for every request.
 */

import { normalizePath, substitutePlaceholders } from "../api/paths.js";
import type { Context } from "../context/resolve.js";
import {
  type CompileFailure,
  inFlight,
  type StagingCompile,
  type StagingCompileWire,
  WAIT_TIMEOUT_MS,
  waitForStagingCompile
} from "../publish/semantic-branch.js";
import * as log from "../ui/log.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";
import { call, type RefusalReason } from "./oltp-client.js";

/** The report `--json` prints. `failures` is always an array, as `oxy compile --json`'s was. */
export interface CompileReport {
  status: StagingCompile["status"];
  workspace_id: string | null;
  branch: string;
  git_sha: string;
  revision_id: string | null;
  file_count_seen: number | null;
  file_count_compiled: number | null;
  file_count_failed: number | null;
  failures: CompileFailure[];
  error: string | null;
}

export function compilePath(ctx: Context): string {
  return substitutePlaceholders(normalizePath("{workspace}/compile/staging"), ctx.placeholders());
}

/** Why a `/compile/staging` route refused, in the terms the caller acts on. */
export function compileRefusal(status: number): string | undefined {
  switch (status) {
    case 400:
      return "the server rejected the branch name";
    case 401:
      return "not authenticated for this target";
    case 403:
      return (
        "not allowed to compile this workspace. A person needs Factory access and editor " +
        "standing in it; a CI run needs OXY_SERVICE_ACCOUNT set and a trust policy on that " +
        "account naming this repository, .github/workflows/validate.yaml and the job's " +
        "environment, with a workspace grant (viewer is enough)"
      );
    case 404:
      return (
        "no such workspace here, one your credential cannot see — or a branch GitHub does not have " +
        "(push it first)"
      );
    case 409:
      return "the workspace is in a state someone has to fix first — the message below says which";
    case 503:
      return "the deployment cannot compile right now (GitHub or the database blipped) — retryable";
    default:
      return undefined;
  }
}

/** What `--json` prints for a state: `oxy compile --json`'s keys, `failures` always an array. */
export function toReport(state: StagingCompile, branch: string): CompileReport {
  return {
    status: state.status,
    workspace_id: state.workspace_id ?? null,
    branch,
    git_sha: state.git_sha,
    revision_id: state.revision_id ?? null,
    file_count_seen: state.file_count_seen ?? null,
    file_count_compiled: state.file_count_compiled ?? null,
    file_count_failed: state.file_count_failed ?? null,
    failures: state.failures ?? [],
    error: state.error ?? null
  };
}

/** The human report: one status line, then one line per failure. */
export function renderReport(r: CompileReport): string {
  const sha = r.git_sha.slice(0, 12);
  const lines: string[] = [];
  switch (r.status) {
    case "ready":
      lines.push(`${r.branch} compiles (${sha}, revision ${r.revision_id ?? "?"})`);
      break;
    case "failed":
      lines.push(
        r.failures.length > 0
          ? `${r.branch} failed to compile (${sha}): ${r.failures.length} file(s) recorded failures`
          : `${r.branch} failed to compile (${sha})`
      );
      break;
    default:
      lines.push(`${r.branch} is ${r.status} (${sha}) — pass --wait to wait for the result`);
  }
  if (r.file_count_seen !== null) {
    lines.push(
      `  files: ${r.file_count_seen} seen, ${r.file_count_compiled ?? 0} compiled, ${r.file_count_failed ?? 0} failed`
    );
  }
  for (const f of r.failures) lines.push(`  ${f.path} [${f.kind}] ${f.message}`);
  if (r.status === "failed" && r.failures.length === 0 && r.error) lines.push(`  ${r.error}`);
  return lines.join("\n");
}

/** The error a terminal report ends the process with; `undefined` when it compiled. */
export function reportFailure(r: CompileReport): CliError | undefined {
  if (r.status !== "failed") return undefined;
  if (r.failures.length > 0) {
    return new CliError(`${r.branch} recorded ${r.failures.length} compile failure(s)`, {
      code: ExitCode.CHECK_FAILED,
      hint: "fix each file above and push; the branch is compiled again on the next run"
    });
  }
  return new CliError(`${r.branch} failed to compile`, {
    code: ExitCode.FAILURE,
    detail: r.error ?? undefined,
    hint: "no file recorded a failure, so the compile itself did not run to the end — re-run it"
  });
}

export async function runCompile(
  ctx: Context,
  branch: string | undefined,
  flags: { wait?: boolean; timeoutSeconds?: number; json?: boolean; pollMs?: number }
): Promise<void> {
  // EVERY USAGE ERROR BEFORE A REQUEST.
  const name = branch?.trim();
  if (!name) throw usageError("name the branch to compile: oxyc compile <branch> --workspace <id>");
  if (
    flags.timeoutSeconds !== undefined &&
    (!Number.isFinite(flags.timeoutSeconds) || flags.timeoutSeconds <= 0)
  ) {
    throw usageError("--timeout must be a positive number of seconds", "e.g. --timeout 900");
  }
  const base = compilePath(ctx);
  const refusal = compileRefusal as RefusalReason;
  const wire: StagingCompileWire = {
    post: () =>
      call<StagingCompile>(ctx, "POST", `${base}?branch=${encodeURIComponent(name)}`, {
        refusal,
        routesArea: "compile"
      }),
    status: (sha) =>
      call<StagingCompile>(ctx, "GET", `${base}/status?git_sha=${encodeURIComponent(sha)}`, {
        refusal,
        routesArea: "compile"
      })
  };
  let state = await wire.post();
  if (flags.wait) {
    let said = "";
    state = await waitForStagingCompile(state, wire, {
      pollMs: flags.pollMs,
      timeoutMs: flags.timeoutSeconds === undefined ? WAIT_TIMEOUT_MS : flags.timeoutSeconds * 1000,
      onWait: (s) => {
        if (!flags.json && s.status !== said) {
          said = s.status;
          log.info(`${name} (${s.git_sha.slice(0, 12)}): ${s.status}…`);
        }
      },
      onTimeout: (s) => {
        throw new CliError(`${name} did not finish compiling in time (last: ${s.status})`, {
          code: ExitCode.UNAVAILABLE,
          hint: "re-run it — a finished compile of the same commit is reused, so nothing is lost"
        });
      }
    });
  }
  const report = toReport(state, name);
  process.stdout.write(
    flags.json ? `${JSON.stringify(report, null, 2)}\n` : `${renderReport(report)}\n`
  );
  if (inFlight(state)) return;
  const failed = reportFailure(report);
  if (failed) throw failed;
}
