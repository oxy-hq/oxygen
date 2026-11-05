import { useState } from "react";

/**
 * Which home a person sees. `simple`: their apps and nothing else in the way, for
 * the people who come to open an app. `full`: Today beside the apps, the ask box,
 * recent questions and the data map, for the people who run the org from here.
 */
export type HomeView = "simple" | "full";

type OrgRole = "owner" | "admin" | "member" | null | undefined;

const key = (orgSlug: string) => `oxy.home.view.${orgSlug}`;

function read(orgSlug: string): HomeView | null {
  try {
    const stored = window.localStorage.getItem(key(orgSlug));
    return stored === "simple" || stored === "full" ? stored : null;
  } catch {
    return null;
  }
}

function write(orgSlug: string, view: HomeView) {
  try {
    window.localStorage.setItem(key(orgSlug), view);
  } catch {
    // Private mode or full storage: the choice lasts for this page only.
  }
}

/** Owners and admins run the org, so they start on the full home; everyone else on the simple one. */
export function defaultHomeView(role: OrgRole): HomeView {
  return role === "owner" || role === "admin" ? "full" : "simple";
}

/**
 * The person's home view in this org: what they last chose in this browser, else
 * the one their org role starts on. A choice is per org, since a person can run
 * one org and only use apps in another.
 */
export function useHomeView(
  orgSlug: string | undefined,
  role: OrgRole
): [HomeView, (view: HomeView) => void] {
  // Chosen during this page's life, by org, so a switch shows before storage is read again.
  const [chosen, setChosen] = useState<Record<string, HomeView>>({});
  const view = (orgSlug && (chosen[orgSlug] ?? read(orgSlug))) || defaultHomeView(role);
  const choose = (next: HomeView) => {
    if (!orgSlug) return;
    write(orgSlug, next);
    setChosen((was) => ({ ...was, [orgSlug]: next }));
  };
  return [view, choose];
}
