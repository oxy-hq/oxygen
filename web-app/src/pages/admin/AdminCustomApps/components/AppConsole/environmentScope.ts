import { PRODUCTION } from "@/types/appEnvironments";
import { environmentLabel, STAGING } from "../AppDetail/components/Secrets/environmentLabel";

/**
 * What the console says about the environment it is acting on.
 *
 * Production is the default and reads neutral; staging and sandboxes take the warn
 * tone. On this surface colour means "look at this", and acting somewhere other
 * than what customers use is exactly that. A run or a secret set while the strip is
 * amber does not touch production.
 */
export interface EnvironmentScope {
  name: string;
  /** "Production", "Staging", "Sandbox dev-luong". */
  label: string;
  tone: "muted" | "warn";
  /** What this environment is, as a sentence without its full stop. */
  what: string;
}

export function environmentScope(name: string): EnvironmentScope {
  if (name === PRODUCTION) {
    return {
      name,
      label: environmentLabel(name),
      tone: "muted",
      what: "What customers use"
    };
  }
  if (name === STAGING) {
    return {
      name,
      label: environmentLabel(name),
      tone: "warn",
      // Where `oxyc publish` lands (`published_to: staging`), and the staging
      // banner's own words for what a staging frame does with data.
      what: "Where a publish lands first, with live reads and writes held or isolated"
    };
  }
  return {
    name,
    label: environmentLabel(name),
    tone: "warn",
    what: "A developer's sandbox, created by oxyc"
  };
}

/** An option in the console's environment switcher: the label, and how many
 *  required secrets that environment is missing when it is known and non-zero. */
export function environmentOption(name: string, missing: number | undefined): string {
  const label = environmentLabel(name);
  return missing ? `${label} · ${missing} secret${missing === 1 ? "" : "s"} missing` : label;
}
