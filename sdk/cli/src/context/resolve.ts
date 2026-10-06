/**
 * The merge point of the two tools.
 *
 * `oxyc`'s customer half knows WHO — which customer, where their repo is, what
 * org they are. `oxyc`'s API half knows HOW to ask a deployment a question.
 * This is what joins them: one resolution that produces both halves, so
 * `oxyc api {org}/workspaces` works from inside a customer repo without
 * anyone typing an id.
 *
 * `gh api` does the same trick with `{owner}` and `{repo}` read off the git
 * remote, and it is the single feature that makes that command usable from
 * memory. Everything here exists to make ours as cheap to use.
 *
 * RESOLUTION IS LAZY AND PARTIAL, deliberately. `oxyc routes` needs a target
 * and no customer; `oxyc list` needs neither. Resolving everything eagerly
 * would make a command fail on a fact it never uses — which is how a tool
 * ends up demanding a GitHub login to print its own help.
 */

import type { PlaceholderValues } from "../api/paths.js";
import { loadCredential } from "../auth/credentials.js";
import { exchangeOidcOnce, githubOidcAvailable, OidcExchangeError } from "../auth/oidc.js";
import { dossierPath, isCloned, slugForDirectory } from "../customer/dossier.js";
import { type Customer, customersOrg, resolveCustomer } from "../github/customers.js";
import * as log from "../ui/log.js";
import { authError, CliError, ExitCode } from "../util/errors.js";
import { repoRoot } from "../util/git.js";
import { loadForTargetResolution, type ResolvedEnv, resolveEnv } from "./target.js";

/** Flags every command shares. Kept in one shape so `main.ts` wires them once. */
export interface GlobalFlags {
  env?: string;
  target?: string;
  tokenEnv?: string;
  apiKeyEnv?: string;
  org?: string;
  workspace?: string;
  project?: string;
  customer?: string;
  refresh?: boolean;
  /** `--service-account <id>`: which account a GitHub OIDC exchange acts as, by its ID. */
  serviceAccount?: string;
}

/**
 * A credential and where it came from.
 *
 * The three sources ARE the resolution order, for every command:
 *
 *   env    `OXY_TOKEN` (or `--token-env`). Any credential: an API token, a
 *          legacy API key, a publish token or a session.
 *   file   what `oxyc login` cached for this host.
 *   oidc   in a GitHub Actions job granted `id-token: write`, the job's OIDC
 *          token exchanged for a fifteen-minute `oxy_ci_` token.
 *
 * The first two are read off the machine and cost nothing. The third is a
 * network exchange, which is why everything that can reach it is async.
 */
export interface ResolvedCredential {
  token: string;
  source: "env" | "file" | "oidc";
  /** RFC 3339, when it is known. */
  expiresAt?: string;
  tokenId?: string;
  /** `<org>/<name>`, on an OIDC credential. */
  serviceAccount?: string;
}

/** Everything a command might need, resolved on demand. */
export interface Context {
  readonly cwd: string;
  readonly flags: GlobalFlags;

  /** The deployment to talk to. Throws when nothing resolves. */
  target(): string;
  /** The resolved env, including the org slug a pasted URL carried. */
  env(): ResolvedEnv;
  /**
   * The credential, from the first source that has one. Throws `authError`
   * when none does, or the exchange's own error when GitHub OIDC was available
   * and refused — that one says what to fix, which "not authenticated" cannot.
   */
  credential(): Promise<ResolvedCredential>;
  /** `credential().token`. */
  bearer(): Promise<string>;
  /**
   * The bearer if there is one, without throwing. A refused OIDC exchange is a
   * warning here rather than an error: the caller asked "if there is one".
   */
  maybeBearer(): Promise<string | undefined>;
  /**
   * The bearer already on this machine — `OXY_TOKEN`, then the login cache —
   * and nothing else. Synchronous, no network, never mints.
   *
   * For a caller with another credential in hand (an API key), or one that
   * must decide before it is ready to spend a single-use OIDC token.
   */
  storedBearer(): string | undefined;
  /** `--service-account`, then `OXY_SERVICE_ACCOUNT`. */
  serviceAccount(): string | undefined;
  /** `OXY_API_KEY`: a legacy API key or an API token for `/external/api`, if one is set. */
  apiKey(): string | undefined;
  /** The customer this invocation is about, if it is about one. */
  customer(): Customer | undefined;
  /** The customer's repo checkout on this machine, if it is here. */
  repoDir(): string | undefined;
  /** Values for `{org}` / `{workspace}` / … in a path. */
  placeholders(): PlaceholderValues;
  /**
   * The same invocation pointed at a different deployment.
   *
   * For `oxyc login --login-env dev,staging`, which is several independent acts
   * rather than one act with several targets — each needs its own resolved
   * env, and a `Context` memoizes the one it was built with. Rebuilding is
   * cheaper than making every consumer take a target parameter it has no use
   * for; `--target` is dropped, because it overrides `--env` and carrying it
   * would point all of them at one host.
   */
  withEnv(env: string): Context;
}

/**
 * Build the context for one invocation.
 *
 * Every accessor memoises, so a command that asks for the bearer three times
 * reads the credentials file once — and, more importantly, a command that
 * never asks never touches the network or the disk at all.
 */
