// The wired workspace shell for custom-app bundles: the same icon rail +
// universal top bar the main web-app renders, fed by the bundle-gated
// shell-context endpoint. Wrap the app's content with it inside
// <OxyAppProvider>:
//
//   <OxyAppProvider>
//     <OxyShell>
//       <Dashboard />
//     </OxyShell>
//   </OxyAppProvider>
//
// Degradation contract: chrome must never take the app down. While the
// bootstrap request is in flight the frame renders with placeholders; if it
// fails (older server, unauthenticated viewer), the children render bare.

import { type CSSProperties, type ReactNode, useEffect, useState } from "react";
import { useOxyApp, useResolvedManifest } from "../custom-app/react";
import { AskDock } from "./AskDock";
import { cx } from "./cx";
import { useDockPrefetch } from "./dockPrefetch";
import { OxyMark } from "./marks";
import { NavSheetPanel, NavSheetTrigger, useNavSheet } from "./NavSheet";
import { ShellPortalContext, useShellPortalContainer } from "./portal";
import { type RailItem, ShellRail } from "./ShellRail";
import { type ShellContextData, useShellContext } from "./shellContext";
import { Breadcrumb, SystemIndicator, TopBar, WorkspaceClock } from "./TopBar";
import { UpdatePill } from "./UpdatePill";
import { WorkspaceTile } from "./WorkspaceTile";

/** Home/HQ glyph (lucide "house" outline, inlined). */
function HouseIcon() {
  return (
    <svg
      width='16'
      height='16'
      viewBox='0 0 24 24'
      fill='none'
      stroke='currentColor'
      strokeWidth='2'
      strokeLinecap='round'
      strokeLinejoin='round'
      aria-hidden='true'
    >
      <path d='M15 21v-8a1 1 0 0 0-1-1h-4a1 1 0 0 0-1 1v8' />
      <path d='M3 10a2 2 0 0 1 .709-1.528l7-5.999a2 2 0 0 1 2.582 0l7 5.999A2 2 0 0 1 21 10v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z' />
    </svg>
  );
}

/** Chat glyph (lucide "messages-square" outline, inlined). */
function MessagesIcon() {
  return (
    <svg
      width='16'
      height='16'
      viewBox='0 0 24 24'
      fill='none'
      stroke='currentColor'
      strokeWidth='2'
      strokeLinecap='round'
      strokeLinejoin='round'
      aria-hidden='true'
    >
      <path d='M16 10a2 2 0 0 1-2 2H6l-4 4V4a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2z' />
      <path d='M20 9a2 2 0 0 1 2 2v11l-4-4h-6a2 2 0 0 1-2-2v-1' />
    </svg>
  );
}

export interface OxyShellProps {
  children: ReactNode;
  /** Breadcrumb page label; defaults to this app's registered name. */
  pageLabel?: string;
  /** Replace the top bar's left side (the default workspace/page
   *  breadcrumb) with the app's own brand/header content. Host-provided
   *  slots keep rendering even when the shell bootstrap is unavailable,
   *  so the app never loses its header by adopting the shell. */
  topBarLeft?: ReactNode;
  /** Extra right-side top bar content, rendered after the status cluster
   *  (Sys · clock) and before the Ask Oxygen button. */
  topBarExtra?: ReactNode;
  /** Rail bottom slot content. The shell adds no built-in entries here —
   *  a bundle that wants a Settings link supplies its own. */
  railBottom?: ReactNode;
  /** Drop the top bar and keep only the rail. */
  hideTopBar?: boolean;
  /** Bind ⌘K / Ctrl+K to toggle the Ask dock (default true). Set false
   *  when the host app owns that shortcut; the top-bar button still works. */
  askHotkey?: boolean;
  /** Origin of the Oxygen product (e.g. `https://app.oxygen-hq.com`).
   *  shell-context returns product links as relative paths, which resolve
   *  correctly when oxy serves the bundle (production, org subdomain). In
   *  local dev the bundle runs on its own Vite origin, so those paths would
   *  resolve to the app's origin, not oxy's — set this to the oxy origin
   *  and the shell rewrites Home / Chat / Settings / app links to absolute
   *  URLs. Leave unset in production. */
  productBaseUrl?: string;
  className?: string;
  /** Background of the shell chrome — the icon rail, the top bar, and the
   *  AskDock (they share one token). Any CSS color, including `var(--…)` to
   *  track a host token (e.g. `var(--background)` to match the app's canvas).
   *  Overrides the theme's default; leave unset to keep it. */
  chromeBackground?: string;
  /** Text/icon color on the chrome, paired with `chromeBackground` for
   *  contrast when using a background outside the current light/dark family.
   *  Unset keeps the theme's foreground. */
  chromeForeground?: string;
}

