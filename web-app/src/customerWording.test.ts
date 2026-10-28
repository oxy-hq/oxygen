import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { parseAst } from "vite";
import { describe, expect, it } from "vitest";

/**
 * With the apps-first experience on (`apps_first_experience`), a customer never sees
 * the word "workspace" (internal-docs/apps-first-product-plan.md, decision 4), nor a
 * file, a branch, a repository or a commit: they see their company and its apps.
 *
 * This parses the source of the screens a plain member reaches with the flag on
 * (`CUSTOMER_SCREENS`) and reports each piece of text a person could read that says
 * so: JSX text, and string or template literals that contain a space (prose) or are a
 * bare capitalised label. Parsing the source, rather than rendering screens, covers
 * the states a render test forgets (errors, empty states); a new string fails with a
 * file and line.
 *
 * With the flag off the screens say "workspace" as they always did (Luong,
 * 2026-10-27): the flag-off surfaces (workspace management, the switcher, the HQ
 * launcher) are simply not reached with it on. So the scan is scoped to the flag-on
 * screens, and text in those files that renders only with the flag off, or only for a
 * Factory user or staff, is listed by name below with why.
 *
 * What it does not see: text that arrives from the server and text built from a
 * variable; the plain-name call sites have their own tests.
 */

const SRC = path.resolve(__dirname);

/** The screens a plain member reaches with the flag on (path prefixes under `src/`). */
const CUSTOMER_SCREENS: readonly string[] = [
  "pages/launcher/",
  "pages/world-model/",
  "pages/chat/",
  "pages/automation/",
  "pages/onboarding/",
  "pages/thread/automation/",
  "pages/thread/analytics/BuilderDelegationPanel/",
  "components/org/OrgSetupPending/index.tsx",
  "components/Shell/",
  "components/Ask/",
  "components/RecentThreads/",
  "components/ThreadRow.tsx",
  "components/IdeUnavailableBanner/",
  "components/ArtifactPanel/ArtifactsContent/automation.tsx",
  "components/settings/SettingsDialog/components/VersionBadge/",
  "components/settings/SettingsDialog/sections/workspace/Airhouse/",
  "components/settings/SettingsDialog/sections/workspace/Oltp/",
  "components/settings/SettingsDialog/sections/workspace/ActivityLogs/",
  "hooks/api/agentic-automations/useLogItems.ts"
];

/** Files under `CUSTOMER_SCREENS` that render only with the flag off. */
const FLAG_OFF_ONLY: readonly { prefix: string; why: string }[] = [
  {
    prefix: "pages/launcher/HqLauncher.tsx",
    why: "The HQ launcher: LauncherPage renders it only with the flag off."
  },
  { prefix: "pages/launcher/components/AskForwardFallback.tsx", why: "Used only by HqLauncher." },
  { prefix: "pages/launcher/components/OtherWorkspaceApps.tsx", why: "Used only by HqLauncher." },
  { prefix: "pages/launcher/useOtherWorkspaceApps.ts", why: "Used only by HqLauncher." },
  {
    prefix: "components/Shell/RailWorkspaceSwitch.tsx",
    why: "The rail mounts it only with the flag off."
  }
];

interface Finding {
  file: string;
  line: number;
  text: string;
}

const PROSE_WITH_WORKSPACE = /workspace/i;
// Capitalised only: lower-case "workspace" alone is an id, a route segment or a key.
const BARE_LABEL = /^\s*Workspaces?\s*$/;

function sourceFiles(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      sourceFiles(full, out);
    } else if (/\.(ts|tsx)$/.test(entry.name) && !/\.(test|spec|d)\.(ts|tsx)$/.test(entry.name)) {
      out.push(full);
    }
  }
  return out;
}

/** True for text a person reads: prose, or a one-word label. Not an id, route or key. */
function isReadable(text: string): boolean {
  if (!PROSE_WITH_WORKSPACE.test(text)) return false;
  return /\s/.test(text.trim()) || BARE_LABEL.test(text);
}

type AstNode = { type: string; start: number; [key: string]: unknown };

function lineOf(source: string, offset: number): number {
  let line = 1;
  for (let i = 0; i < offset; i++) if (source.charCodeAt(i) === 10) line++;
  return line;
}

