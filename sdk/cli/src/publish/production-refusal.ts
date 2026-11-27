/**
 * A promoting publish (`oxyc publish --promote`) that production refused.
 *
 * The server answers `409` in the body a refused `oxyc promote` answers —
 * `{code, error, message, blockers: [{code, message}]}` — with every blocker
 * the promote's plan would list: an unset required secret, a schedule
 * retirement nobody confirmed, the app's four-eyes rule, the build's checks
 * where the org holds promotes on them. Nothing of the
 * publish landed, staging included. This turns that body into an error that
 * names each blocker and says, for each, what to set or which flag to pass.
 */

import type { Blocker } from "../commands/promote.js";
import { CliError, ExitCode } from "../util/errors.js";

/** The blockers of a refused promoting publish, or `undefined` for any other body. */
export function blockersOf(body: string): Blocker[] | undefined {
  let parsed: unknown;
  try {
    parsed = JSON.parse(body);
  } catch {
    return undefined;
  }
  if (parsed === null || typeof parsed !== "object") return undefined;
  const { blockers } = parsed as { blockers?: unknown };
  if (!Array.isArray(blockers) || blockers.length === 0) return undefined;
  const valid = blockers.filter(
    (b): b is Blocker =>
      typeof b === "object" &&
      b !== null &&
      typeof (b as Blocker).code === "string" &&
      typeof (b as Blocker).message === "string"
  );
  return valid.length > 0 ? valid : undefined;
}

/** What to do about one blocker, said for a publish of `app` (`<org>/<app>`). */
export function remedyFor(code: string, app: string): string | undefined {
  switch (code) {
    case "required_secrets_unset":
      return "set each key named above in production — the console's Secrets panel, on Production (`oxyc env secret` sets a sandbox's only) — then publish again";
    case "schedule_removal_unconfirmed":
      return "to retire them, add `--remove-schedule <function>` once for each function named above";
    case "same_person_promote":
      return `this app requires a second person to promote: publish without --promote, then someone else runs \`oxyc promote ${app}\``;
    case "event_listeners_broken":
      return "this build removes or incompatibly changes an event a production function listens to: keep the event, or add `--break-listeners` to ship anyway (audited)";
    case "validation_not_passed":
      return "publish a build that passes validation";
    case "staging_never_served":
      return `publish without --promote so staging serves the build, then \`oxyc promote ${app}\``;
    // The build's checks run once staging serves it, so a promoting publish never has them.
    case "checks_not_passed":
    case "checks_running":
      return `this app's promotes wait on the build's checks, which run once staging serves it: publish without --promote, wait for them (\`oxyc checks show ${app} --wait\`), then \`oxyc promote ${app}\``;
    case "changes_not_seen":
      return `publish without --promote, look at the screens that differ on the app's Builds page and mark them seen, then \`oxyc promote ${app}\``;
    default:
      return undefined;
  }
}

/** The error for a refused promoting publish; `undefined` when `body` is not one. */
export function productionRefused(status: number, body: string, app: string): CliError | undefined {
  const blockers = blockersOf(body);
  if (!blockers) return undefined;
  const remedies = blockers
    .map((b) => remedyFor(b.code, app))
    .filter((r): r is string => r !== undefined);
  const hint = [
    ...remedies,
    `nothing was published, staging included — or publish without --promote to land on staging, and promote it with \`oxyc promote ${app}\``
  ].join("\n");
  return new CliError(
    `production refused this promoting publish (${status}): ${blockers.map((b) => b.code).join(", ")}`,
    {
      code: ExitCode.REQUEST,
      detail: blockers.map((b) => `${b.code}: ${b.message}`).join("\n"),
      hint,
      serverCode: blockers[0]?.code,
      serverMessage: blockers[0]?.message
    }
  );
}
