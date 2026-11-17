// @vitest-environment node
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import contractJson from "./contract/contract.json";
import pairsJson from "./contract/pairs.json";

// contract.json is the machine form of the theme contract (every name a direction fills, its
// type, modes and DTCG path). This proves the conversion: the counts the contract states, the
// flattening rule every authored path must obey, aliases that land on authored tokens, fixed rows
// that agree with the files that own them, and contrast obligations that name real tokens.

interface Row {
  name: string;
  group: string;
  type: string;
  modes?: "per-mode" | "both";
  path?: string;
  target?: string;
  value?: string;
  owner?: string;
  description: string;
}
interface Pair {
  fg: string;
  bg: string;
  over?: string;
  min: number;
  kind: string;
  note: string;
}

const here = dirname(fileURLToPath(import.meta.url));
const contract = contractJson as { version: number; counts: Record<string, number>; tokens: Row[] };
const pairs = pairsJson as Pair[];
const rows = contract.tokens;
const authored = rows.filter((r) => r.modes !== undefined);
const aliases = rows.filter((r) => r.type === "alias");
const fixed = rows.filter((r) => r.type === "fixed");
const byName = new Map(rows.map((r) => [r.name, r]));

/** The flattening rule: drop every "$root", drop a leading "color", join with "-", prefix "--". */
function flatten(path: string): string {
  const segs = path.split(".").filter((s) => s !== "$root");
  if (segs[0] === "color") segs.shift();
  return `--${segs.join("-")}`;
}

describe("theme contract", () => {
  it("is version 1", () => {
    expect(contract.version).toBe(1);
  });

  it("has 207 authored tokens (122 per-mode, 85 both), 30 derived aliases and 9 fixed names", () => {
    expect(authored).toHaveLength(207);
    expect(authored.filter((r) => r.modes === "per-mode")).toHaveLength(122);
    expect(authored.filter((r) => r.modes === "both")).toHaveLength(85);
    expect(aliases).toHaveLength(30);
    expect(fixed).toHaveLength(9);
    expect(rows).toHaveLength(207 + 30 + 9);
    expect(contract.counts).toEqual({
      authored: 207,
      perMode: 122,
      both: 85,
      aliases: 30,
      fixed: 9
    });
  });

  it("names every token once, as a lowercase custom property", () => {
    expect(new Set(rows.map((r) => r.name)).size).toBe(rows.length);
    for (const r of rows) expect(r.name).toMatch(/^--[a-z][a-z0-9-]*$/);
  });

  it("gives every authored token a contract type and a path that flattens to its own name", () => {
    const types = [
      "color",
      "length",
      "number",
      "em",
      "percent",
      "font-family",
      "font-weight",
      "shadow",
      "chevron"
    ];
    for (const r of authored) {
      expect(types, r.name).toContain(r.type);
      expect(r.path, r.name).toMatch(/^[a-z0-9$][a-z0-9$.-]*$/);
      expect(flatten(r.path ?? ""), `${r.name} from ${r.path}`).toBe(r.name);
    }
  });

  it("flattens the documented examples", () => {
    expect(flatten("color.background")).toBe("--background");
    expect(flatten("color.status.success.$root")).toBe("--status-success");
    expect(flatten("type.body-sm.size")).toBe("--type-body-sm-size");
    expect(flatten("control-h.lg")).toBe("--control-h-lg");
    expect(flatten("row-h")).toBe("--row-h");
    expect(flatten("tile.glyph-tint")).toBe("--tile-glyph-tint");
  });

  it("points every derived alias, through any chain, at an authored token", () => {
    for (const a of aliases) {
      let target = a.target;
      const seen = new Set([a.name]);
      while (target && byName.get(target)?.type === "alias" && !seen.has(target)) {
        seen.add(target);
        target = byName.get(target)?.target;
      }
      expect(byName.get(target ?? "")?.modes, `${a.name} -> ${a.target}`).toBeDefined();
    }
  });

  it("owns each fixed name outside direction files, with the value its owner sets", () => {
    const motion = readFileSync(join(here, "..", "motion", "motion.css"), "utf8");
    const motionValues = new Map(
      [...motion.matchAll(/(--oxy-[a-z-]+):\s*([^;]+);/g)].map((m) => [m[1], m[2].trim()])
    );
    for (const f of fixed) {
      if (f.name === "--oxy-theme-contract") {
        expect(f.owner).toBe("sdk/typescript/scripts/build-theme.mjs");
        expect(f.value).toBe(String(contract.version));
      } else {
        expect(f.owner, f.name).toBe("sdk/typescript/src/motion/motion.css");
        expect(motionValues.get(f.name), f.name).toBe(f.value);
      }
    }
  });

  it("states 182 contrast obligations, each naming authored colours", () => {
    expect(pairs).toHaveLength(182);
    const kinds = ["text", "large-text", "non-text", "focus", "control-border"];
    for (const p of pairs) {
      for (const name of [p.fg, p.bg, ...(p.over ? [p.over] : [])]) {
        expect(byName.get(name)?.type, `${p.fg} on ${p.bg}: ${name}`).toBe("color");
      }
      expect(kinds).toContain(p.kind);
      expect(p.min).toBeGreaterThan(1);
    }
  });
});
