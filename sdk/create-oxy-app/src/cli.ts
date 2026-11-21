/**
 * `create-oxy-app` — scaffold a new Oxy custom app.
 *
 * Usage:
 *   pnpm dlx create-oxy-app my-app
 *   pnpm dlx create-oxy-app my-app --template single-store
 *
 * The CLI copies a template directory from `templates/<id>/` next to
 * this script (bundled with the package) into a new directory at
 * `<cwd>/<name>/`, with `{{name}}` and `{{slug}}` substituted in
 * `package.json` and `oxy-app.json`.
 *
 * Templates ship with the kit deps pre-pinned, the vite-plugin
 * pre-wired in `vite.config.ts`, and a working `useQuery` example.
 * The point: the operator types one command and `pnpm install &&
 * pnpm dev` works.
 */

import { promises as fs } from "node:fs";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
// Templates live next to the bundled CLI. In dev (`pnpm dev` against
// the source) `dist/` is one level under the package root; in
// publish (`pnpm dlx`) `dist/` and `templates/` are siblings under
// the published package. Both shapes resolve to the same parent.
const TEMPLATES_DIR = path.resolve(HERE, "..", "templates");

interface ParsedArgs {
  name: string | null;
  template: string;
  /**
   * The org the app belongs to, written into `oxy-app.json` as `orgSlug` so
   * `pnpm dev` and `oxyc publish` need no `--org` / `OXY_ORG` afterwards.
   */
  org: string | null;
  help: boolean;
}

function parseArgs(argv: string[]): ParsedArgs {
  const out: ParsedArgs = { name: null, template: "vite", org: null, help: false };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--help" || arg === "-h") {
      out.help = true;
    } else if (arg === "--template" || arg === "-t") {
      const next = argv[i + 1];
      if (!next) throw new Error("--template requires a value");
      out.template = next;
      i++;
    } else if (arg.startsWith("--template=")) {
      out.template = arg.slice("--template=".length);
    } else if (arg === "--org") {
      const next = argv[i + 1];
      if (!next) throw new Error("--org requires a value");
      out.org = next;
      i++;
    } else if (arg.startsWith("--org=")) {
      out.org = arg.slice("--org=".length);
    } else if (!arg.startsWith("-") && !out.name) {
      out.name = arg;
    }
  }
  return out;
}

/**
 * The server's `is_valid_slug`, as the SDK's `isValidSlug` states it: 1–63
 * lowercase letters, digits and single hyphens. Applied to the app slug
 * `slugify` derives and to `--org`, which is the org's slug, not its name.
 */
const SLUG_PATTERN = /^[a-z0-9]+(-[a-z0-9]+)*$/;
const SLUG_MAX_LEN = 63;
const isValidSlug = (slug: string): boolean => slug.length <= SLUG_MAX_LEN && SLUG_PATTERN.test(slug);

function printHelp(): void {
  console.log(`
create-oxy-app — scaffold a new Oxy custom app.

Usage:
  create-oxy-app <name> [--template <id>] [--org <slug>]

Options:
  -t, --template <id>   Template to use (default: vite).
                        Available:
                          vite          a standard custom app (frontend only)
                          functions     …plus a server-side Oxy Function + email
                          single-store  parameter selector + summary + activity
                          dashboard     KPI strip + chart + leaderboard
      --org <slug>      The org the app belongs to. Written to oxy-app.json as
                        "orgSlug", so \`pnpm dev\` and \`oxyc publish\` find the
                        app without --org or OXY_ORG.
  -h, --help            Show this help.

Examples:
  pnpm dlx create-oxy-app store-pulse --org acme
  pnpm dlx create-oxy-app sales-dashboard --template dashboard
  pnpm dlx create-oxy-app notifier --template functions --org acme
`);
}

/**
 * Slugify a name into a valid oxy-app slug: lowercase, alphanumeric
 * + dashes, starts with a letter or digit. Matches the regex the
 * vite-plugin enforces — see validateManifest there.
 */
function slugify(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .replace(/-{2,}/g, "-");
}

/** Filename a template entry is written under in a scaffolded app.
 *
 * Both `.example` spellings are stripped, matching the server-side scaffold's
 * filter, which skips `.yml.example` AND
 * `.yaml.example`. The two must agree: anything the server filters out, the CLI
 * has to rename back, or a standalone scaffold silently loses the file.
 */
export function templateDestName(entryName: string): string {
  if (entryName === "_gitignore") return ".gitignore";
  if (entryName.endsWith(".yml.example") || entryName.endsWith(".yaml.example")) {
    return entryName.slice(0, -".example".length);
  }
  return entryName;
}

/**
 * Write `orgSlug` into the scaffolded `oxy-app.json`, keeping `$schema` and
 * `schemaVersion` first. A key the templates do not carry, because the admin
 * scaffold renders the same files into an org's own repo and the CLI is the
 * one place the org is a flag.
 */
export async function writeOrgSlug(manifestPath: string, org: string): Promise<void> {
  const manifest = JSON.parse(await fs.readFile(manifestPath, "utf-8")) as Record<string, unknown>;
  const { $schema, schemaVersion, slug, name, ...rest } = manifest;
  const ordered: Record<string, unknown> = {};
  if ($schema !== undefined) ordered.$schema = $schema;
  if (schemaVersion !== undefined) ordered.schemaVersion = schemaVersion;
  if (slug !== undefined) ordered.slug = slug;
  if (name !== undefined) ordered.name = name;
  ordered.orgSlug = org;
  await fs.writeFile(manifestPath, `${JSON.stringify({ ...ordered, ...rest }, null, 2)}\n`, "utf-8");
}

