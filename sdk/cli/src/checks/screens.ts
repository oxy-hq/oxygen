/**
 * Which screens of a build the promote checks open, and what each one's files
 * are called.
 *
 * A build's screens are the ones its manifest declares (`checks.screens`,
 * which the claim hands over), or, when it declares none, the ones the ready
 * root page links to: every same-app `<a href>` target, read once and never
 * clicked. A screen is an app-relative path; `""` is the root. A DECLARED
 * screen may carry a query (`?module=waste`), for an app that picks its
 * screen by one; a link's query is dropped, since a link's is as often a
 * filter or a tracking tag as a screen.
 *
 * NOTHING HERE OPENS A PAGE. These are the rules, as plain functions over
 * strings: what counts as a screen of this app, in what order, how many, and
 * the file name each picture and text is kept under.
 */

import type { LoadsTarget } from "./loads-target.js";

/** Where a screen list holds the app: its base URL, origin and base path. */
type AppBase = Pick<LoadsTarget, "url" | "origin" | "basePath">;

export interface ScreenList {
  /** App-relative paths, in the order they are opened. */
  paths: string[];
  /** `true` when they were read from the root page's links; `false` when the build declared them. */
  discovered: boolean;
  /** How many more there were than the cap allows. */
  omitted: number;
}

/**
 * Paths under the app that are no screen and are never opened as one: a
 * function route (the checker calls no function) and the runtime's own
 * endpoints.
 */
const NEVER_A_SCREEN = /^(?:fn|__oxy)(?:\/|$)/;
/** A link to a file, not to a screen: the bundle's assets, and anything with a file extension. */
const A_FILE = /^assets(?:\/|$)|\.(?!html?$)[a-z0-9]{1,8}$/i;

/**
 * The app-relative path `href` names, with its query and hash dropped and no
 * trailing slash; `undefined` when it is not a screen of this app (another
 * origin, outside the base, a function route).
 */
export function screenPath(href: string, app: AppBase): string | undefined {
  let url: URL;
  try {
    url = new URL(href, app.url);
  } catch {
    return undefined;
  }
  if (url.origin !== app.origin) return undefined;
  // The base without its trailing slash is the root too.
  const pathname = `${url.pathname}/` === app.basePath ? app.basePath : url.pathname;
  if (!pathname.startsWith(app.basePath)) return undefined;
  const path = pathname.slice(app.basePath.length).replace(/\/+$/, "");
  return NEVER_A_SCREEN.test(path) ? undefined : path;
}

/**
 * The screen a declared entry names: its path by {@link screenPath}'s rule,
 * with the entry's query kept as it was written. `undefined` when the path is
 * no screen of this app.
 */
function declaredScreen(entry: string, app: AppBase): string | undefined {
  // App-relative: a leading slash would name the origin's root, not the app's.
  const relative = entry.replace(/^\/+/, "");
  const at = relative.indexOf("?");
  const path = screenPath(at < 0 ? relative : relative.slice(0, at), app);
  if (path === undefined) return undefined;
  const query = at < 0 ? "" : (relative.slice(at).split("#")[0] ?? "");
  // A bare `?` asks for nothing: the path alone.
  return query.length > 1 ? `${path}${query}` : path;
}

/** A screen's path without the query a declared one may carry. */
function pathOf(screen: string): string {
  const at = screen.indexOf("?");
  return at < 0 ? screen : screen.slice(0, at);
}

/** The first `cap` of `paths`, each once, and how many were left out. */
function capped(paths: readonly string[], cap: number): Pick<ScreenList, "paths" | "omitted"> {
  const unique = Array.from(new Set(paths));
  return { paths: unique.slice(0, cap), omitted: Math.max(0, unique.length - cap) };
}

/**
 * The screens a build declares, in its order. `undefined` when it declares
 * none (the claim's `screens` is null or no list): they are discovered then.
 * A declared entry that is no path of this app is dropped; a list with
 * nothing left is the root alone.
 */
export function declaredScreens(
  declared: unknown,
  app: AppBase,
  cap: number
): ScreenList | undefined {
  if (!Array.isArray(declared)) return undefined;
  const paths = declared
    .filter((entry): entry is string => typeof entry === "string")
    .map((entry) => declaredScreen(entry, app))
    .filter((path): path is string => path !== undefined);
  const list = capped(paths, cap);
  return {
    paths: list.paths.length > 0 ? list.paths : [""],
    discovered: false,
    omitted: list.omitted
  };
}