/** Rewrite a shell-context relative product path to an absolute URL against
 *  the oxy origin, when one is configured. `/api/*` paths are left relative
 *  — they ride the host's same-origin fetch / dev proxy (which attaches
 *  auth), so absolutizing them would drop the credentials the proxy adds. */
function makeAbs(base: string | undefined) {
  const origin = base?.replace(/\/$/, "");
  return (url?: string | null): string | undefined => {
    if (!url) return undefined;
    if (!origin || !url.startsWith("/") || url.startsWith("/api/")) return url;
    return `${origin}${url}`;
  };
}

function buildRailGroups(
  data: ShellContextData,
  currentAppSlug: string | undefined,
  abs: (url?: string | null) => string | undefined
): RailItem[][] {
  const hq: RailItem = {
    key: "hq",
    label: "HQ",
    testId: "rail-hq",
    icon: <HouseIcon />,
    href: abs(data.links.home)
  };
  const chat: RailItem = {
    key: "chat",
    label: "Chat",
    testId: "rail-chat",
    icon: <MessagesIcon />,
    href: abs(data.links.threads)
  };
  const appItems: RailItem[] = data.apps.map((app) => ({
    key: app.id,
    label: app.name,
    testId: `rail-app-${app.slug}`,
    letter: app.name.slice(0, 1).toUpperCase(),
    imageUrl: abs(app.icon_url),
    // Every app entry is a real anchor into this tab — same as the main
    // web-app's rail — so moving between apps is moving between documents that
    // each own their URL. The current app gets `active` (RailEntry adds
    // aria-current="page"); re-clicking it is a same-page reload.
    active: app.slug === currentAppSlug,
    href: abs(app.url)
  }));
  return appItems.length ? [[hq, chat], appItems] : [[hq, chat]];
}

/**
 * The workspace chrome around a custom app: icon rail + universal top bar
 * + content column — visually identical to the main web-app shell.
 */
