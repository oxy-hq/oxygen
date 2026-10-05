import { useCallback, useEffect, useRef, useState } from "react";
import { cn } from "@/libs/shadcn/utils";
import type { CustomApp } from "@/types/apps";
import { resolveBundleUrl } from "../../../../resolveBundleUrl";
import { fromPreviewPath, PREVIEW_NONCE_PARAM, toPreviewPath } from "../../appViewState";
import { type DraftTarget, draftTarget, liveView, STAGING_FRAME_NOTE } from "../../draftTarget";
import { StagingBanner } from "../../StagingBanner";
import type { ChannelView, Device } from "../DetailToolbar";
import { DebugPanel } from "./DebugPanel";
import { applying, type DeepLinkHandoff, landed, report } from "./deepLinkHandoff";
import { PreviewNav } from "./PreviewNav";
import { useOxyRequestLog } from "./useOxyRequestLog";
import { usePreviewHistory } from "./usePreviewHistory";

/**
 * Iframe canvas for the custom-app preview. All chrome (device
 * selector, channel toggle, reload, URL) lives in the unified
 * `DetailToolbar` one row up — this component owns only the canvas
 * + device frame, so the visible vertical real estate goes to the
 * actual custom app.
 *
 * Two visual modes:
 *   - **Desktop**: full-bleed. iframe fills the pane edge-to-edge,
 *     no border / shadow / padding. You're looking AT the customer
 *     app, not at "an iframe of one". This is the immersive default
 *     because most custom apps are rendered for desktop.
 *   - **Mobile / Tablet**: device-shaped card on a dotted backdrop
 *     with a soft accent halo underneath. Aspect ratios are real
 *     (iPhone-ish 9:19.5, iPad-ish 3:4) so a long page scrolls inside
 *     the frame instead of stretching to fit. A mono dimensions
 *     label sits below the device for context.
 *
 * Controlled by `AppDetail` — device / channel / nonce are props so
 * the toolbar one row up can drive them without remounting the iframe.
 */
export interface LivePreviewProps {
  app: CustomApp;
  device: Device;
  /** Used purely to force a fresh iframe navigation; bumped when the
   *  caller wants to refetch (reload, channel toggle). */
  nonce: number;
  /** The toolbar's selected channel. `published` frames the app's own URL;
   *  `draft` frames the app's **staging host** (`draft`), never the production
   *  URL. Also part of the iframe `key`, so a flip always remounts the frame. */
  channel: ChannelView;
  /** Where Draft points (see `draftTarget.ts`). Defaults to what `app` itself
   *  says, which for a registry-list row is "pending": only the detail response
   *  carries `staging_url`. */
  draft?: DraftTarget;
  /**
   * Where inside the app to point the preview, app-relative
   * (`/?vendor=ubereats`). Comes off the admin console's own query string, so
   * an admin link reproduces the previewed screen and not just the app.
   *
   * Applied once per document — after that the operator (or the app) owns the
   * location, and re-applying on every render would fight them.
   */
  path: string | null;
  /** Report where the preview moved, app-relative, so the admin URL can follow
   *  it. `null` when it left the app's own prefix. */
  onPathChange: (path: string | null) => void;
}

/**
 * Device specs. Widths follow common v0 / Vercel preview conventions;
 * the aspect ratios are the real device shapes so a long custom app
 * scrolls inside the device frame instead of stretching.
 */
const DEVICE_SPECS: Record<
  Device,
  { width: number | "100%"; aspect: string; label: string; cornerRadius: string }
> = {
  mobile: {
    width: 390,
    aspect: "9 / 19.5",
    label: "390 × 844",
    cornerRadius: "2rem"
  },
  tablet: {
    width: 820,
    aspect: "3 / 4",
    label: "820 × 1180",
    cornerRadius: "1.25rem"
  },
  desktop: {
    width: "100%",
    aspect: "auto",
    label: "",
    cornerRadius: "0"
  }
};

