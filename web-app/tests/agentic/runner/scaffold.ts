// `pnpm test:agentic --scaffold <feature> --from <file-path>` —
// bootstraps a starter `flows/<feature>.flow.test.yml` based on the
// surface inferred from the source path and the data-testid attributes
// present in nearby files.
//
// Goal: a Claude agent fixing a chat-panel bug runs
//   pnpm test:agentic --scaffold chat-agent-switch \
//     --from src/pages/home/components/AgentSelector.tsx
// and gets a starter YAML with relevant testids pre-quoted, so they
// can fill in the act: prompts and expect: claims without first
// hunting around for selectors.

import { existsSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

interface SurfaceInfer {
  target: "chat" | "ide" | "threads" | "onboarding" | "any";
}

function inferSurface(filePath: string): SurfaceInfer {
  const norm = filePath.replace(/\\/g, "/");
  if (/\/pages\/home\//.test(norm) || /\/components\/Chat\//.test(norm)) {
    return { target: "chat" };
  }
  if (/\/pages\/ide\//.test(norm)) {
    return { target: "ide" };
  }
  if (/\/pages\/threads\//.test(norm)) {
    return { target: "threads" };
  }
  if (
    /\/pages\/onboarding\//.test(norm) ||
    /\/components\/workspaces\//.test(norm) ||
    /\/components\/org\//.test(norm)
  ) {
    return { target: "onboarding" };
  }
  return { target: "any" };
}

const TESTID_RE = /data-testid\s*=\s*['"`]([^'"`]+)['"`]|data-testid\s*=\s*\{`([^`]+)`\}/g;

function discoverTestIds(rootPath: string, depth = 1): string[] {
  const ids = new Set<string>();
  const walk = (path: string, depthLeft: number) => {
    if (!existsSync(path)) return;
    const stat = statSync(path);
    if (stat.isFile() && /\.(tsx|ts|jsx|js)$/.test(path)) {
      const content = readFileSync(path, "utf-8");
      for (const match of content.matchAll(TESTID_RE)) {
        const id = match[1] ?? match[2];
        if (id) ids.add(id);
      }
    } else if (stat.isDirectory() && depthLeft > 0) {
      for (const entry of readdirSync(path)) {
        if (entry.startsWith(".")) continue;
        if (entry === "node_modules" || entry === "dist") continue;
        walk(resolve(path, entry), depthLeft - 1);
      }
    }
  };

  // Walk the file's directory and one level up — close-by testids are
  // the most relevant for a flow targeting that file.
  if (statSync(rootPath).isFile()) {
    walk(rootPath, 0);
    walk(dirname(rootPath), depth);
    walk(dirname(dirname(rootPath)), Math.max(0, depth - 1));
  } else {
    walk(rootPath, depth);
  }
  return [...ids].sort();
}

interface ScaffoldArgs {
  featureName: string;
  fromPath: string;
  outPath: string;
}

export function runScaffold({ featureName, fromPath, outPath }: ScaffoldArgs): void {
  if (!existsSync(fromPath)) {
    console.error(`[scaffold] --from path does not exist: ${fromPath}`);
    process.exit(2);
  }
  if (existsSync(outPath)) {
    console.error(`[scaffold] flow already exists: ${outPath}`);
    process.exit(2);
  }

  const surface = inferSurface(fromPath);
  const testids = discoverTestIds(fromPath);
  const featurePretty = featureName.replace(/[-_]/g, " ");
  const tags = surface.target === "any" ? "[smoke]" : `[${surface.target}, smoke]`;
  const testidExpects =
    testids.length > 0
      ? testids
          .slice(0, 5)
          .map((id) => `      - assert: "selector [data-testid=${id}] is visible"`)
          .join("\n")
      : '      - assert: "selector body is visible"';
  const testidActHint =
    testids.length > 0
      ? `\n          # Relevant testids found nearby:\n${testids
          .slice(0, 8)
          .map((id) => `          #   [data-testid=${id}]`)
          .join("\n")}\n`
      : "";

  const yaml = `# yaml-language-server: $schema=../../../../json-schemas/flow-test.json
#
# ${featurePretty} flow — scaffolded from ${fromPath}
#
# TODO: fill in the act: prompt and expect: claims, then run cold:
#   pnpm test:agentic ${featureName}
# After it passes once, run again to verify the warm replay.
name: ${featurePretty}
target: ${surface.target}

settings:
  runs: 1
  trace: on-failure
  cache_actions: true
  max_steps: 30

setup:
  - "goto:/"

cases:
  - name: TODO describe the case
    tags: ${tags}
    steps:
      - act: |
          TODO: describe the action sequence in natural language.
          Prefer [data-testid=…] selectors over text= or role= when
          available.${testidActHint}

    expect:
${testidExpects}
`;

  writeFileSync(outPath, yaml, "utf-8");
  console.log(`[scaffold] wrote ${outPath}`);
  console.log(`[scaffold] target: ${surface.target}`);
  console.log(`[scaffold] testids found nearby: ${testids.length}`);
  if (testids.length > 0) {
    console.log("[scaffold] sample:");
    for (const id of testids.slice(0, 5)) console.log(`  [data-testid=${id}]`);
  }
}