export function OxyShell({
  children,
  pageLabel,
  topBarLeft,
  topBarExtra,
  railBottom,
  hideTopBar,
  askHotkey = true,
  productBaseUrl,
  className,
  chromeBackground,
  chromeForeground
}: OxyShellProps) {
  const abs = makeAbs(productBaseUrl);
  // Chrome color overrides. We set the HOST tokens (--sidebar-background,
  // --foreground, --muted-foreground) rather than the derived --oxy-shell-*
  // ones: every sub-component (rail, top bar, AskDock…) is its own
  // `.oxy-shell-scope` and re-derives `--oxy-shell-*` from these host tokens,
  // so overriding a derived token here wouldn't reach them — overriding the
  // host token (which they all read via `var()`) does. Only the tokens the
  // caller provides are set; the theme keeps the rest.
  const chromeStyle = {
    ...(chromeBackground ? { "--sidebar-background": chromeBackground } : {}),
    ...(chromeForeground
      ? {
          "--foreground": chromeForeground,
          "--muted-foreground": `color-mix(in srgb, ${chromeForeground} 65%, transparent)`
        }
      : {})
  } as CSSProperties;
  const { appSlug } = useOxyApp();
  // Safe here: OxyAppProvider only renders children once the manifest is
  // resolved, and OxyShell must sit inside the provider.
  const { manifest } = useResolvedManifest();
  const { data, loading } = useShellContext();
  const { container, setContainer } = useShellPortalContainer();
  // The rail is the dock: its apps open in this tab, warmed on hover.
  useDockPrefetch();

  // Bootstrap failed (old server / no session) → degraded: no chrome. The
  // wrapper tree stays MOUNTED in every state (loading → data/degraded) and
  // only the rail/top-bar siblings toggle, so the app's component tree is
  // never reparented — reparenting would remount the whole app and drop its
  // state (queries, form input) exactly when the shell resolves.
  const degraded = !loading && !data;

  // `resolved.appSlug` is "" (not undefined) when running without server
  // injection — fall back to the manifest's own slug for matching/labels.
  const effectiveSlug = appSlug || manifest.slug;
  const currentApp = data?.apps.find((app) => app.slug === effectiveSlug);
  const workspaceLabel = data ? data.org.name || data.workspace.name : "";
  const page = pageLabel ?? currentApp?.name ?? manifest.name ?? effectiveSlug ?? "";

  // Ask Oxygen: available when this app's manifest binds an agent
  // (`ask.agent`, surfaced by shell-context as `default_agent`). The dock
  // stays mounted once available so its transcript survives close/reopen;
  // ⌘K / Ctrl+K toggles it, same binding as the main web-app.
  const askAgent = currentApp?.default_agent ?? manifest.ask?.agent ?? undefined;
  const askSuggestions = currentApp?.suggested_questions?.length
    ? currentApp.suggested_questions
    : (manifest.ask?.suggestedQuestions ?? []);
  const [askOpen, setAskOpen] = useState(false);
  useEffect(() => {
    if (!askAgent || !askHotkey) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key.toLowerCase() !== "k" || !(e.metaKey || e.ctrlKey)) return;
      e.preventDefault();
      setAskOpen((o) => !o);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [askAgent, askHotkey]);

  // Bottom rail slot: the host app's custom content only. The built-in
  // Settings link was removed — a bundle that wants one can supply it via
  // `railBottom`.
  const bottom = railBottom;

  const groups = data ? buildRailGroups(data, effectiveSlug, abs) : [[]];
  const workspaceTile = data ? (
    <WorkspaceTile name={data.workspace.name} logoUrl={data.logo_url ?? undefined} />
  ) : (
    <span className='oxy-workspace-tile'>
      <OxyMark className='oxy-rail__item-img' />
    </span>
  );
  // Below 768px the rail gives way to NavSheet, whose menu button lives in the
  // top bar — so only when there is a top bar to hold it. `hideTopBar` keeps
  // the rail at every width rather than leave a phone with no navigation.
  const navSheet = !hideTopBar && !degraded;
  const sheet = useNavSheet();

  return (
    <ShellPortalContext.Provider value={container}>
      <div
        ref={setContainer}
        className={cx(
          "oxy-shell-scope oxy-shell",
          degraded && "oxy-shell--degraded",
          navSheet && "oxy-shell--navsheet",
          className
        )}
        style={chromeStyle}
      >
        {!degraded && <ShellRail top={workspaceTile} groups={groups} bottom={bottom} />}
        <div className='oxy-shell__content-col'>
          {/* The top bar renders in degraded mode too when the host passed
              its own slot content — an app that moved its header into the
              shell must never lose it to a failed bootstrap. Only the
              data-driven pieces (breadcrumb, Ask) hide. */}
          {!hideTopBar && (!degraded || topBarLeft || topBarExtra) && (
            <TopBar
              left={
                <>
                  {navSheet && <NavSheetTrigger sheet={sheet} />}
                  {topBarLeft ??
                    (data ? (
                      <Breadcrumb
                        workspaceLabel={workspaceLabel}
                        pageLabel={page}
                        homeHref={abs(data.links.home) ?? data.links.home}
                      />
                    ) : undefined)}
                </>
              }
              right={
                <>
                  <UpdatePill />
                  <SystemIndicator />
                  <WorkspaceClock />
                  {topBarExtra}
                  {askAgent && !degraded && (
                    <button
                      type='button'
                      data-testid='ask-oxygen-button'
                      onClick={() => setAskOpen((o) => !o)}
                      className={cx("oxy-askbtn", askOpen && "oxy-askbtn--open")}
                    >
                      <OxyMark className='oxy-askbtn__mark' />
                      <span className='oxy-askbtn__label'>Ask Oxygen</span>
                      <kbd className='oxy-askbtn__kbd'>⌘K</kbd>
                    </button>
                  )}
                </>
              }
            />
          )}
          {/* A div, not <main>: the app's own content very likely carries
              its own main landmark (the scaffolded template does). */}
          <div className='oxy-shell__main'>{children}</div>
        </div>
        {/* Flex sibling of the content column — opening the dock compacts
            the app (Cursor-style) rather than floating over it, matching
            the web-app AskDock. */}
        {askAgent && !degraded && (
          <AskDock
            agentId={askAgent}
            open={askOpen}
            onClose={() => setAskOpen(false)}
            workspaceName={data?.workspace.name}
            suggestions={askSuggestions}
            threadsHref={abs(data?.links.threads)}
          />
        )}
        {navSheet && (
          <NavSheetPanel
            sheet={sheet}
            title={workspaceLabel}
            top={workspaceTile}
            groups={groups}
            bottom={bottom}
          />
        )}
      </div>
    </ShellPortalContext.Provider>
  );
}
