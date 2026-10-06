import { appCountProblem, hoursProblem } from "@/libs/sandboxAgentToken";
import type {
  CreateSandboxAgentTokenRequest,
  SandboxAgentLimits,
  SandboxApp
} from "@/types/apiToken";

/** What the person picked for the lifetime: one of the spans on offer, or hours typed in. */
export type LifetimeChoice = { kind: "preset"; hours: number } | { kind: "custom"; text: string };

export interface SandboxDraft {
  appIds: string[];
  /**
   * `null` until the person picks: the lifetime is then the server's default, which arrives
   * with the options after the form has already opened.
   */
  lifetime: LifetimeChoice | null;
}

export const emptySandboxDraft = (): SandboxDraft => ({ appIds: [], lifetime: null });

/** The lifetime the form shows: the person's pick, or the server's default before one. */
export const effectiveLifetime = (
  draft: Pick<SandboxDraft, "lifetime">,
  limits: SandboxAgentLimits
): LifetimeChoice => draft.lifetime ?? { kind: "preset", hours: limits.default_hours };

/** The hours a choice comes to, or `null` while the custom box holds no whole number. */
export const lifetimeHours = (choice: LifetimeChoice): number | null => {
  if (choice.kind === "preset") return choice.hours;
  const text = choice.text.trim();
  return /^\d+$/.test(text) ? Number(text) : null;
};

/** Why the lifetime can't be used, or `null`. An empty custom box is unfinished, not wrong. */
export const lifetimeProblem = (
  choice: LifetimeChoice,
  limits: SandboxAgentLimits
): string | null => {
  const hours = lifetimeHours(choice);
  if (hours !== null) return hoursProblem(hours, limits);
  return choice.kind === "custom" && choice.text.trim() ? "Enter a whole number of hours." : null;
};

/**
 * The picks that are still on offer, in the order they were made. An app the caller lost the
 * reach to build drops out of the options, and so out of the picks.
 */
export const pickedApps = (appIds: readonly string[], apps: readonly SandboxApp[]): SandboxApp[] =>
  appIds.flatMap((id) => apps.find((app) => app.id === id) ?? []);

/** Tick or untick one app. A tick past the limit changes nothing: the picker disables it too. */
export const toggleApp = (
  appIds: readonly string[],
  id: string,
  on: boolean,
  limits: SandboxAgentLimits
): string[] => {
  const without = appIds.filter((picked) => picked !== id);
  if (!on) return without;
  return without.length >= limits.max_apps ? [...appIds] : [...without, id];
};

/**
 * The mint body, or `undefined` while something is missing or out of range. It carries none of
 * a personal token's fields: with `kind: "sandbox_agent"` the server refuses any of them.
 */
export const sandboxMintRequest = (
  name: string,
  picked: readonly SandboxApp[],
  lifetime: LifetimeChoice,
  limits: SandboxAgentLimits
): CreateSandboxAgentTokenRequest | undefined => {
  const hours = lifetimeHours(lifetime);
  if (!name.trim() || hours === null) return undefined;
  if (hoursProblem(hours, limits) || appCountProblem(picked.length, limits)) return undefined;
  return {
    name: name.trim(),
    kind: "sandbox_agent",
    apps: picked.map((app) => app.id),
    expires_in_hours: hours
  };
};