export const LivePreview = ({
  app,
  device,
  nonce,
  channel,
  draft,
  path,
  onPathChange
}: LivePreviewProps) => {
  const target = channel === "draft" ? (draft ?? draftTarget(app)) : null;
  // Published: resolveBundleUrl rewrites the host to the admin shell's own
  // origin so cookies travel through Vite's /customer-apps proxy in dev (and is
  // a no-op in production where admin + bundles share an origin).
  // Draft: the staging host, as the server named it. It is another origin, so it
  // is NOT rewritten — the shared `.oxygen-hq.com` session cookie is same-site
  // and authenticates the frame — and the request log / preview history, which
  // need a same-origin frame, report themselves unavailable.
  // `null` when Draft has nowhere to point: the stage says why instead.
  const frameBase =
    target === null ? resolveBundleUrl(app.url) : target.kind === "staging" ? target.url : null;
  const frame = frameBase ? new URL(frameBase) : null;
  const base = frame?.pathname ?? "/";
  const frameOrigin = frame?.origin ?? window.location.origin;
  const onStaging = target?.kind === "staging";

  // The frame-key changes on a channel flip or a reload, each a new document.
  const frameKey = `${channel}-${nonce}`;
  // The deep link a staging frame starts at. A cross-origin frame can't be
  // steered once loaded (`applyPath` can't reach it), so the `?preview=` path
  // goes into its first URL instead. Captured once per document: re-deriving it
  // on every `path` change would swap `src` and reload the frame under the
  // operator. A same-origin frame starts at the root and is steered on load.
  const initialPath = useRef<{ key: string; path: string | null }>({ key: frameKey, path });
  if (initialPath.current.key !== frameKey) initialPath.current = { key: frameKey, path };
  const startPath = onStaging ? initialPath.current.path : null;

  // The nonce query param cache-busts the iframe — bumped on Reload, on channel
  // flip, and on first mount — without changing the path.
  const previewUrl = (() => {
    if (!frameBase) return null;
    const start = startPath ? fromPreviewPath(startPath, base, frameOrigin) : null;
    const u = new URL(start ?? frameBase);
    u.searchParams.set(PREVIEW_NONCE_PARAM, String(nonce));
    return u.toString();
  })();
  // Under the frame: why the console can't follow a staging frame, or that an
  // unpromoted app's live URL takes real writes.
  const frameNote = onStaging ? STAGING_FRAME_NOTE : target === null ? liveView(app).note : null;

  // Same-origin preview → instrument the iframe to capture its calls to oxy.
  const { entries, clear, handleLoad, available } = useOxyRequestLog();
  const previewHistory = usePreviewHistory();
  const [panelOpen, setPanelOpen] = useState(false);

  // The last path this component either applied or heard back from the frame —
  // in other words, where the preview IS. `path` is compared against this
  // rather than against "have we applied a deep link yet", which is the version
  // that snapped the frame backwards: a full-document navigation inside the app
  // reloads the frame while the admin URL still names the previous location, so
  // an unwritten marker made every such click bounce to where it came from.
  //
  // Comparing against the real position also makes the admin console's own Back
  // work on the preview: an entry whose `?preview` differs from where the frame
  // sits is a location the operator asked to return to, so the frame follows.
  const framePath = useRef<string | null>(null);

  /** The deep-link handoff — see `deepLinkHandoff.ts` for the rule and for the
   *  two exits that keep the suppression bounded to one document. */
  const handoff = useRef<DeepLinkHandoff>(null);

  // The frame is remounted (not re-navigated) on a channel flip or a reload —
  // see the `key` on each `<iframe>`. That is a different document, so the
  // stack that described the old one has to go with it, and the deep link
  // becomes applicable again.
  const lastFrameKey = useRef(frameKey);
  const resetHistory = previewHistory.reset;
  useEffect(() => {
    if (lastFrameKey.current !== frameKey) {
      lastFrameKey.current = frameKey;
      resetHistory();
      framePath.current = null;
      handoff.current = landed();
    }
  }, [frameKey, resetHistory]);

  const goPreview = previewHistory.go;
  const applyPath = useCallback(
    (next: string) => {
      if (framePath.current === next) return;
      const url = fromPreviewPath(next, base, frameOrigin);
      // `null` means the stored path did not resolve inside this app — a `..`
      // that climbed out of the bundle prefix. Ignore it rather than pointing
      // the frame at whatever it normalised to.
      if (!url) return;
      // Record only what happened. `none` is the ORDINARY answer for a deep
      // link: the admin URL is read on mount and the iframe has not fired
      // `load`. Recording anyway made `framePath` claim a position the frame
      // never took, so the apply on first load early-returned as "already
      // there" and the link was dropped — the operator opened a shared
      // `?preview=` link and got the app root.
      const kind = goPreview(url);
      if (kind === "none") return;
      framePath.current = next;
      // Only a cross-document move passes through a document worth suppressing.
      // Arming the handoff for a fragment move would latch it until the next
      // load, because no load is coming and the report it does produce may
      // normalise to a different string than the one applied.
      handoff.current = kind === "cross-document" ? applying(next) : landed();
    },
    [base, frameOrigin, goPreview]
  );

  const onIframeLoad = useCallback(
    (el: HTMLIFrameElement) => {
      // A new document means whatever was in flight has landed — wherever it
      // landed. This is the exit that bounds the suppression to one document,
      // and the frame-key effect above calls the same thing for the other way a
      // fresh document arrives (a channel flip or a reload remounts the frame).
      handoff.current = landed();
      handleLoad(el);
      previewHistory.handleLoad(el);
      if (path) applyPath(path);
    },
    [handleLoad, previewHistory.handleLoad, path, applyPath]
  );

  // The admin URL moved (Back, Forward, or a pasted link) to a location the
  // frame is not at. Follow it — the URL is the state.
  useEffect(() => {
    if (path) applyPath(path);
  }, [path, applyPath]);

  // Push the preview's location back up so the admin URL follows it. Reported
  // app-relative: the admin console stores a path, never an absolute URL, so a
  // link cannot aim the frame at another origin.
  const previewUrlNow = previewHistory.url;
  useEffect(() => {
    if (!previewUrlNow) return;
    const next = toPreviewPath(previewUrlNow, base);
    const decision = report(handoff.current, next);
    handoff.current = decision.next;
    if (!decision.publish) return;
    // Recording where the frame is, not just publishing it: this is the write
    // that keeps `applyPath` from treating the app's own move as a mismatch and
    // navigating back over it.
    framePath.current = next;
    onPathChange(next);
  }, [previewUrlNow, base, onPathChange]);

  // After every hook: the frame-key effect and the history hooks must run the
  // same number of times whether or not there is a frame.
  if (!previewUrl) return <DraftNotice target={target} />;

  return (
    <div className='flex min-h-0 flex-1 flex-col'>
      {/* The app's own back/forward. The window's controls walk the ADMIN
          console — that separation is the whole point, and it only reads as
          deliberate if the other history has visible controls of its own. */}
      <PreviewNav
        history={previewHistory}
        path={previewUrlNow ? toPreviewPath(previewUrlNow, base) : null}
      />
      {/* Only while the frame is actually the staging host — never over the
          production/Live frame below, and never for a pending/unavailable
          target (those never reach this return at all; see the early
          `!previewUrl` exit above). */}
      {onStaging && <StagingBanner appId={app.id} />}
      <div className='flex min-h-0 flex-1 flex-col'>
        {device === "desktop" ? (
          <DesktopFrame
            app={app}
            url={previewUrl}
            channel={channel}
            nonce={nonce}
            onIframeLoad={onIframeLoad}
          />
        ) : (
          <DeviceFrame
            app={app}
            url={previewUrl}
            device={device}
            channel={channel}
            nonce={nonce}
            onIframeLoad={onIframeLoad}
          />
        )}
      </div>
      {frameNote && (
        <p
          className='shrink-0 border-t bg-muted/30 px-3 py-1 text-muted-foreground text-xs'
          data-testid='admin-app-frame-note'
        >
          {frameNote}
        </p>
      )}
      <DebugPanel
        entries={entries}
        onClear={clear}
        open={panelOpen}
        onToggle={() => setPanelOpen((o) => !o)}
        available={available}
      />
    </div>
  );
};

