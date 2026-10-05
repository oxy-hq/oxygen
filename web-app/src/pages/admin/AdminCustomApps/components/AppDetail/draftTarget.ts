import type { CustomApp } from "@/types/apps";
import type { ChannelView } from "./components/DetailToolbar";

/**
 * What the console's two channel controls frame, decided once for the toolbar
 * and the stage.
 *
 * **Draft** is the app's staging environment, served on its staging host
 * (`staging--<org>--<slug>.customer-apps.<zone>/`). There is no other way to see
 * it: the staff `oxy_preview_draft` cookie that flipped the production URL to the
 * draft is retired, and the production URL is never loaded as "draft".
 *
 * `staging_url` is `null` for two different reasons, and the copy has to say which:
 * the deployment has no customer-apps zone at all (local dev — `url_subdomain` is
 * null too, from the same zone derivation), or this app has no staging host.
 *
 * **Published** frames the production URL. For an app that has never been
 * promoted that URL still serves something — its only build, with real writes —
 * so the control stays enabled as **Live** and says exactly that (`liveView`).
 * It never calls that build draft, staging or held: it is none of those.
 */
export type DraftTarget =
  | { kind: "staging"; url: string }
  /** The detail response (the only one carrying `staging_url`) has not landed. */
  | { kind: "pending" }
  | { kind: "unavailable"; reason: string };

export const STAGING_NOT_CONFIGURED =
  "Staging isn't configured on this deployment (no customer-apps zone)";

export const NO_STAGING_HOST = "No staging host for this app.";

export const DETAIL_LOAD_FAILED = "Couldn't load this app's staging host — reload to retry.";

export const NOT_PROMOTED_YET =
  "Not promoted yet — the live URL serves the only build, and its writes are real.";

/** Under a staging frame: it is another origin, so the console can't read it. */
export const STAGING_FRAME_NOTE =
  "Request log and navigation aren't available for the staging frame (different origin).";

/**
 * `detail` is the app's detail response, or `null` while it loads. `loadFailed`
 * is that request's error state: without it a failed fetch would read "pending"
 * for ever.
 */
export function draftTarget(
  detail: Pick<CustomApp, "staging_url" | "url_subdomain"> | null,
  loadFailed = false
): DraftTarget {
  if (!detail) {
    return loadFailed ? { kind: "unavailable", reason: DETAIL_LOAD_FAILED } : { kind: "pending" };
  }
  if (detail.staging_url) return { kind: "staging", url: detail.staging_url };
  if (!detail.url_subdomain) return { kind: "unavailable", reason: STAGING_NOT_CONFIGURED };
  return { kind: "unavailable", reason: NO_STAGING_HOST };
}

/** The Published control: always enabled, relabelled for an unpromoted app. */
export interface LiveView {
  label: "Published" | "Live";
  /** Shown on the control and under the frame; `null` for a promoted app. */
  note: string | null;
}

export function liveView(app: Pick<CustomApp, "published_at">): LiveView {
  return app.published_at
    ? { label: "Published", note: null }
    : { label: "Live", note: NOT_PROMOTED_YET };
}

/**
 * The channel a bare URL (no `?channel=`) opens on. A promoted app opens on
 * Published. An unpromoted one opens on its staging host when it has one, and
 * waits on Draft while that is still being looked up, so the production URL —
 * whose writes are real — is never framed only to be swapped out a moment later.
 * With no staging host it opens Live.
 */
export function defaultChannel(
  app: Pick<CustomApp, "published_at">,
  draft: DraftTarget
): ChannelView {
  if (app.published_at) return "published";
  return draft.kind === "unavailable" ? "published" : "draft";
}
