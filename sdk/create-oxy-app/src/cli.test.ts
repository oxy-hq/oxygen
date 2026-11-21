import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, test } from "vitest";
import { parseArgs, slugify, templateDestName, writeOrgSlug } from "./cli.js";

describe("parseArgs", () => {
  test("captures the first positional as name, default template is vite", () => {
    expect(parseArgs(["my-app"])).toMatchObject({ name: "my-app", template: "vite" });
  });

  test("--template <id>", () => {
    expect(parseArgs(["my-app", "--template", "dashboard"])).toMatchObject({
      name: "my-app",
      template: "dashboard"
    });
  });

  test("--template=<id>", () => {
    expect(parseArgs(["my-app", "--template=single-store"])).toMatchObject({
      template: "single-store"
    });
  });

  test("--template functions (the opt-in server-side template)", () => {
    expect(parseArgs(["my-app", "--template", "functions"])).toMatchObject({
      name: "my-app",
      template: "functions"
    });
  });

  test("-t <id> short form", () => {
    expect(parseArgs(["x", "-t", "dashboard"]).template).toBe("dashboard");
  });

  test("--help", () => {
    expect(parseArgs(["--help"]).help).toBe(true);
    expect(parseArgs(["-h"]).help).toBe(true);
  });

  test("ignores subsequent positionals", () => {
    expect(parseArgs(["first", "second", "third"]).name).toBe("first");
  });
});

describe("slugify", () => {
  test("lowercases", () => {
    expect(slugify("StorePulse")).toBe("storepulse");
  });

  test("replaces spaces with dashes", () => {
    expect(slugify("Store Pulse App")).toBe("store-pulse-app");
  });

  test("collapses runs of dashes", () => {
    expect(slugify("a -- b")).toBe("a-b");
  });

  test("strips leading and trailing dashes", () => {
    expect(slugify("--leading-trailing--")).toBe("leading-trailing");
  });

  test("strips non-alphanumeric characters", () => {
    expect(slugify("hello!world@2024")).toBe("hello-world-2024");
  });
});

describe("templateDestName", () => {
  test("_gitignore becomes .gitignore (npm strips real dotfiles)", () => {
    expect(templateDestName("_gitignore")).toBe(".gitignore");
  });

  // Both spellings must be stripped: the server-side scaffold
  // filters `.yml.example` AND
  // `.yaml.example` so those files never land in the customer-apps monorepo,
  // so a standalone CLI scaffold has to rename both back or it loses them.
  test("strips .yml.example (shared deploy workflow)", () => {
    expect(templateDestName("deploy.yml.example")).toBe("deploy.yml");
  });

  test("strips .yaml.example (the functions template's pnpm workspace root)", () => {
    expect(templateDestName("pnpm-workspace.yaml.example")).toBe("pnpm-workspace.yaml");
  });

  test("leaves ordinary filenames alone", () => {
    expect(templateDestName("package.json")).toBe("package.json");
    expect(templateDestName("vite.config.ts")).toBe("vite.config.ts");
  });
});

describe("--org", () => {
  test("is captured in both spellings, and defaults to none", () => {
    expect(parseArgs(["my-app"]).org).toBeNull();
    expect(parseArgs(["my-app", "--org", "acme"]).org).toBe("acme");
    expect(parseArgs(["my-app", "--org=acme"]).org).toBe("acme");
  });

  test("writes orgSlug into the scaffolded manifest, after the identity keys", async () => {
    const dir = mkdtempSync(join(tmpdir(), "create-oxy-app-"));
    try {
      const manifest = join(dir, "oxy-app.json");
      writeFileSync(
        manifest,
        JSON.stringify({
          $schema: "https://unpkg.com/@oxy-hq/sdk/schema/oxy-app.schema.json",
          schemaVersion: 2,
          slug: "store-pulse",
          name: "Store Pulse",
          icon: "icon.svg"
        })
      );
      await writeOrgSlug(manifest, "acme");
      const written = readFileSync(manifest, "utf8");
      expect(Object.keys(JSON.parse(written))).toEqual([
        "$schema",
        "schemaVersion",
        "slug",
        "name",
        "orgSlug",
        "icon"
      ]);
      expect(JSON.parse(written).orgSlug).toBe("acme");
      expect(written.endsWith("\n")).toBe(true);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
