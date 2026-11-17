/**
 * The words every lifecycle action confirms with — one copy, read by the console, the
 * dossier, the settings panel and both partner panels.
 *
 * Before this, "publish" was a Switch with no confirmation on the partner console, a
 * "Re-publish" button in Settings, and "Make live" in the build history; the same
 * pointer move read three different ways depending on which panel you were on. A
 * confirm names what customers will see next and that it can be undone here, in one
 * plain sentence, and nothing else.
 */
export interface ConfirmCopy {
  title: string;
  description: string;
  confirm: string;
}

/** Git-sha-style short id; leaves already-short ids untouched. */
export const shortBuildId = (s: string) => (s.length > 10 ? s.slice(0, 7) : s);

export const LIFECYCLE_COPY = {
  /** Point production at the build staging is serving. */
  promote: (draft: string, live: string | null): ConfirmCopy => ({
    title: "Promote staging to production?",
    description: `Customers will see build ${shortBuildId(draft)}${
      live ? ` instead of ${shortBuildId(live)}` : ""
    } immediately. You can roll back here at any time.`,
    confirm: "Promote to production"
  }),

  /** First publish: the app appears for the org's members. `null` when the surface
   *  does not know the org's slug (a partner's per-org panel). */
  publish: (orgSlug: string | null): ConfirmCopy => ({
    title: "Publish this app?",
    description: `Members of ${orgSlug ?? "this organization"} can open it immediately. You can unpublish here at any time.`,
    confirm: "Publish"
  }),

  /** Move the live pointer to a retained build. */
  rollback: (target: string, live: string | null): ConfirmCopy => ({
    title: "Roll back to this build?",
    description: `Customers will see build ${shortBuildId(target)}${
      live ? ` instead of ${shortBuildId(live)}` : ""
    } immediately. You can switch again here at any time.`,
    confirm: "Roll back"
  }),

  unpublish: (): ConfirmCopy => ({
    title: "Unpublish this app?",
    description:
      "Customers lose access immediately. The builds stay; you can publish again here at any time.",
    confirm: "Unpublish"
  }),

  /** The fleet's batch action: every selected app's newest build goes live. */
  promoteLatest: (count: number): ConfirmCopy => ({
    title: `Promote the latest build on ${count} app${count === 1 ? "" : "s"}?`,
    description:
      "Each app's newest build goes live immediately. Apps already serving their newest build are unchanged. You can roll any of them back from its console.",
    confirm: "Promote latest"
  })
} as const;