/**
 * The stage when Draft has nothing to frame. Says why in the same words as the
 * disabled toolbar control, and never falls back to the production URL — that
 * would show the published build under a "draft" label.
 */
const DraftNotice = ({ target }: { target: DraftTarget | null }) => (
  <div
    className='flex min-h-0 flex-1 items-center justify-center bg-muted/30 p-8'
    data-testid='admin-app-draft-notice'
  >
    <div className='max-w-sm space-y-1.5 rounded-lg border bg-background p-4 text-center'>
      <p className='font-medium text-sm'>
        {target?.kind === "unavailable" ? "No staging view" : "Looking up the staging host…"}
      </p>
      {target?.kind === "unavailable" && (
        <p className='text-muted-foreground text-xs'>{target.reason}</p>
      )}
    </div>
  </div>
);

/**
 * Full-bleed desktop. No card, no border, no padding — just the
 * iframe filling the pane. The toolbar's bottom border is the only
 * separator. This is the immersive case: you forget you're in an
 * admin shell.
 */
const DesktopFrame = ({
  app,
  url,
  channel,
  nonce,
  onIframeLoad
}: {
  app: CustomApp;
  url: string;
  channel: ChannelView;
  nonce: number;
  onIframeLoad: (el: HTMLIFrameElement) => void;
}) => (
  <div className='min-h-0 flex-1 bg-background'>
    <iframe
      onLoad={(e) => onIframeLoad(e.currentTarget)}
      // No `sandbox` attribute: this preview loads the app's own URL or
      // its staging host — code we ship — and the app needs cookies +
      // localStorage + same-origin fetch to function.
      // The only sandbox tokens that'd let those through
      // (`allow-same-origin` + `allow-scripts`) are documented by the
      // HTML spec as "effectively no sandbox" — combining them lets
      // the framed page reach into its parent and remove its own
      // sandbox. So the attribute was security theater. Trust model:
      // we ship the bundle code; origin gating is what actually
      // protects the admin shell.
      key={`desktop-${channel}-${nonce}`}
      src={url}
      title={`${app.name} live preview`}
      className='size-full border-0'
    />
  </div>
);

