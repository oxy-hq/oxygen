/**
 * The app-environment name grammar `oxyc` validates client-side.
 *
 * Mirrors `oxy_app_core::custom_app_environment` — the database's CHECK
 * constraint (`app_environments_name_matches_kind`) carries the same rule, so
 * a malformed `--app-env` is a usage error (exit 2) here rather than a round
 * trip that comes back `400 invalid_environment_name`.
 */

import { usageError } from "../util/errors.js";

/** Mirrors `custom_app_env_request::request_environment`'s header name. */
export const APP_ENV_HEADER = "X-Oxy-App-Env";

/** `oxy_app_core::custom_app_environment::DEV_HANDLE_MAX_LEN`. */
const DEV_HANDLE_MAX_LEN = 12;
const HANDLE_CHARS_RE = /^[a-z0-9-]+$/;

/**
 * Lowercase ASCII letters, digits and single hyphens; 1..=12 characters; no
 * leading or trailing hyphen, no `--` (the host delimiter).
 */
function isValidDevHandle(handle: string): boolean {
  return (
    handle.length > 0 &&
    handle.length <= DEV_HANDLE_MAX_LEN &&
    HANDLE_CHARS_RE.test(handle) &&
    !handle.startsWith("-") &&
    !handle.endsWith("-") &&
    !handle.includes("--")
  );
}

/** `production`, `staging` or `dev-<handle>`; throws usageError (exit 2) otherwise. */
export function parseAppEnv(value: string): string {
  if (value === "production" || value === "staging") return value;
  if (value.startsWith("dev-") && isValidDevHandle(value.slice(4))) return value;
  throw usageError(
    `--app-env ${JSON.stringify(value)} is not a valid environment`,
    "production, staging, or dev-<handle> — 1-12 lowercase letters, digits and single " +
      "hyphens, not starting or ending with one"
  );
}

/** A `dev-<handle>` name only; throws usageError otherwise. */
export function requireSandboxName(value: string): string {
  const parsed = parseAppEnv(value);
  if (parsed === "production" || parsed === "staging") {
    throw usageError(
      `${parsed} is not a sandbox`,
      "name a dev-<handle> environment — production and staging always exist and are never created or deleted this way"
    );
  }
  return parsed;
}

/** `undefined` (no `--app-env`) and `"production"` both mean production. */
export function isProduction(appEnv: string | undefined): boolean {
  return appEnv === undefined || appEnv === "production";
}
