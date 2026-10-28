/**
 * A plain name for something the workspace defines in a file: an automation, an
 * agent, a data app. `automations/daily_sales.automation.yml` → "Daily sales".
 *
 * For customer screens with the apps-first experience on, which never show a file
 * path, a folder or an extension. The Factory keeps showing paths.
 */
export function objectNameFromPath(path: string): string {
  const base = path.split(/[/\\]/).filter(Boolean).pop() ?? path;
  const stem = base
    .replace(
      /\.(procedure|workflow|automation|agentic|agent|aw|app|view|topic|airway|test)\.(yml|yaml)$/,
      ""
    )
    .replace(/\.(yml|yaml|sql)$/, "");
  const words = stem
    .split(/[_\-\s.]+/)
    .filter(Boolean)
    .join(" ");
  if (!words) return stem;
  return words.charAt(0).toUpperCase() + words.slice(1);
}
