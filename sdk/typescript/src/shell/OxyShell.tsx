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

import type { CSSProperties, ReactNode } from "react";
import { useOxyApp, useResolvedManifest } from "../custom-app/react";
import { cx } from "./cx";
import { useDockPrefetch } from "./dockPrefetch";
import { ModeSwitch } from "./ModeSwitch";
import { OxyMark } from "./marks";
import { useOxyModePin, useShellMode } from "./mode";
import { NavSheetPanel, NavSheetTrigger, useNavSheet } from "./NavSheet";
import { PhoneTapHintFrame } from "./phone/PhoneTapHintFrame";
import { ShellPortalContext, useShellPortalContainer } from "./portal";
import { type RailItem, ShellRail } from "./ShellRail";
import { type ShellContextData, useShellContext } from "./shellContext";
import { TodayClock } from "./TodayClock";
import { Breadcrumb, SystemIndicator, TopBar, WorkspaceClock } from "./TopBar";
import { useOxyTouchTargets } from "./touch";
import { UpdatePill } from "./UpdatePill";
import { WorkspaceTile } from "./WorkspaceTile";

/** Home's glyph: Phosphor's House at the regular weight, the one Oxygen's rail
 *  draws (the SDK carries no icon library, so its path is inlined). */
function HouseIcon() {
  return (
    <svg width='16' height='16' viewBox='0 0 256 256' fill='currentColor' aria-hidden='true'>
      <path d='M219.31,108.68l-80-80a16,16,0,0,0-22.62,0l-80,80A15.87,15.87,0,0,0,32,120v96a8,8,0,0,0,8,8h64a8,8,0,0,0,8-8V160h32v56a8,8,0,0,0,8,8h64a8,8,0,0,0,8-8V120A15.87,15.87,0,0,0,219.31,108.68ZM208,208H160V152a8,8,0,0,0-8-8H104a8,8,0,0,0-8,8v56H48V120l80-80,80,80Z' />
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
   *  (Sys · clock). */
  topBarExtra?: ReactNode;
  /** Rail bottom slot content. The shell adds no built-in entries here —
   *  a bundle that wants a Settings link supplies its own. */
  railBottom?: ReactNode;
  /** Drop the top bar and keep only the rail. */
  hideTopBar?: boolean;
  /** Keep the top bar's clock a plain time, without the Today button and
   *  panel (and so without its `GET /api/home/widgets` read). For a screen
   *  that is not one person's, such as a store's shared tablet, where the
   *  viewer's widgets have no place. Default false: the clock opens Today. */
  plainClock?: boolean;
  /** No effect since 2.34.0, kept so an app that passes it still builds: the
   *  shell has no Ask entry any more (no top-bar button, no ⌘K / Ctrl+K, no
   *  dock), as Oxygen's own chrome has none. The shortcut is the app's to use. */
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
  /** Show only this mode while the shell is mounted, whatever the person
   *  picked, and hide the Light / Dark / System switch: for a screen that is
   *  never one person's, such as a store's shared tablet. Unset (the default)
   *  follows the person's mode, which Oxygen shares. A whole app can pin in
   *  its `index.html` instead (`<html data-oxy-mode-pin="light">`), which the
   *  pre-paint sees too; a screen without the shell uses `useOxyModePin`.
   *  Like the mode itself, acts only with `@oxy-hq/sdk/theme.css` imported. */
  modePin?: "light" | "dark";
  /** On a touch screen, with `@oxy-hq/sdk/theme.css` imported, the shell's
   *  rail entries, Today clock, Ask button and update pill each answer a
   *  tap within a 44px area (nothing drawn grows). `false` keeps them at
   *  their drawn size, for a screen that is not a person's desk, such as a
   *  store's enrolled tablet. A component around the shell uses
   *  `useOxyTouchTargets(false)`; a whole app,
   *  `<html data-oxy-touch-targets="off">`. Default true. */
  touchTargets?: boolean;
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
  // Home, then the apps; no Chat entry, as in Oxygen's own rail.
  const hq: RailItem = {
    key: "hq",
    label: "Home",
    testId: "rail-hq",
    icon: <HouseIcon />,
    href: abs(data.links.home)
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
  // A to Z, as Oxygen's rail lists them.
  appItems.sort((a, b) => a.label.localeCompare(b.label, undefined, { sensitivity: "base" }));
  return appItems.length ? [[hq], appItems] : [[hq]];
}

/** What the rail and the nav sheet share: the entries, the workspace mark (the Oxygen mark while
 *  the bootstrap is in flight) and the org's name, which heads the open rail, the sheet and the
 *  breadcrumb. */
function railParts(
  data: ShellContextData | null,
  currentAppSlug: string | undefined,
  abs: (url?: string | null) => string | undefined
): { groups: RailItem[][]; workspaceTile: ReactNode; workspaceLabel: string } {
  if (!data) {
    return {
      groups: [[]],
      workspaceTile: (
        <span className='oxy-workspace-tile'>
          <OxyMark className='oxy-rail__item-img' />
        </span>
      ),
      workspaceLabel: ""
    };
  }
  return {
    groups: buildRailGroups(data, currentAppSlug, abs),
    workspaceTile: (
      <WorkspaceTile name={data.workspace.name} logoUrl={data.logo_url ?? undefined} />
    ),
    workspaceLabel: data.org.name || data.workspace.name
  };
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
  plainClock = false,
  productBaseUrl,
  className,
  chromeBackground,
  chromeForeground,
  modePin,
  touchTargets
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
  // The person's light/dark mode (theme.css only): applied, followed, and
  // switchable from the rail unless this screen pins one.
  useOxyModePin(modePin);
  useOxyTouchTargets(touchTargets);
  const mode = useShellMode(data);
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
  const { groups, workspaceTile, workspaceLabel } = railParts(data, effectiveSlug, abs);
  const page = pageLabel ?? currentApp?.name ?? manifest.name ?? effectiveSlug ?? "";

  // Bottom rail slot: the host app's custom content, then the mode switch,
  // where Oxygen keeps its user menu. The built-in Settings link was removed —
  // a bundle that wants one can supply it via `railBottom`. Without the switch
  // (no theme, a pin, an older server, a crew sign-in) the slot is exactly the
  // app's, so a rail with no `railBottom` still has no bottom section.
  const modeSwitch = (variant: "rail" | "sheet") =>
    mode.switchable ? (
      <>
        {railBottom}
        <ModeSwitch mode={mode.choice} onChoose={mode.choose} variant={variant} />
      </>
    ) : (
      railBottom
    );

  // Below 768px, and on a phone on its side (NAV_SHEET_QUERY), the rail gives
  // way to NavSheet, whose menu button lives in the top bar — so only when
  // there is a top bar to hold it. `hideTopBar` keeps the rail at every width
  // rather than leave a phone with no navigation.
  const navSheet = !hideTopBar && !degraded;
  const sheet = useNavSheet();

  return (
    <ShellPortalContext.Provider value={container}>
      {/* On a phone layout, a hint with no tap form from the app (a `Stat`'s tip) gets the kit's
          tap ⓘ, which a finger can open; the desk is unchanged (2.31.1, PhoneTapHintFrame). */}
      <PhoneTapHintFrame>
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
          {!degraded && (
            <ShellRail
              top={workspaceTile}
              title={workspaceLabel}
              groups={groups}
              groupTitles={[undefined, "Apps"]}
              // Open, the mode switch is the sheet's "Appearance" row, as Oxygen's rail
              // draws its account menu as a row with its name; closed, the one icon.
              bottom={(open: boolean) => modeSwitch(open ? "sheet" : "rail")}
            />
          )}
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
                    {data?.org?.slug && !plainClock ? (
                      // The clock opens Today: this person's widgets from every app.
                      <TodayClock
                        orgSlug={data.org.slug}
                        homeHref={data.links.home}
                        resolve={abs}
                      />
                    ) : (
                      <WorkspaceClock />
                    )}
                    {topBarExtra}
                  </>
                }
              />
            )}
            {/* A div, not <main>: the app's own content very likely carries
              its own main landmark (the scaffolded template does). */}
            <div className='oxy-shell__main'>{children}</div>
          </div>
          {navSheet && (
            <NavSheetPanel
              sheet={sheet}
              title={workspaceLabel}
              top={workspaceTile}
              groups={groups}
              bottom={modeSwitch("sheet")}
            />
          )}
        </div>
      </PhoneTapHintFrame>
    </ShellPortalContext.Provider>
  );
}