async function copyTemplate(
  templateDir: string,
  targetDir: string,
  vars: { name: string; slug: string }
): Promise<void> {
  await fs.mkdir(targetDir, { recursive: true });
  const entries = await fs.readdir(templateDir, { withFileTypes: true });
  for (const entry of entries) {
    const src = path.join(templateDir, entry.name);
    // Rename `_gitignore` → `.gitignore` so npm doesn't strip
    // dotfiles from the published package. Same trick as create-vite.
    // Also rename `.example` files to the real path the scaffolded app
    // wants. The suffix exists so the server-side admin-scaffold can filter
    // them: that path writes into `apps/<org>/<app>/` of the customer-apps
    // monorepo, where a per-app copy would conflict with something the repo
    // already owns at its root — a shared deploy workflow, and (for the
    // functions template) the pnpm workspace root itself. A standalone CLI
    // scaffold owns its whole directory and genuinely wants both files.
    const destName = templateDestName(entry.name);
    const dest = path.join(targetDir, destName);
    if (entry.isDirectory()) {
      await copyTemplate(src, dest, vars);
    } else if (entry.name === "template.json" || entry.name === "screenshot.png") {
      // Metadata for the admin gallery only — don't ship to the
      // scaffolded app.
      continue;
    } else {
      const bytes = await fs.readFile(src, "utf-8");
      // Use the same placeholders as the server-side scaffold renderer so a
      // single template set serves both code paths.
      const substituted = bytes
        .replace(/\{\{APP_DISPLAY_NAME\}\}/g, vars.name)
        .replace(/\{\{APP_SLUG\}\}/g, vars.slug);
      await fs.writeFile(dest, substituted, "utf-8");
    }
  }
}

async function main(): Promise<void> {
  const args = parseArgs(process.argv.slice(2));
  if (args.help) {
    printHelp();
    return;
  }
  if (!args.name) {
    console.error("error: app name is required");
    printHelp();
    process.exit(1);
  }
  const slug = slugify(args.name);
  // Same rule as the server's `is_valid_slug` and the vite plugin: 1–63 chars,
  // single hyphens only. `slugify` collapses `--` and trims hyphens but caps no
  // length, so a long name would otherwise scaffold an app that 422s at publish.
  if (!isValidSlug(slug)) {
    console.error(
      `error: name "${args.name}" yields slug "${slug}", which is invalid; ` +
        `pick a name that yields 1–63 lowercase letters, digits and single hyphens`
    );
    process.exit(1);
  }
  if (args.org !== null && !isValidSlug(args.org)) {
    console.error(
      `error: --org "${args.org}" is not an org slug; it is the lowercase, hyphenated ` +
        "name in the org's URL (oxyc api /api/orgs lists them)"
    );
    process.exit(1);
  }

  const templateDir = path.join(TEMPLATES_DIR, args.template);
  if (!existsSync(templateDir)) {
    console.error(`error: unknown template "${args.template}"`);
    const available = await fs.readdir(TEMPLATES_DIR);
    console.error(`available: ${available.join(", ")}`);
    process.exit(1);
  }

  const targetDir = path.resolve(process.cwd(), args.name);
  if (existsSync(targetDir)) {
    const contents = await fs.readdir(targetDir);
    if (contents.length > 0) {
      console.error(`error: ${targetDir} already exists and is non-empty`);
      process.exit(1);
    }
  }

  console.log(`Creating ${args.name} from template "${args.template}"...`);
  await copyTemplate(templateDir, targetDir, { name: args.name, slug });
  if (args.org !== null) {
    await writeOrgSlug(path.join(targetDir, "oxy-app.json"), args.org);
  }
  // Without an org the scaffold still works, but every `pnpm dev` and
  // `oxyc publish` will ask for one — say so here, where adding it is one key.
  const orgNote =
    args.org === null
      ? `
oxy-app.json has no "orgSlug" yet: add "orgSlug": "<org>" to it (or re-run with
--org <slug>) so \`pnpm dev\` and \`oxyc publish\` find the app without --org / OXY_ORG.
`
      : "";
  console.log(`
Done. Next steps:

  cd ${args.name}
  pnpm install
  pnpm dev          # Vite dev server at http://localhost:5173
${orgNote}
Ship it with \`oxyc publish\` (\`npm install -g @oxy-hq/cli\`): the first publish
registers the app in its org's workspace, or register it via the admin UI
(Custom apps → Add new) — the Link flow picks up oxy-app.json automatically.
`);
}

// Run main() only when invoked as a CLI, not when imported by tests.
// `import.meta.url` is the module URL; `process.argv[1]` is the
// script entrypoint Node was invoked with. They match when this
// file is the entry; differ when vitest imports it.
const invokedDirectly =
  import.meta.url === `file://${process.argv[1]}` ||
  import.meta.url.endsWith(path.basename(process.argv[1] ?? ""));

if (invokedDirectly) {
  main().catch((err) => {
    console.error(`error: ${err instanceof Error ? err.message : String(err)}`);
    process.exit(1);
  });
}

// Exported for unit tests.
export { parseArgs, slugify };
