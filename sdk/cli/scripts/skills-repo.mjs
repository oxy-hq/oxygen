#!/usr/bin/env node
/**
 * The Oxy platform skills: validate the bundle, and render the public repo from it.
 *
 * `sdk/cli/skills/` is the ONE source. It ships inside `@oxy-hq/cli` (for
 * `oxyc skills install`) and is rendered into `oxy-hq/skills` by
 * `.github/workflows/skills-publish.yaml`, which is what `npx skills add
 * oxy-hq/skills` and the Claude Code plugin read. Nothing is edited in that repo.
 *
 * Validation is the Agent Skills spec (agentskills.io) plus one rule of ours. It
 * exists because four of the six skills once had a description with an unquoted
 * `: ` in it — valid to Claude Code's lenient reader, a YAML error to everyone
 * else's — so `npx skills add` silently installed two skills out of six.
 *
 *   node scripts/skills-repo.mjs check
 *   node scripts/skills-repo.mjs check-repo     every .claude/skills entry in this repo loads
 *   node scripts/skills-repo.mjs render <repo-dir> [--source-sha <sha>]
 */

import { execFileSync } from "node:child_process";
import {
  cpSync,
  existsSync,
  lstatSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  readlinkSync,
  rmSync,
  writeFileSync
} from "node:fs";
import { basename, dirname, join, normalize, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { parse } from "yaml";

export const SKILLS_DIR = resolve(dirname(fileURLToPath(import.meta.url)), "..", "skills");

const NAME = /^[a-z0-9]+(-[a-z0-9]+)*$/;
const PRIVATE_REFERENCE = /internal-docs\/|oxygen-internal|\bcrates\/[a-z]/;
const SPEC_KEYS = new Set([
  "name",
  "description",
  "license",
  "compatibility",
  "metadata",
  "allowed-tools"
]);

/** Every skill directory under `dir`, with its parsed frontmatter or the reason it has none. */
export function readBundle(dir = SKILLS_DIR) {
  return readdirSync(dir, { withFileTypes: true })
    .filter((e) => e.isDirectory() && !e.name.startsWith("."))
    .map((e) => e.name)
    .sort()
    .map((name) => readSkill(join(dir, name)));
}

/** One skill directory (a symlink to one is followed), parsed or with the reason it isn't. */
export function readSkill(skillDir) {
  const name = basename(skillDir);
  const file = join(skillDir, "SKILL.md");
  if (!existsSync(file)) return { name, dir: skillDir, error: "no SKILL.md" };
  const text = readFileSync(file, "utf8");
  const match = /^---\n([\s\S]*?)\n---\n/.exec(text);
  if (!match) return { name, dir: skillDir, error: "no frontmatter" };
  try {
    return { name, dir: skillDir, meta: parse(match[1]) };
  } catch (err) {
    return { name, dir: skillDir, error: `frontmatter is not YAML: ${firstLine(err)}` };
  }
}

/**
 * What would stop a skill anywhere in the repo from loading — every `<dir>/.claude/skills/<name>`
 * git tracks, directory or symlink. Those are not published, so only the loading rules apply:
 * a symlink resolves (the platform skills are linked into the crates that implement them, and a
 * rename leaves the links dangling), there is a SKILL.md, its frontmatter is strict YAML, and its
 * name is its directory.
 */
export function repoSkillProblems(root) {
  const tracked = execFileSync("git", ["ls-files", "-z"], { cwd: root, encoding: "utf8" });
  const entries = new Set();
  for (const path of tracked.split("\0")) {
    const m = /(?:^|\/)\.claude\/skills\/[^/]+/.exec(path);
    if (m) entries.add(path.slice(0, m.index + m[0].length).replace(/^\//, ""));
  }
  const out = [];
  for (const entry of [...entries].sort((a, b) => (a < b ? -1 : a > b ? 1 : 0))) {
    const abs = join(root, entry);
    if (lstatSync(abs).isSymbolicLink() && !existsSync(abs)) {
      out.push(`${entry}: a symlink to ${readlinkSync(abs)} that does not resolve`);
      continue;
    }
    const skill = readSkill(abs);
    if (skill.error) out.push(`${entry}: ${skill.error}`);
    else if (skill.meta?.name !== skill.name)
      out.push(`${entry}: name "${skill.meta?.name}" does not match its directory`);
  }
  return out;
}

/** Problems with one skill, as sentences. Empty means it installs everywhere. */
export function problems(skill) {
  if (skill.error) return [skill.error];
  const out = [];
  const { meta, name } = skill;
  if (!meta || typeof meta !== "object") return ["frontmatter is not a mapping"];
  if (meta.name !== name) out.push(`name "${meta.name}" does not match its directory "${name}"`);
  if (!NAME.test(name) || name.length > 64)
    out.push("name must be ≤64 chars of a-z, 0-9 and single hyphens");
  if (typeof meta.description !== "string" || meta.description.trim() === "") {
    out.push("description is missing");
  } else if (meta.description.length > 1024) {
    out.push(`description is ${meta.description.length} chars; the spec allows 1024`);
  }
  for (const key of Object.keys(meta)) {
    if (!SPEC_KEYS.has(key)) out.push(`frontmatter key "${key}" is not in the Agent Skills spec`);
  }
  for (const file of skillFiles(skill.dir)) {
    const from = dirname(file);
    const label =
      file === join(skill.dir, "SKILL.md") ? "" : `${file.slice(skill.dir.length + 1)} `;
    const text = readFileSync(file, "utf8");
    // The bundle is published publicly, templates and all; a pointer into the private
    // repo is a dead end for every reader but us, and a hint the skill restates code.
    const leak = PRIVATE_REFERENCE.exec(text);
    if (leak)
      out.push(`${label}points into the private repo ("${leak[0]}"); state the behaviour instead`);
    if (!file.endsWith(".md")) continue;
    for (const target of relativeLinks(text)) {
      // A link out of the skill directory works in this repo and 404s in every
      // installed copy, which carries the skill directory and nothing else.
      const abs = normalize(join(from, target));
      if (!abs.startsWith(skill.dir + sep)) out.push(`${label}links outside the skill: ${target}`);
      else if (!existsSync(abs)) out.push(`${label}links a file that does not exist: ${target}`);
    }
  }
  return out;
}

/** Every file a render publishes, i.e. everything in the skill directory. */
function skillFiles(dir) {
  return readdirSync(dir, { withFileTypes: true, recursive: true })
    .filter((e) => e.isFile())
    .map((e) => join(e.parentPath, e.name))
    .sort();
}

function relativeLinks(body) {
  const links = [];
  for (const [, href] of body.matchAll(/\]\(([^)\s]+)\)/g)) {
    if (/^([a-z]+:|#|\/)/i.test(href)) continue;
    links.push(decodeURI(href.split("#")[0]));
  }
  return links.filter(Boolean);
}

function firstLine(err) {
  return String(err?.message ?? err).split("\n")[0];
}

/** The bundle, or a thrown list of everything wrong with it. */
export function checkedBundle(dir = SKILLS_DIR) {
  const skills = readBundle(dir);
  const bad = skills.flatMap((s) => problems(s).map((p) => `  ${s.name}: ${p}`));
  if (bad.length) throw new Error(`the skills bundle is not installable:\n${bad.join("\n")}`);
  return skills;
}

/**
 * One line for the README table: the description's first sentence, cut on a word
 * if it runs long, with `<`/`>` escaped so `<org>/<slug>` is not read as HTML.
 */
export function summary(description) {
  const flat = description.replace(/\s+/g, " ").trim();
  const end = [...flat.matchAll(/[.!?](?=\s|$)/g)].find(
    (m) => !/\b(e\.g|i\.e|vs|etc)$/i.test(flat.slice(0, m.index))
  );
  let line = end ? flat.slice(0, end.index + 1) : flat;
  if (line.length > 220)
    line = `${line
      .slice(0, line.lastIndexOf(" ", 217))
      .replace(/[,;:—-]$/, "")
      .trimEnd()}…`;
  return line.replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

const MARKETPLACE = "oxy-hq";
const PLUGIN = "oxy";
const PLUGIN_DESCRIPTION =
  "Oxy platform skills for coding agents: custom apps and Oxy Functions, semantic models, " +
  "agents, automations, Airway pipelines, evals, and the oxyc CLI.";

export function marketplaceJson() {
  return {
    name: MARKETPLACE,
    owner: { name: "Oxy", email: "support@oxy.tech" },
    metadata: { description: "Official Oxy skills for coding agents" },
    plugins: [
      { name: PLUGIN, source: "./", description: PLUGIN_DESCRIPTION, category: "development" }
    ],
    // The plugin was `oxy-skills` until the bundle moved to oxygen-internal.
    // Claude Code rewrites existing installs on its own; keep this forever.
    renames: { "oxy-skills": PLUGIN }
  };
}

export function pluginJson() {
  // No `version`, deliberately: without one Claude Code versions the plugin by
  // commit, so every sync is an update. A pinned version would hold every
  // install on its cached copy until someone remembered to bump it.
  return {
    name: PLUGIN,
    description: PLUGIN_DESCRIPTION,
    author: { name: "Oxy", url: "https://oxy.tech" },
    homepage: "https://github.com/oxy-hq/skills",
    repository: "https://github.com/oxy-hq/skills",
    license: "MIT",
    keywords: ["oxy", "oxygen", "analytics", "semantic-model", "custom-apps", "agents", "skills"]
  };
}

export function readme(skills, sourceSha) {
  const rows = skills.map(
    (s) => `| \`${s.name}\` | ${summary(s.meta.description).replace(/\|/g, "\\|")} |`
  );
  const from = sourceSha ? ` at \`${sourceSha.slice(0, 12)}\`` : "";
  return `# Oxy skills

Skills that teach a coding agent — Claude Code, Cursor, Codex, Copilot and anything else that
reads [Agent Skills](https://agentskills.io) — how to build on [Oxy](https://oxy.tech).

## Install

Any agent:

\`\`\`bash
npx skills add oxy-hq/skills
\`\`\`

Claude Code, as a plugin:

\`\`\`text
/plugin marketplace add oxy-hq/skills
/plugin install ${PLUGIN}@${MARKETPLACE}
\`\`\`

With the Oxy CLI (\`oxyc\` ships the same skills):

\`\`\`bash
npm install -g @oxy-hq/cli && oxyc skills install
\`\`\`

To update: \`npx skills update\`, \`claude plugin update ${PLUGIN}@${MARKETPLACE}\`, or reinstall
\`@oxy-hq/cli\` and run \`oxyc skills install\` again.

## Skills

| Skill | Use it when |
| --- | --- |
${rows.join("\n")}

## Contributing

This repository is generated${from} from \`sdk/cli/skills/\` in Oxy's product repository, so a
skill changes in the same pull request as the behaviour it describes. Edits made here are
overwritten by the next sync — open an issue instead.
`;
}

/** Write the public repo's managed paths into `out`. Anything else there is left alone. */
export function render(out, { sourceSha, dir = SKILLS_DIR } = {}) {
  const skills = checkedBundle(dir);
  rmSync(join(out, "skills"), { recursive: true, force: true });
  for (const s of skills) cpSync(s.dir, join(out, "skills", s.name), { recursive: true });
  mkdirSync(join(out, ".claude-plugin"), { recursive: true });
  const json = (v) => `${JSON.stringify(v, null, 2)}\n`;
  writeFileSync(join(out, ".claude-plugin", "marketplace.json"), json(marketplaceJson()));
  writeFileSync(join(out, ".claude-plugin", "plugin.json"), json(pluginJson()));
  writeFileSync(join(out, "README.md"), readme(skills, sourceSha));
  return skills;
}

function main(argv) {
  const [command, target, ...rest] = argv;
  if (command === "check") {
    const skills = checkedBundle();
    console.log(`skills: ${skills.length} installable — ${skills.map((s) => s.name).join(", ")}`);
    return;
  }
  if (command === "check-repo") {
    const root = resolve(SKILLS_DIR, "..", "..", "..");
    const bad = repoSkillProblems(root);
    if (bad.length)
      throw new Error(`skills in this repo that will not load:\n  ${bad.join("\n  ")}`);
    console.log("skills: every .claude/skills entry in the repo loads");
    return;
  }
  if (command === "render" && target) {
    const sha = rest[0] === "--source-sha" ? rest[1] : undefined;
    const skills = render(resolve(target), { sourceSha: sha });
    console.log(`skills: rendered ${skills.length} into ${target}`);
    return;
  }
  console.error(
    "usage: skills-repo.mjs check | check-repo | render <repo-dir> [--source-sha <sha>]"
  );
  process.exit(2);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main(process.argv.slice(2));
  } catch (err) {
    console.error(
      firstLine(err) === String(err?.message) ? err.message : String(err?.message ?? err)
    );
    process.exit(1);
  }
}
