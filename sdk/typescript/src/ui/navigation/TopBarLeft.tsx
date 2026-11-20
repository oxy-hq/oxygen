// Started from oxy-hq/reference-app@3caad1f `src/chrome/TopBarLeft.tsx` (BEYOND-GALLERY row 10:
// it states the shell's three states; 10 copies in 3 repos re-derive the same values). Kept: the
// shell's own exported `Breadcrumb`, fed the three inputs OxyShell feeds it, in its order (the
// org's name, else the workspace's; this app's registered name, else the manifest's `name`, else
// its slug; the HQ link), the three states, a fragment so the strip is a direct child of the
// top bar. Changed: the dev stand-in shows the app's own name, and the workspace name the
// reference's stand-in printed (its fixture's company) only when the caller passes one
// (`standIn`), drawn as the reference drew it; the strip is `ParentTabs` in the top bar,
// router-free; `productBaseUrl` resolves the HQ link as OxyShell's own prop does.
//
// The top bar's left side: the breadcrumb, then the open section's tab strip. OxyShell's
// `topBarLeft` REPLACES the shell's breadcrumb, and the strip must sit right after it (where a
// reader's eye goes next, and where the keyboard reaches it before Ask Oxygen), so this draws
// the breadcrumb itself:
//
//   <OxyShell topBarLeft={<TopBarLeft tabs={sectionTabs} />}>
//
// Three states, the shell's own:
//   - shell-context loaded: the shell's breadcrumb, then the strip;
//   - still loading: the strip alone, as the shell itself draws nothing there yet;
//   - unavailable (a local `pnpm dev`, where oxy does not serve the bootstrap): a plain-text
//     stand-in of the same shape, then the strip.
//
// `useShellContext` is read here as OxyShell reads it, so the page asks for shell-context twice;
// an OxyShell slot after its own breadcrumb would save the second read (the report says so).

import { useOxyApp, useResolvedManifest } from "../../custom-app/react";
import { useShellContext } from "../../shell/shellContext";
import { Breadcrumb } from "../../shell/TopBar";
import { cx } from "../cx";
import { ParentTabs, type ParentTabsProps } from "./ParentTabs";

/** The names a local stand-in prints where no shell serves shell-context. */
export interface BreadcrumbStandIn {
  /** The workspace the shell's breadcrumb would name ("Northwind Kitchens"). */
  workspaceLabel: string;
  /** The page; defaults to the page the breadcrumb resolves (`pageLabel`, else the app's name). */
  pageLabel?: string;
}

export interface AppBreadcrumbProps {
  /** The page label; defaults to this app's registered name, as OxyShell's `pageLabel`. */
  pageLabel?: string;
  /** The Oxygen product's origin, as OxyShell's `productBaseUrl`: the HQ link resolves on it. */
  productBaseUrl?: string;
  /** What the stand-in names when shell-context is unavailable (a local preview, where oxy does
   *  not serve it): "workspace / page", as the reference's stand-in printed its company and
   *  name. Never shown inside the shell, whose own names win. Without it, the page alone. */
  standIn?: BreadcrumbStandIn;
}

/** The value OxyShell's `makeAbs` gives a product link. */
function absolute(base: string | undefined, url: string): string {
  const origin = base?.replace(/\/$/, "");
  if (!origin || !url.startsWith("/") || url.startsWith("/api/")) return url;
  return `${origin}${url}`;
}

/** The shell's breadcrumb, with the values the shell would give it. */
export function AppBreadcrumb({ pageLabel, productBaseUrl, standIn }: AppBreadcrumbProps) {
  const { data, loading } = useShellContext();
  const { appSlug } = useOxyApp();
  const { manifest } = useResolvedManifest();
  const slug = appSlug || manifest.slug;
  const page =
    pageLabel ?? data?.apps.find((a) => a.slug === slug)?.name ?? manifest.name ?? slug ?? "";
  if (!data) {
    if (loading) return null;
    // A stand-in of the same shape, in plain text: there is no HQ to link to.
    if (standIn)
      return (
        <span
          className='oxy-ui-top-bar-left__fallback oxy-ui-top-bar-left__fallback--crumbs'
          data-testid='breadcrumb-fallback'
        >
          <span className='oxy-ui-top-bar-left__fallback-workspace'>{standIn.workspaceLabel}</span>
          <span>/</span>
          <span>{standIn.pageLabel ?? page}</span>
        </span>
      );
    return (
      <span className='oxy-ui-top-bar-left__fallback' data-testid='breadcrumb-fallback'>
        {page}
      </span>
    );
  }
  return (
    <Breadcrumb
      workspaceLabel={data.org.name || data.workspace.name}
      pageLabel={page}
      homeHref={absolute(productBaseUrl, data.links.home)}
    />
  );
}

export interface TopBarLeftProps extends AppBreadcrumbProps {
  /** The open section's strip, or null for a section of one screen (a strip of one is a
   *  control that changes nothing). */
  tabs: Omit<ParentTabsProps<string>, "placement"> | null;
  className?: string;
}

/**
 * A fragment, not a wrapper: the strip has to be a direct child of the top bar so it stretches
 * to the bar's height and sits on its bottom rule.
 */
export function TopBarLeft({
  tabs,
  pageLabel,
  productBaseUrl,
  standIn,
  className
}: TopBarLeftProps) {
  return (
    <>
      {/* From 1024px the breadcrumb's box is held to the Sections column's width, so the strip
          starts where the page does and the selected tab opens onto the page. */}
      <div className={cx("oxy-ui-top-bar-left", className)}>
        <AppBreadcrumb pageLabel={pageLabel} productBaseUrl={productBaseUrl} standIn={standIn} />
      </div>
      {tabs && <ParentTabs {...tabs} placement='topbar' />}
    </>
  );
}