function findingsIn(file: string, readable: (text: string) => boolean = isReadable): Finding[] {
  const rel = path.relative(SRC, file).split(path.sep).join("/");
  const source = readFileSync(file, "utf8");
  const ast = parseAst(source, { lang: file.endsWith("x") ? "tsx" : "ts" }) as unknown as AstNode;
  const found: Finding[] = [];

  const add = (node: AstNode, text: string) => {
    const clean = text.replace(/\s+/g, " ").trim();
    if (readable(clean)) found.push({ file: rel, line: lineOf(source, node.start), text: clean });
  };

  const visit = (node: unknown, parent: AstNode | null) => {
    if (Array.isArray(node)) {
      for (const child of node) visit(child, parent);
      return;
    }
    if (!node || typeof node !== "object") return;
    const n = node as AstNode;
    if (typeof n.type === "string") {
      if (n.type === "JSXText") add(n, String(n.value));
      else if (n.type === "TemplateElement") add(n, String((n.value as { raw: string }).raw));
      else if (n.type === "Literal" && typeof n.value === "string") {
        const isModuleSpecifier =
          parent !== null &&
          (parent.type === "ImportDeclaration" ||
            parent.type === "ExportNamedDeclaration" ||
            parent.type === "ExportAllDeclaration");
        const isTypeLiteral = parent !== null && parent.type === "TSLiteralType";
        if (!isModuleSpecifier && !isTypeLiteral) add(n, n.value);
      }
    }
    for (const key of Object.keys(n)) {
      if (key === "type" || key === "start" || key === "end") continue;
      visit(n[key], typeof n.type === "string" ? n : parent);
    }
  };
  visit(ast, null);
  return found;
}

/** Text in a customer screen that only renders with the flag off, or only for a
 *  Factory user or staff. Each says which. */
const FLAG_OFF_OR_FACTORY_TEXT: readonly { file: string; text: string; why: string }[] = [
  {
    file: "components/Shell/TopBar/Breadcrumb.tsx",
    text: "Workspace",
    why: "Fallback when there is neither an org name nor a workspace name: local mode only."
  },
  {
    file: "components/Shell/WorkspaceShell.tsx",
    text: "Open any organization's Factory",
    why: "Tooltip of the Open Factory rail entry, which only operators (isOperator) get."
  },
  {
    file: "components/org/OrgSetupPending/index.tsx",
    text: "setting up your workspace and apps",
    why: "The flag-off wording; with the flag on it says the Oxy team is setting up the apps."
  },
  {
    file: "components/org/OrgSetupPending/index.tsx",
    text: "Create workspace",
    why: "The create button renders only with the flag off (canCreateWorkspace)."
  },
  {
    file: "components/settings/SettingsDialog/sections/workspace/Airhouse/components/CatalogIndexes.tsx",
    text: "this workspace",
    why: "Rendered only where canProvision: with the flag on, a Factory user."
  },
  {
    file: "components/settings/SettingsDialog/sections/workspace/Airhouse/components/ProvisionPrompt.tsx",
    text: "workspace",
    why: "Rendered only where canProvision: with the flag on, a Factory user."
  },
  {
    file: "components/settings/SettingsDialog/sections/workspace/Airhouse/index.tsx",
    text: "provisioned for this workspace",
    why: "The flag-off wording; with the flag on it says the Oxy team sets Airhouse up."
  },
  {
    file: "components/IdeUnavailableBanner/index.tsx",
    text: "File editing, Git, and live app runs are paused",
    why: "The Factory wording: flag off, or a Factory user. Otherwise it says live data is paused."
  },
  {
    file: "components/settings/SettingsDialog/components/VersionBadge/index.tsx",
    text: "Commit:",
    why: "Build details: flag off, or Oxy staff. Everyone else sees only the build's name."
  },
  {
    file: "components/settings/SettingsDialog/sections/workspace/Airhouse/components/ConnectionDetails.tsx",
    text: "config.yml",
    why: "The add-to-config action: flag off, or a Factory user (showConfigAction)."
  },
  {
    file: "components/settings/SettingsDialog/sections/workspace/Airhouse/components/ConnectionDetails.tsx",
    text: "Commit the file",
    why: "The same action's help text."
  },
  {
    file: "components/settings/SettingsDialog/sections/workspace/Airhouse/index.tsx",
    text: "config.yml",
    why: "Toasts of the add-to-config action, which renders only where showConfigAction does."
  },
  {
    file: "components/settings/SettingsDialog/sections/workspace/Oltp/index.tsx",
    text: "Add to config.yml",
    why: "The config snippet: flag off, or a Factory user (showConfigSnippet)."
  },
  {
    file: "hooks/api/agentic-automations/useLogItems.ts",
    text: "Reused from cached file",
    why: "Flag off only (hidePaths); with it on the line says it was reused from cache."
  },
  {
    file: "hooks/api/agentic-automations/useLogItems.ts",
    text: "branch:",
    why: "A conditional task's branch (which arm ran), not a git branch."
  },
  {
    file: "pages/automation/AutomationsListPage.tsx",
    text: "Create a `.automation.yml`",
    why: "Flag off only (plainNames); with it on the empty state says there are none yet."
  }
];

