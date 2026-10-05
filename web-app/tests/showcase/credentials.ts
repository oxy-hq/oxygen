// The credentials a showcase run holds, taken OUT of the environment the
// moment the run starts.
//
// The agentic runner expands any `${NAME}` in a step, and in a recorded
// action's arguments, from `process.env` (runner/secrets.ts) — then types it.
// A step is model output shaped by a PR's title, description and diff. A
// credential left in the environment is one sentence in a PR description away
// from being typed into a page, filmed, and posted under a release
// announcement. So nothing a capture can name is there to expand: the keys
// live here and are handed to the calls that need them.

const NAMES = ["ANTHROPIC_API_KEY", "SLACK_BOT_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"] as const;
export type CredentialName = (typeof NAMES)[number];

const held = new Map<CredentialName, string>();

/** Move every credential out of the environment. Call once, before anything else runs. */
export function takeCredentials(env: NodeJS.ProcessEnv = process.env): void {
  for (const name of NAMES) {
    const value = env[name];
    if (value) held.set(name, value);
    delete env[name];
  }
}

export function credential(name: CredentialName): string | undefined {
  return held.get(name);
}

/** What a `gh` child needs to sign in. Empty off a runner, where gh has its own stored login. */
export function ghAuthEnv(): Record<string, string> {
  const token = held.get("GH_TOKEN") ?? held.get("GITHUB_TOKEN");
  return token ? { GH_TOKEN: token } : {};
}
