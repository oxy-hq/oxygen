// Started from oxy-hq/reference-app@3caad1f `src/router.tsx` (BEYOND-GALLERY row 7), made generic
// over the app's route type: the route table (`parse`, `path`, `title`) stays in the app, as the
// row recommends. Kept: push rather than replace, so Back returns to the previous screen instead
// of walking out of the app; one store read through `useSyncExternalStore` that re-renders on
// push, replace, Back and Forward; real hrefs, so ⌘-click and "open in new tab" work; a `Link`
// that navigates in-app only on a plain left click, carries the one focus ring, and replaces
// rather than stacks when it is the screen already shown; the scroll to the top. New: the base
// path is the platform's (`/customer-apps/<org>/<app>` on the main host, `/a/<app>` on an org
// subdomain, the root on the app's own subdomain, from `window.__OXY_APP__`), not only the
// first; it tells `urlState` about its own navigations (`noteUrl`), as Watchlist's router does.
// Warehouse's unsaved-work hold and `leaveFor` are that app's and stay there.
//
// It binds no keys: ⌘K / Ctrl+K belongs to Ask Oxygen (the shell's), and Back/Forward are the
// browser's.
//
//   const router = createRouter({ parse: parseRoute, path: routePath, title: titleFor });
//   export const { navigate, useRoute, hrefFor, Link } = router;

import {
  type AnchorHTMLAttributes,
  type MouseEvent,
  type ReactNode,
  useSyncExternalStore
} from "react";
import { readInjectedAppConfig } from "../../custom-app/inject";
import { appBasePath } from "../../custom-app/notifications";
import { cx } from "../cx";
import { ui } from "../recipes";
import { noteUrl } from "./urlState";

/** The app's own routes: how a path reads as a route and back. */
export interface RouteTable<R> {
  /** An app-relative path (leading slash, no base, no query) as a route. Anything it does not
   *  recognise should land somewhere real (the app's first screen), never throw. */
  parse(path: string): R;
  /** A route's app-relative path, leading slash. */
  path(route: R): string;
  /** The document title for a route; omit to leave the title alone. */
  title?(route: R): string;
}

export interface NavigateOptions {
  /** Replace the current entry instead of pushing one (a redirect, a canonical spelling). */
  replace?: boolean;
  /** The query string to open the destination with, without the leading `?`: state that
   *  belongs to the screen being opened. A screen's own filters go through `writeUrlParams`. */
  search?: string;
  /** Scroll the window to the top (default true): a new screen starts at its top. */
  scroll?: boolean;
}

export type RouterLinkProps<R> = Omit<AnchorHTMLAttributes<HTMLAnchorElement>, "href"> & {
  to: R;
  replace?: boolean;
  search?: string;
  children?: ReactNode;
};

export interface Router<R> {
  navigate(route: R, opts?: NavigateOptions): void;
  /** The current route. Re-renders on push, replace, Back and Forward, including one that changes
   *  only the query. */
  useRoute(): R;
  /** The absolute href for a route, under the app's base path. */
  hrefFor(route: R, search?: string): string;
  /** An anchor that navigates in-app on a plain left click and is a normal link otherwise. */
  Link(props: RouterLinkProps<R>): ReactNode;
}

/** The main host's prefix, the one an un-injected page (a local build) can still be under. */
const CUSTOMER_APPS = /^\/customer-apps\/[^/]+\/[^/]+(?=\/|$)/;

/**
 * The path prefix this bundle is served under, without a trailing slash: `/customer-apps/<org>/
 * <app>`, `/a/<app>`, or `""` on the app's own subdomain and in `pnpm dev` at the root.
 */
export function appBase(pathname: string = window.location.pathname): string {
  const injected = readInjectedAppConfig();
  if (injected?.slug) {
    const base = appBasePath(pathname, injected.orgSlug, injected.slug);
    return base === "/" ? "" : base.replace(/\/+$/, "");
  }
  return CUSTOMER_APPS.exec(pathname)?.[0] ?? "";
}

/** The app-relative path: the pathname with the base removed, always leading-slashed. */
export function appPath(pathname: string = window.location.pathname): string {
  const base = appBase(pathname);
  const rest = base && pathname.startsWith(base) ? pathname.slice(base.length) : pathname;
  return rest.startsWith("/") ? rest : `/${rest}`;
}

const listeners = new Set<() => void>();

function emit() {
  for (const l of listeners) l();
}

function subscribe(onChange: () => void) {
  listeners.add(onChange);
  window.addEventListener("popstate", onChange);
  return () => {
    listeners.delete(onChange);
    window.removeEventListener("popstate", onChange);
  };
}

const currentPath = () => appPath();

/** What the store follows: the app-relative path and the query, so `navigate` or a `Link` that
 *  changes only the query (`reports?range=week` to `?range=month`) still re-renders the screen
 *  that reads it. Not the hash, which no route reads, so a change of the hash alone re-renders
 *  nothing, as before. */
const currentLocation = () => `${appPath()}${window.location.search}`;
const serverLocation = () => "/";

/** A location's path, the part the route table reads. A pathname holds no `?` (it is escaped as
 *  `%3F`), so the first one starts the query. */
const pathOf = (location: string) => location.split("?", 1)[0];

/** The href for an app-relative path under the current base. */
function hrefOf(path: string, search?: string): string {
  const base = appBase();
  const href = path === "/" ? `${base}/` : `${base}${path}`;
  return search ? `${href}?${search.replace(/^\?/, "")}` : href;
}

function isPlainClick(e: MouseEvent): boolean {
  return !(e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey);
}

export function createRouter<R>(table: RouteTable<R>): Router<R> {
  const hrefFor = (route: R, search?: string) => hrefOf(table.path(route), search);

  function navigate(route: R, opts: NavigateOptions = {}): void {
    const url = hrefFor(route, opts.search);
    if (opts.replace) window.history.replaceState(null, "", url);
    else window.history.pushState(null, "", url);
    noteUrl();
    emit();
    if (opts.scroll !== false) window.scrollTo(0, 0);
  }

  // biome-ignore lint/correctness/noComponentHookFactories: called once, at module level, by the app's routes file; the hook keeps one identity for the page's life.
  function useRoute(): R {
    const here = useSyncExternalStore(subscribe, currentLocation, serverLocation);
    const route = table.parse(pathOf(here));
    if (table.title && typeof document !== "undefined") {
      const title = table.title(route);
      if (document.title !== title) document.title = title;
    }
    return route;
  }

  // biome-ignore lint/correctness/noComponentHookFactories: as useRoute: one Link per router, made once.
  function Link({ to, replace, search, onClick, className, ...rest }: RouterLinkProps<R>) {
    const handle = (e: MouseEvent<HTMLAnchorElement>) => {
      onClick?.(e);
      if (e.defaultPrevented || !isPlainClick(e)) return;
      e.preventDefault();
      // Re-clicking the screen you are on does not stack an entry Back has to walk through.
      navigate(to, { replace: replace ?? table.path(to) === currentPath(), search });
    };
    // The one focus ring, here rather than at each call site, where it is a thing to forget.
    return (
      <a
        {...rest}
        href={hrefFor(to, search)}
        onClick={handle}
        className={cx(ui.focusRing, className)}
      />
    );
  }

  return { navigate, useRoute, hrefFor, Link };
}