export function createContext(flags: GlobalFlags, cwd = process.cwd()): Context {
  const memo = new Map<string, unknown>();

  const once = <T>(key: string, compute: () => T): T => {
    if (!memo.has(key)) memo.set(key, compute());
    return memo.get(key) as T;
  };

  const env = (): ResolvedEnv =>
    once("env", () => {
      // Read here rather than up front: a broken oxy-app.json is an error, and
      // it must not fail a command that never resolves a target (`oxyc list`).
      const manifest = loadForTargetResolution(cwd, flags.target);
      const resolved = resolveEnv(flags.env ?? "production", flags.target, manifest);
      if (!resolved) {
        throw new CliError(`could not resolve a target for --env ${flags.env}`, {
          code: ExitCode.USAGE,
          hint: "pass --target <url>, use a URL as the env (--env https://…), or add it to oxy-app.json environments"
        });
      }
      return resolved;
    });

  const customer = (): Customer | undefined =>
    once("customer", () => {
      // An explicit --customer (or a positional the launcher already resolved)
      // wins. Otherwise infer from the checkout we are standing in, which is
      // what makes the placeholders free inside a customer session.
      if (flags.customer) return resolveCustomer(flags.customer, { refresh: flags.refresh });
      const slug = slugForDirectory(cwd);
      if (!slug) return undefined;
      const [, name] = slug.split("/");
      if (!name) return undefined;
      try {
        return resolveCustomer(name, { refresh: flags.refresh });
      } catch {
        // Standing in one of OUR repos, or any repo that is not a customer.
        // Not an error: most invocations are not about a customer at all.
        return undefined;
      }
    });

  const repoDir = (): string | undefined =>
    once("repoDir", () => {
      const found = customer();
      if (found) {
        const slug = `${customersOrg()}/${found.name}`;
        if (isCloned(slug)) return dossierPath(slug);
      }
      // `--here`: working in one of our own repos on a customer's behalf.
      return repoRoot(cwd);
    });

  const stored = (): ResolvedCredential | undefined =>
    once("stored", () => {
      const fromEnv = process.env[flags.tokenEnv ?? "OXY_TOKEN"]?.trim();
      if (fromEnv) return { token: fromEnv, source: "env" as const };
      const cached = loadCredential(env().target);
      const token = cached?.token?.trim();
      if (!token) return undefined;
      return {
        token,
        source: "file" as const,
        expiresAt: cached?.expires_at,
        tokenId: cached?.token_id
      };
    });

  const serviceAccount = (): string | undefined =>
    flags.serviceAccount?.trim() || process.env.OXY_SERVICE_ACCOUNT?.trim() || undefined;

  /**
   * The third source. Memoised per process inside `exchangeOidcOnce`.
   *
   * Attempted only for a run that names its service account. One that names
   * none gets `no_service_account` without a request being made — the error
   * every command but `publish` and `checks run` then shows.
   */
  const minted = async (): Promise<ResolvedCredential> => {
    const exchanged = await exchangeOidcOnce(env().target, serviceAccount());
    return {
      token: exchanged.token,
      source: "oidc",
      expiresAt: exchanged.expiresAt,
      tokenId: exchanged.tokenId,
      serviceAccount: exchanged.serviceAccount
    };
  };

  const credential = async (): Promise<ResolvedCredential> => {
    const have = stored();
    if (have) return have;
    if (githubOidcAvailable()) return minted();
    throw authError(env().target, flags.env ?? "production", flags.tokenEnv ?? "OXY_TOKEN");
  };

  return {
    cwd,
    flags,
    env,
    // `target: undefined` on purpose: it overrides `--env`, so carrying it
    // would point every rebuilt context at one host — which is the opposite of
    // what asking for a different env means.
    withEnv: (next: string) => createContext({ ...flags, env: next, target: undefined }, cwd),
    target: () => env().target,
    credential,
    bearer: async () => (await credential()).token,
    maybeBearer: async () => {
      const have = stored();
      if (have) return have.token;
      // No account named: no exchange to attempt, so no bearer — silently, as
      // in any job that never had `id-token: write`.
      if (!githubOidcAvailable() || !serviceAccount()) return undefined;
      try {
        return (await minted()).token;
      } catch (cause) {
        if (!(cause instanceof OidcExchangeError)) throw cause;
        // A deployment with no exchange is the old behaviour exactly — no
        // bearer, and nothing to say about it. Any other refusal is worth one
        // line, or the "not authenticated" that follows explains nothing.
        if (cause.oidcCode !== "unsupported") {
          once("oidc-warned", () => {
            log.warn(`GitHub OIDC is available, but the exchange was refused: ${cause.message}`);
            if (cause.hint) for (const line of cause.hint.split("\n")) log.hint(line);
          });
        }
        return undefined;
      }
    },
    storedBearer: () => stored()?.token,
    serviceAccount,
    apiKey: () => {
      const name = flags.apiKeyEnv ?? "OXY_API_KEY";
      return process.env[name]?.trim() || undefined;
    },
    customer,
    repoDir,
    placeholders: () =>
      once("placeholders", () => {
        const found = customer();
        const values: PlaceholderValues = {
          // `--org` wins, then the org a pasted `--env` URL named, then the
          // customer's own slug. The URL is ahead of the customer because
          // pasting an address bar is a deliberate act of naming an org.
          org: flags.org ?? env().orgSlug ?? found?.name,
          workspace: flags.workspace,
          project: flags.project,
          customer: found?.name,
          me: loadCredential(env().target)?.email
        };
        // A workspace id was not passed and the repo is here: the workspace is
        // a directory, not an id, so it cannot fill {workspace}. Left unset so
        // the placeholder error says how to find one rather than substituting
        // a path into a URL.
        return values;
      })
  };
}