/**
 * Mobile / tablet device frame. Centered on a dotted backdrop with a
 * soft accent halo beneath the device so it reads as a physical
 * object floating on a stage, not a constrained div.
 */
const DeviceFrame = ({
  app,
  url,
  device,
  channel,
  nonce,
  onIframeLoad
}: {
  app: CustomApp;
  url: string;
  device: Exclude<Device, "desktop">;
  channel: ChannelView;
  nonce: number;
  onIframeLoad: (el: HTMLIFrameElement) => void;
}) => {
  const spec = DEVICE_SPECS[device];

  return (
    <div
      className='relative flex min-h-0 flex-1 items-center justify-center overflow-auto bg-muted/30 p-8'
      style={{
        // Dotted grid backdrop — Vercel-preview-style. Inline so the
        // dot color tracks the theme's border token; --border resolves
        // to oklch in both light and dark.
        backgroundImage: "radial-gradient(circle, var(--border) 1px, transparent 1px)",
        backgroundSize: "22px 22px"
      }}
    >
      {/* Soft accent halo beneath the device. Sits behind everything
          (negative z) with heavy blur — gives the device a faint glow
          ground so it doesn't read as floating in a vacuum. Color
          ties to the brand primary so the eye reads it as intentional
          rather than incidental. */}
      <div
        aria-hidden
        className='pointer-events-none absolute top-1/2 left-1/2 size-[60%] -translate-x-1/2 rounded-full bg-primary/15 opacity-60 blur-3xl'
      />

      <div
        className='relative flex max-h-full flex-col items-center'
        style={{ width: typeof spec.width === "number" ? `${spec.width}px` : spec.width }}
      >
        {/* Device card. Aspect-ratio + max-height keep the device
            real-shape while shrinking to fit short panes. The inner
            ring (`ring-1 ring-foreground/5`) reads as a thin bezel
            without going full-skeuomorphic phone art. */}
        <div
          className={cn(
            "relative w-full overflow-hidden bg-background shadow-2xl ring-1 ring-foreground/5 transition-[max-width,border-radius] duration-300",
            "shadow-foreground/20"
          )}
          style={{
            aspectRatio: spec.aspect,
            maxHeight: "calc(100% - 2.5rem)",
            borderRadius: spec.cornerRadius
          }}
        >
          <iframe
            // See DesktopFrame's comment — same reasoning: drop the
            // sandbox attribute rather than combine the two tokens
            // that neutralise each other.
            onLoad={(e) => onIframeLoad(e.currentTarget)}
            key={`${device}-${channel}-${nonce}`}
            src={url}
            title={`${app.name} live preview`}
            className='size-full border-0'
          />
        </div>

        {/* Quiet dimensions label. Mono + uppercase, low opacity,
            sits just below the device — reads as a caption, not a
            control. Confirms the frame is a literal device size. */}
        <span className='mt-3 font-mono text-muted-foreground/70 text-xs uppercase tracking-[0.15em]'>
          {spec.label}
        </span>
      </div>
    </div>
  );
};