const FILE_WORDS =
  /\b(files?|folders?|branch(es)?|repo(s|sitory|sitories)?|commits?|git|working copy)\b|config\.ya?ml|\.(agentic|automation|procedure|workflow|app|view|topic)\.ya?ml/i;

/** Prose, or a capitalised one-word label, that names a filesystem concept. */
function namesFilesystem(text: string): boolean {
  if (!FILE_WORDS.test(text)) return false;
  return /\s/.test(text.trim()) || /^[A-Z]/.test(text.trim());
}

describe("apps-first customer screens", () => {
  const relOf = (file: string) => path.relative(SRC, file).split(path.sep).join("/");
  const flagOffOnly = (rel: string) => FLAG_OFF_ONLY.some((f) => rel.startsWith(f.prefix));
  const files = sourceFiles(SRC).filter((f) => {
    const rel = relOf(f);
    return CUSTOMER_SCREENS.some((prefix) => rel.startsWith(prefix)) && !flagOffOnly(rel);
  });
  const exempt = (f: Finding) =>
    FLAG_OFF_OR_FACTORY_TEXT.some((a) => a.file === f.file && f.text.includes(a.text));
  const report = (findings: Finding[]) =>
    findings.map((f) => `${f.file}:${f.line}  ${f.text.slice(0, 120)}`);

  it("never say workspace", () => {
    const findings = files.flatMap((f) => findingsIn(f)).filter((f) => !exempt(f));
    expect(
      report(findings),
      "With apps_first_experience on, a customer sees their company and its apps, never a workspace. Reword the text, or if it renders only with the flag off or only for a Factory user, add it to FLAG_OFF_OR_FACTORY_TEXT (or the file to FLAG_OFF_ONLY) and say why."
    ).toEqual([]);
  });

  it("never name a file, a branch, a repository or a commit", () => {
    const findings = files.flatMap((f) => findingsIn(f, namesFilesystem)).filter((f) => !exempt(f));
    expect(
      report(findings),
      "With apps_first_experience on, a customer never sees filesystem or git concepts. Use a plain name (objectNameFromPath, appDisplayLabel), or if the text renders only with the flag off or only for a Factory user, add it to FLAG_OFF_OR_FACTORY_TEXT and say why."
    ).toEqual([]);
  });

  it("keeps every listed screen and exemption pointing at something that exists", () => {
    const all = sourceFiles(SRC).map(relOf);
    const stale = [
      ...CUSTOMER_SCREENS.filter((p) => !all.some((f) => f.startsWith(p))),
      ...FLAG_OFF_ONLY.filter((f) => !all.some((a) => a.startsWith(f.prefix))).map((f) => f.prefix),
      ...FLAG_OFF_OR_FACTORY_TEXT.filter((a) => !all.includes(a.file)).map((a) => a.file)
    ];
    expect(stale).toEqual([]);
  });

  it("still finds the words where they are allowed, so the scan is not blind", () => {
    expect(
      findingsIn(path.join(SRC, "pages/admin/AdminWorkspaces/index.tsx")).length
    ).toBeGreaterThan(0);
    expect(
      findingsIn(path.join(SRC, "components/IdeUnavailableBanner/index.tsx"), namesFilesystem)
        .length
    ).toBeGreaterThan(0);
  });
});
