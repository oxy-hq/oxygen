import { describe, expect, it } from "vitest";
import {
  entriesReaching,
  importSpecifiers,
  pageEntries,
  resolveSpecifier,
  reverseGraph,
  routeTable
} from "./imports";

describe("routeTable", () => {
  it("keeps route lines and their nesting, drops the rest", () => {
    const app = [
      "import X from 'x';",
      "function Shell() {",
      "  const a = useThing();",
      "  return (",
      "    <Routes>",
      "      <Route index element={<Launcher />} />",
      "      <Route",
      "        path='ide'",
      "        element={<Ide />}",
      "      >",
      "      </Route>",
      "    </Routes>"
    ].join("\n");
    expect(routeTable(app)).toBe(
      [
        "    <Routes>",
        "      <Route index element={<Launcher />} />",
        "      <Route",
        "        path='ide'",
        "        element={<Ide />}",
        "      </Route>",
        "    </Routes>"
      ].join("\n")
    );
  });
});

const FILES: Record<string, string> = {
  "web-app/src/App.tsx": [
    'import ThreadPage from "@/pages/thread";',
    'import { Button } from "@/components/ui/button";',
    'const IdePage = React.lazy(() => import("./pages/ide"));'
  ].join("\n"),
  "web-app/src/pages/thread/index.tsx":
    'import { Composer } from "./Composer";\nimport "./thread.css";',
  "web-app/src/pages/thread/Composer.tsx":
    'import {\n  Button,\n  Badge\n} from "@/components/ui/button";',
  "web-app/src/pages/ide/index.tsx": 'export { Panel } from "../../components/Panel";',
  "web-app/src/components/Panel.tsx": 'import { Button } from "@/components/ui/button";',
  "web-app/src/components/ui/button.tsx": 'import React from "react";'
};
const read = (p: string) => FILES[p];
const files = Object.keys(FILES);
const known = new Set(files);

describe("importSpecifiers", () => {
  it("finds static, multi-line, re-export, side-effect and dynamic imports", () => {
    expect(importSpecifiers(FILES["web-app/src/pages/thread/Composer.tsx"])).toEqual([
      "@/components/ui/button"
    ]);
    expect(importSpecifiers(FILES["web-app/src/pages/thread/index.tsx"])).toEqual([
      "./Composer",
      "./thread.css"
    ]);
    expect(importSpecifiers(FILES["web-app/src/pages/ide/index.tsx"])).toEqual([
      "../../components/Panel"
    ]);
    expect(importSpecifiers(FILES["web-app/src/App.tsx"])).toContain("./pages/ide");
  });
});

describe("resolveSpecifier", () => {
  it("resolves the @ alias, relative paths and index files; ignores packages", () => {
    expect(resolveSpecifier("web-app/src/App.tsx", "@/pages/thread", known)).toBe(
      "web-app/src/pages/thread/index.tsx"
    );
    expect(
      resolveSpecifier("web-app/src/pages/ide/index.tsx", "../../components/Panel", known)
    ).toBe("web-app/src/components/Panel.tsx");
    expect(resolveSpecifier("web-app/src/App.tsx", "react", known)).toBeUndefined();
  });
});

describe("entriesReaching", () => {
  const importers = reverseGraph(files, read);
  const entries = pageEntries(FILES["web-app/src/App.tsx"], known);

  it("binds App.tsx's page imports, static and lazy", () => {
    expect([...entries.values()].sort()).toEqual(["IdePage", "ThreadPage"]);
  });

  it("walks a shared component up to every page that renders it", () => {
    const hits = entriesReaching(["web-app/src/components/ui/button.tsx"], importers, entries);
    expect(hits.map((h) => h.entry).sort()).toEqual(["IdePage", "ThreadPage"]);
    expect(hits.find((h) => h.entry === "IdePage")?.via).toEqual([
      "web-app/src/pages/ide/index.tsx",
      "web-app/src/components/Panel.tsx",
      "web-app/src/components/ui/button.tsx"
    ]);
  });

  it("returns a changed page itself", () => {
    const hits = entriesReaching(["web-app/src/pages/thread/index.tsx"], importers, entries);
    expect(hits).toEqual([
      {
        entry: "ThreadPage",
        file: "web-app/src/pages/thread/index.tsx",
        via: ["web-app/src/pages/thread/index.tsx"]
      }
    ]);
  });
});
