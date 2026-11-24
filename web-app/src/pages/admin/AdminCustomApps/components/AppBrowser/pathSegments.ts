import type { CustomApp } from "@/types/apps";
import type { BrowserScope } from "../../appBrowserModel";
import { CONSOLE_PAGES, type ConsolePageId, consolePagePath } from "../AppConsole/consolePages";
import type { PathSegment } from "./PathBar";

/**
 * The path bar's trail. The last segment is where you are, so it carries no link.
 *
 * It names the app by its slug rather than its display name: the slug is what the URL,
 * an `oxyc` command and the Apps tier's org suffix all use, and the pane below already
 * shows the name in full. Pure, so the one rule that matters — that you can always click
 * your way back up a tier — is testable without a router.
 */
export function pathSegments(
  scope: BrowserScope,
  app: CustomApp | null,
  page: ConsolePageId | null
): PathSegment[] {
  const trail: PathSegment[] = [{ label: "All apps", to: "/admin/apps" }];
  if (scope.kind === "org") {
    trail.push({
      label: scope.orgSlug,
      to: `/admin/apps?org=${encodeURIComponent(scope.orgSlug)}`
    });
  }
  if (app) {
    trail.push({ label: app.slug, to: consolePagePath(app, "overview") });
    const named = CONSOLE_PAGES.find((p) => p.id === page);
    if (named) trail.push({ label: named.label });
  }
  return trail.map((segment, i) => (i === trail.length - 1 ? { label: segment.label } : segment));
}
