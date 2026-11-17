/**
 * Where a route function is served: the app's canonical URL
 * (`<base>/customer-apps/<org>/<app>/`) plus `fn/<name>`, the path the runtime
 * routes (`FN_ROUTE` in `custom_apps_functions/agent_gate.rs`). `null` when the
 * app URL does not parse — a missing row beats a wrong URL someone copies.
 */
export function functionUrl(appUrl: string, name: string): string | null {
  try {
    const base = appUrl.endsWith("/") ? appUrl : `${appUrl}/`;
    return new URL(`fn/${encodeURIComponent(name)}`, base).toString();
  } catch {
    return null;
  }
}
