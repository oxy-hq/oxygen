/**
 * `oxyc oltp dsn | connect | expose | deprovision` — reaching into an org's OLTP
 * database, and releasing it, from a laptop.
 *
 *   POST   /api/admin/orgs/{org_id}/oltp/credentials         {"role": "analyst" | "app:<w>" | "pipeline:<s>"}
 *   POST   /api/admin/orgs/{org_id}/oltp/visibility          {"writer": "...", "visible": bool}
 *   POST   /api/admin/orgs/{org_id}/oltp/deprovision-writer  {"writer": "..."}
 *   DELETE /api/admin/orgs/{org_id}/oltp                     the whole tenant database
 *
 * These replaced the Rust `oxy oltp dsn | connect | expose | deprovision`, which
 * read `OXY_DATABASE_URL` and the sealing key and so only ever reached the
 * control plane of the machine they ran on. Every route here is audited
 * server-side (`oltp.credential.disclosed`, `oltp.visibility.changed`,
 * `oltp.writer.deprovisioned`, `oltp.deprovisioned`) and sits behind
 * `PlatformOltp` and the admin scope fence, so an out-of-scope org answers 404.
 *
 * There is no `--role owner`: the credentials route hands out the analyst and
 * writer DSNs only, never the migration owner's.
 */

import { spawn } from "node:child_process";

import type { Context } from "../context/resolve.js";
import { out } from "../ui/tty.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";
import { orgOrHint, resolveOrgId } from "./assume.js";
import { parseWriter } from "./oltp.js";
import { call, orgPath, PROVISION_TIMEOUT_MS } from "./oltp-client.js";

/** `oxy_api_data_plane::oltp::admin::CredentialsResponse`. */
export interface Credentials {
  role: string;
  dsn: string;
  writable: boolean;
}

/** The fields of `ConnectionInfoResponse` these verbs read. */
interface StoreInfo {
  is_provisioned: boolean;
  host: string;
  database: string;
  provider: string;
  schemas: Array<{ schema: string; analytics_visible: boolean }>;
}

/** `--role` → what the credentials route takes. `analyst` unless asked otherwise. */
export function parseRole(typed: string | undefined): string {
  const role = (typed ?? "analyst").trim();
  if (role === "analyst") return role;
  if (role === "owner") {
    throw usageError(
      "--role owner is not handed out",
      "the credentials route discloses the analyst and writer DSNs only; the migration owner's stays on the server"
    );
  }
  return parseWriter(role).spec;
}

async function fetchCredentials(
  ctx: Context,
  org: string | undefined,
  role: string
): Promise<Credentials> {
  const orgId = await resolveOrgId(ctx, org);
  return call<Credentials>(ctx, "POST", `${orgPath(orgId)}/credentials`, { body: { role } });
}

export async function runOltpDsn(
  ctx: Context,
  org: string | undefined,
  flags: { role?: string; json?: boolean }
): Promise<void> {
  const role = parseRole(flags.role);
  const creds = await fetchCredentials(ctx, org, role);
  if (flags.json) {
    process.stdout.write(`${JSON.stringify(creds, null, 2)}\n`);
    return;
  }
  // Bare stdout, no decoration: this is meant to be captured into a variable.
  process.stdout.write(`${creds.dsn}\n`);
}

/**
 * A DSN with its password removed, and the password.
 *
 * Only the `userinfo` half of a URI is touched: host, port, database and every
 * query parameter (`sslmode` included) pass through unchanged, because dropping
 * one of those would change how the connection is made, not how it is spelled.
 * Returns the DSN untouched and no password when there is none to move — the
 * local-cluster case.
 */
export function splitPassword(dsn: string): [string, string | undefined] {
  const sep = dsn.indexOf("://");
  if (sep < 0) return [dsn, undefined];
  const scheme = dsn.slice(0, sep);
  const rest = dsn.slice(sep + 3);
  // The LAST `@` before the path: a password may legally contain one, and
  // splitting on the first would hand psql a hostname made of the rest of it.
  const slash = rest.indexOf("/");
  const hostEnd = slash < 0 ? rest.length : slash;
  const at = rest.slice(0, hostEnd).lastIndexOf("@");
  if (at < 0) return [dsn, undefined];
  const userinfo = rest.slice(0, at);
  const tail = rest.slice(at);
  const colon = userinfo.indexOf(":");
  if (colon < 0) return [dsn, undefined];
  return [
    `${scheme}://${userinfo.slice(0, colon)}${tail}`,
    decodeUserinfo(userinfo.slice(colon + 1))
  ];
}

/**
 * Percent-decoding for a userinfo field. The DSN carries the password encoded;
 * `PGPASSWORD` takes the raw bytes, so the encoded form would fail as "password
 * authentication failed" — a mangled credential that reads as a wrong one. A
 * stray `%` is not an escape and survives.
 */
export function decodeUserinfo(s: string): string {
  const bytes: number[] = [];
  let i = 0;
  while (i < s.length) {
    if (s[i] === "%" && i + 2 < s.length) {
      const hex = s.slice(i + 1, i + 3);
      if (/^[0-9a-fA-F]{2}$/.test(hex)) {
        bytes.push(Number.parseInt(hex, 16));
        i += 3;
        continue;
      }
    }
    bytes.push(...Buffer.from(s[i] as string, "utf8"));
    i += 1;
  }
  return Buffer.from(bytes).toString("utf8");
}

/**
 * `psql` into the org's database as `--role` — the analyst by default, so the
 * reflex action is the read-only one. On a managed provider the DSN carries
 * `sslmode=require`, so this also proves TLS end to end.
 */