/**
 * The screens the root page links to: the root first, then every same-app
 * link target in document order, each once, at most `cap`.
 */
export function discoveredScreens(hrefs: readonly string[], app: AppBase, cap: number): ScreenList {
  const paths = [""];
  for (const href of hrefs) {
    const path = screenPath(href, app);
    if (path !== undefined && !A_FILE.test(path)) paths.push(path);
  }
  return { ...capped(paths, cap), discovered: true };
}

/** The most unlisted screens a check names; the rest are only counted. */
export const MAX_UNLISTED = 50;
/** The longest path a build may declare (`checks.screens`): a longer one cannot be listed. */
const MAX_DECLARED_CHARS = 256;

/** Screens of the app that the opened screens link to and the list does not hold. */
export interface Unlisted {
  /** The first of them, in the order their links were read, at most {@link MAX_UNLISTED}. */
  paths: string[];
  /** How many there are in all. */
  total: number;
}

/**
 * The screens `hrefs` name that `listed` does not hold, each once: what the
 * checks did not open, and a person may want to declare. A link counts by the
 * rule a discovered list goes by ({@link discoveredScreens}), so by its path
 * alone: a link to `?module=temps` is the root's, and is listed wherever the
 * root, with or without a query of its own, is. A path too long to declare is
 * left out, since naming it could not get it listed.
 */
export function unlistedScreens(
  hrefs: readonly string[],
  app: AppBase,
  listed: readonly string[],
  cap: number = MAX_UNLISTED
): Unlisted {
  const known = new Set(listed.map(pathOf));
  const found = new Set<string>();
  for (const href of hrefs) {
    const path = screenPath(href, app);
    if (path === undefined || A_FILE.test(path) || known.has(path)) continue;
    if (path.length <= MAX_DECLARED_CHARS) found.add(path);
  }
  return { paths: Array.from(found).slice(0, cap), total: found.size };
}

/** The URL a screen is opened at. */
export function screenUrl(app: Pick<LoadsTarget, "url">, path: string): string {
  return `${app.url}${path}`;
}

/** A screen as a person reads it: `/` for the root, `/stores` for `stores`, `/?module=waste`. */
export function screenLabel(path: string): string {
  return `/${path}`;
}

/** The longest slug: a file name is at most 120 characters, and the longest prefix and suffix take 27. */
const MAX_SLUG_CHARS = 80;

/**
 * Each path's slug, for its files' names: the path with `/` as `--`, the root
 * as `root`. A file name is `[a-z0-9._-]`, so the slug is lower-cased and any
 * other character becomes `-` (`?module=waste` is `module-waste`); two paths
 * that would share a slug are told apart by a number (`stores`, `stores-2`).
 */
export function screenSlugs(paths: readonly string[]): Map<string, string> {
  const slugs = new Map<string, string>();
  const taken = new Set<string>();
  for (const path of paths) {
    if (slugs.has(path)) continue;
    const plain =
      path === ""
        ? "root"
        : path
            .split("/")
            .join("--")
            .toLowerCase()
            .replace(/[^a-z0-9._-]/g, "-")
            // A query-only screen starts with `?`: its slug starts with its first name.
            .replace(/^-+/, "")
            .slice(0, MAX_SLUG_CHARS) || "screen";
    let slug = plain;
    for (let n = 2; taken.has(slug); n += 1) slug = `${plain}-${n}`;
    taken.add(slug);
    slugs.set(path, slug);
  }
  return slugs;
}

/** `shot-staging-stores-desk.png`: one screen's picture in one environment. */
export function shotName(environment: string, slug: string, viewport: string): string {
  return `shot-${environment}-${slug}-${viewport}.png`;
}

/** `diff-stores-desk.png`: where staging's picture and production's differ. */
export function diffName(slug: string, viewport: string): string {
  return `diff-${slug}-${viewport}.png`;
}

/** `text-staging-stores-desk.json`: one screen's visible text in one environment. */
export function textName(environment: string, slug: string, viewport: string): string {
  return `text-${environment}-${slug}-${viewport}.json`;
}

/** The app's own colour and font-size declarations, once per attempt. */
export const CSS_DIGEST_NAME = "css-digest.json";
