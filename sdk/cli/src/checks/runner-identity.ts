/**
 * Who is running a build's promote checks, as the app console shows it beside
 * the attempt: a GitHub Actions run with its link, or the same command run by
 * hand (then there is no link).
 *
 * Read from the variables GitHub sets in every job. Each is checked for its
 * shape before it becomes part of a link someone will click: a value that is
 * not what GitHub writes yields no link rather than a wrong one.
 */

import type { Runner } from "./promote-api.js";

const DIGITS = /^[0-9]{1,20}$/;
/** `owner/name`, as GitHub spells a repository. */
const REPOSITORY = /^[A-Za-z0-9_.-]{1,100}\/[A-Za-z0-9_.-]{1,100}$/;

export function runnerIdentity(env: NodeJS.ProcessEnv = process.env): Runner {
  if (env.GITHUB_ACTIONS !== "true") return { label: "oxyc, run by hand", url: null };
  const number = env.GITHUB_RUN_NUMBER ?? "";
  const label = DIGITS.test(number) ? `GitHub Actions run #${number}` : "GitHub Actions";
  return { label, url: runUrl(env) };
}

function runUrl(env: NodeJS.ProcessEnv): string | null {
  const [server, repository, run] = [
    env.GITHUB_SERVER_URL ?? "",
    env.GITHUB_REPOSITORY ?? "",
    env.GITHUB_RUN_ID ?? ""
  ];
  if (!DIGITS.test(run) || !REPOSITORY.test(repository)) return null;
  let origin: string;
  try {
    const parsed = new URL(server);
    if (parsed.protocol !== "https:") return null;
    origin = parsed.origin;
  } catch {
    return null;
  }
  return `${origin}/${repository}/actions/runs/${run}`;
}
