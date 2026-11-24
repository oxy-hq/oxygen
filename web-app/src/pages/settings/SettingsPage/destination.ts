import type { SettingsSection } from "@/components/settings/settingsRoute";

/** What the settings route should show for a given address. */
export type SettingsDestination =
  | { kind: "nothing" }
  /** Send the viewer to `section` instead — the index on a wide screen, or a page they lack. */
  | { kind: "redirect"; section: SettingsSection }
  /** The nav on its own, which is what the index is on a phone. */
  | { kind: "list" }
  | { kind: "section"; section: SettingsSection };

export interface DestinationInput {
  /** The section the address names, or `null` on the index. */
  section: SettingsSection | null;
  /** Sections this viewer can currently see, in nav order. */
  visible: SettingsSection[];
  isDesktop: boolean;
  /**
   * Whether the org, role and workspace the nav gates on have answered. Before they have, a
   * section missing from `visible` is missing because nothing has loaded.
   */
  contextReady: boolean;
}

/**
 * Where a settings address resolves to.
 *
 * Pure and exported because it is the one place two awkward cases meet, and they pull in
 * opposite directions: a viewer must not be left on a page they cannot open, and a deep link
 * must not be thrown away just because the gates have not answered yet. Deciding that inside
 * the component meant reading its branches to know which was which.
 */
export function settingsDestination({
  section,
  visible,
  isDesktop,
  contextReady
}: DestinationInput): SettingsDestination {
  const first = visible[0];

  if (!section) {
    // The index is a real place on a phone — it is the list. On a wide screen the list is
    // always on screen beside the content, so there is nothing to stay here for.
    if (!isDesktop) return { kind: "list" };
    return first ? { kind: "redirect", section: first } : { kind: "nothing" };
  }

  if (visible.includes(section)) return { kind: "section", section };

  // Not in the nav. Only meaningful once everything it gates on has answered.
  if (!contextReady) return { kind: "section", section };
  return first ? { kind: "redirect", section: first } : { kind: "nothing" };
}