export async function runOltpConnect(
  ctx: Context,
  org: string | undefined,
  flags: { role?: string }
): Promise<void> {
  const role = parseRole(flags.role);
  const creds = await fetchCredentials(ctx, org, role);
  // The password goes in the ENVIRONMENT, not argv: an argument is readable by
  // every user on the box through `ps auxww` for the life of the session.
  const [sanitised, password] = splitPassword(creds.dsn);
  process.stderr.write(
    `connecting as ${creds.role}${creds.writable ? " (WRITABLE)" : ""} (^D to exit)\n`
  );
  const env = { ...process.env };
  if (password !== undefined) env.PGPASSWORD = password;
  // psql owns the terminal, and ^C / ^\ reach the whole foreground group. psql
  // catches them to cancel a query; with node's default disposition oxyc would
  // die instead, handing the terminal back to the shell and orphaning psql
  // (and its open transaction). So oxyc ignores both while psql runs.
  const ignore = () => {};
  process.on("SIGINT", ignore);
  process.on("SIGQUIT", ignore);
  const code = await new Promise<number>((done, fail) => {
    const child = spawn("psql", [sanitised], { stdio: "inherit", env });
    child.on("error", (cause) => {
      const missing = (cause as NodeJS.ErrnoException).code === "ENOENT";
      fail(
        new CliError(missing ? "psql is not on PATH" : `could not start psql: ${cause.message}`, {
          code: ExitCode.UNAVAILABLE,
          remedy: "install libpq, or use: oxyc oltp dsn --org <org>"
        })
      );
    });
    child.on("exit", (status) => done(status ?? 1));
  }).finally(() => {
    process.off("SIGINT", ignore);
    process.off("SIGQUIT", ignore);
  });
  if (code !== 0) throw new CliError(`psql exited with ${code}`, { code: ExitCode.FAILURE });
}

/**
 * Let the read-only analyst read a writer's schema, or withdraw it. `app_*` is
 * hidden by default because live app state may be regulated, so widening it is
 * its own verb with its own audit line.
 */
export async function runOltpExpose(
  ctx: Context,
  org: string | undefined,
  flags: { writer?: string; revoke?: boolean; json?: boolean }
): Promise<void> {
  if (!flags.writer) throw usageError("name the writer: --writer app:<slug> | pipeline:<source>");
  const writer = parseWriter(flags.writer);
  const visible = !flags.revoke;
  const orgId = await resolveOrgId(ctx, org);
  const info = await call<StoreInfo>(ctx, "POST", `${orgPath(orgId)}/visibility`, {
    body: { writer: writer.spec, visible }
  });
  if (flags.json) {
    process.stdout.write(`${JSON.stringify(info, null, 2)}\n`);
    return;
  }
  process.stdout.write(
    `${out.green(`${writer.schema} is ${visible ? "readable by" : "hidden from"} the read-only analyst`)}\n`
  );
}

/**
 * Drop one writer (`--writer`), or destroy the org's whole database at the
 * provider. Both are irreversible, so neither runs without `--yes`, and there
 * is no prompt: the plan is printed, and the command exits 8.
 */
export async function runOltpDeprovision(
  ctx: Context,
  org: string | undefined,
  flags: { writer?: string; yes?: boolean; json?: boolean }
): Promise<void> {
  // A malformed `--writer` must be a usage error here: falling through to the
  // whole-tenant branch would turn a typo into the destruction of the database.
  if (flags.writer !== undefined && flags.writer.trim() === "") {
    throw usageError(
      "--writer is empty",
      "app:<slug> | pipeline:<source>, or omit it to destroy the whole database"
    );
  }
  const writer = flags.writer === undefined ? undefined : parseWriter(flags.writer);
  const orgId = await resolveOrgId(ctx, org);
  const named = orgOrHint(ctx, org);
  const current = await call<StoreInfo>(ctx, "GET", orgPath(orgId));
  if (!current.is_provisioned) {
    process.stdout.write(`no OLTP database for org ${named} — nothing to do\n`);
    return;
  }

  // Host as well as database: on a fleet with more than one cluster it is the
  // line that says you are pointed at the right one.
  const where = `${current.database} on ${current.host} (${current.provider})`;
  const plan = writer
    ? [`This DROPS schema ${writer.schema} and all its data in ${where}, on ${ctx.target()}`]
    : [
        `This DESTROYS ${where}, on ${ctx.target()}`,
        ...(current.provider === "local"
          ? []
          : ["  the provider project is deleted and cannot be recovered"])
      ];
  process.stderr.write(`${plan.join("\n")}\n`);
  if (!flags.yes) {
    throw new CliError("refused without --yes", {
      code: ExitCode.REFUSED,
      detail: "deprovisioning is irreversible",
      remedy: "re-run with --yes once the line above is what you want"
    });
  }

  const info = writer
    ? await call<StoreInfo>(ctx, "POST", `${orgPath(orgId)}/deprovision-writer`, {
        body: { writer: writer.spec },
        timeoutMs: PROVISION_TIMEOUT_MS
      })
    : await call<StoreInfo>(ctx, "DELETE", orgPath(orgId), { timeoutMs: PROVISION_TIMEOUT_MS });
  if (flags.json) {
    process.stdout.write(`${JSON.stringify(info, null, 2)}\n`);
    return;
  }
  process.stdout.write(
    `${out.green(
      writer
        ? `deprovisioned writer ${writer.spec} (schema ${writer.schema})`
        : `deprovisioned ${current.database} (${current.provider})`
    )}\n`
  );
}
