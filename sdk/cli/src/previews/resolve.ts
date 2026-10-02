/**
 * The one path helper every `oxyc preview` verb shares: `/api/{workspace_id}/previews...`
 * (`crates/app/src/server/api/workspace_previews.rs`), staff-only
 * (`WorkspacePreviewer`) and mounted BESIDE the workspace tree rather than
 * inside it.
 *
 * `{workspace}` resolves exactly the way `oxyc api {workspace}/...` resolves
 * it — `--workspace <id>`, through `ctx.placeholders()` and
 * `substitutePlaceholders` — there is no second workspace resolver here, and
 * an unresolved `{workspace}` fails with the same usage error `oxyc api`
 * gives (naming `--workspace`).
 */

import { normalizePath, substitutePlaceholders } from "../api/paths.js";
import type { Context } from "../context/resolve.js";

/** `{workspace}/previews<suffix>`, normalised and placeholder-substituted. */
export function previewsPath(ctx: Context, suffix = ""): string {
  return substitutePlaceholders(normalizePath(`{workspace}/previews${suffix}`), ctx.placeholders());
}
