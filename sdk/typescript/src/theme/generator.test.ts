// @vitest-environment node
import { spawnSync } from "node:child_process";
import {
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, describe, expect, it } from "vitest";
import { ALIAS_SELECTOR, cssBlockMap, themeSelectors } from "./css-blocks";

// scripts/build-theme.mjs as a CLI: the committed outputs are what the tokens generate, two runs
// give the same bytes, the flattener matches the design validator's reference output for x0, and
// broken tokens fail with a message naming the file, the token path and the rule.

const here = dirname(fileURLToPath(import.meta.url));
const script = join(here, "..", "..", "scripts", "build-theme.mjs");
const x0 = join(here, "__fixtures__", "x0");
const scratch = mkdtempSync(join(tmpdir(), "oxy-theme-"));
afterAll(() => rmSync(scratch, { recursive: true, force: true }));

function build(...args: string[]) {
  const r = spawnSync(process.execPath, [script, ...args], { encoding: "utf8" });
  return { status: r.status, stdout: r.stdout, stderr: r.stderr };
}

function files(root: string): string[] {
  return readdirSync(root, { recursive: true, encoding: "utf8" })
    .filter((p) => statSync(join(root, p)).isFile())
    .map((p) => p.split("\\").join("/"))
    .sort();
}

type Json = Record<string, unknown>;

/** A copy of the x0 fixture, edited, in a folder named x0 (the id must match its folder). */
function brokenX0(name: string, edit: (files: Record<string, Json>) => void): string {
  const dir = join(scratch, name, "x0");
  cpSync(join(x0, "tokens"), join(dir, "tokens"), { recursive: true });
  const loaded: Record<string, Json> = {};
  for (const f of ["primitives", "base", "light", "dark"]) {
    loaded[f] = JSON.parse(readFileSync(join(dir, "tokens", `${f}.tokens.json`), "utf8")) as Json;
  }
  edit(loaded);
  for (const [f, data] of Object.entries(loaded))
    writeFileSync(join(dir, "tokens", `${f}.tokens.json`), JSON.stringify(data));
  return dir;
}
const group = (data: Json, key: string) => data[key] as Json;

describe("build-theme.mjs", () => {
  it("--check: the committed outputs are what the tokens generate", () => {
    const r = build("--check");
    expect(r.stderr).toBe("");
    expect(r.status).toBe(0);
    expect(r.stdout).toMatch(/outputs match the tokens/);
  });

  it("is deterministic: two runs write the same bytes", () => {
    const [a, b] = [join(scratch, "run-a"), join(scratch, "run-b")];
    expect(build("--out", a).status).toBe(0);
    expect(build("--out", b).status).toBe(0);
    const listed = files(a);
    expect(listed).toEqual(files(b));
    expect(listed).toContain("sdk/typescript/src/theme/theme.css");
    expect(listed).toContain("mobile/design/theme/OxyTheme.kt");
    for (const f of listed)
      expect(readFileSync(join(a, f)).equals(readFileSync(join(b, f))), f).toBe(true);
  });

  it("--check names an output that differs, is missing or is stale", () => {
    const out = join(scratch, "drift");
    expect(build("--out", out).status).toBe(0);
    expect(build("--check", "--out", out).status).toBe(0);
    writeFileSync(join(out, "sdk/typescript/src/theme/theme.css"), "/* edited */\n");
    rmSync(join(out, "mobile/design/theme/OxyTheme.swift"));
    mkdirSync(join(out, "web-app/src/styles/shadcn/fonts"), { recursive: true });
    writeFileSync(join(out, "web-app/src/styles/shadcn/fonts/Old.woff2"), "");
    // A theme-fonts.css from a build that still had faces: nothing writes it now.
    writeFileSync(join(out, "sdk/typescript/src/theme/theme-fonts.css"), "");
    const r = build("--check", "--out", out);
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("sdk/typescript/src/theme/theme.css (differs)");
    expect(r.stderr).toContain("mobile/design/theme/OxyTheme.swift (missing)");
    expect(r.stderr).toContain("web-app/src/styles/shadcn/fonts/Old.woff2 (stale");
    expect(r.stderr).toContain("sdk/typescript/src/theme/theme-fonts.css (stale");
    // Writing clears them: the stale files and the emptied fonts folder go.
    expect(build("--out", out).status).toBe(0);
    expect(existsSync(join(out, "sdk/typescript/src/theme/theme-fonts.css"))).toBe(false);
    expect(existsSync(join(out, "web-app/src/styles/shadcn/fonts"))).toBe(false);
    expect(build("--check", "--out", out).status).toBe(0);
  });

  it("matches the reference flattener on x0, value for value", () => {
    const r = build("--emit-css", x0);
    expect(r.stderr).toBe("");
    const generated = cssBlockMap(r.stdout);
    const reference = cssBlockMap(readFileSync(join(x0, "x0.theme.css"), "utf8"));
    // The reference flattener keys every block on data-oxy-theme; this generator keys a direction
    // that is not the shipped one the same way, inside :where(). Same blocks, same order.
    const selectors = themeSelectors("x0", "reference");
    expect([...generated.keys()]).toEqual([selectors.light, selectors.dark, ALIAS_SELECTOR]);
    expect([...reference.keys()]).toEqual([
      ':root[data-oxy-theme="x0"]',
      ':root[data-oxy-theme="x0"][data-oxy-mode="dark"]',
      ":root[data-oxy-theme]"
    ]);
    // The reference flattener writes a translucent colour as `rgb(r g b / a)`; this generator writes
    // the same colour as `rgba(r, g, b, a)`, which Chrome paints as apps write it (colorCss).
    const legacy = (v: string) =>
      v.replace(/\brgb\((\d+) (\d+) (\d+) \/ ([\d.]+)\)/g, "rgba($1, $2, $3, $4)");
    const pairs = [...reference.values()].map((decls, i) => [decls, [...generated.values()][i]]);
    for (const [want, got] of pairs)
      expect(Object.fromEntries(got ?? []), "block").toEqual(
        Object.fromEntries([...(want ?? [])].map(([k, v]) => [k, legacy(v)]))
      );
  });

  it("never emits the x0 fixture", () => {
    expect(readFileSync(join(here, "theme.css"), "utf8")).not.toContain('data-oxy-theme="x0"');
  });

  const color = (f: Json) => group(f, "color");
  const meta = (f: Json) => group(group(f, "$extensions"), "com.oxygen");
  const failures: [string, (f: Record<string, Json>) => void, RegExp][] = [
    [
      "a missing token",
      (f) => {
        delete color(f.light).background;
      },
      /light\.tokens\.json: color\.background: contract\.missing:/
    ],
    [
      "a name outside the contract",
      (f) => {
        color(f.light)["not-a-token"] = { $value: "{palette.neutral.light.1}" };
      },
      /light\.tokens\.json: color\.not-a-token: contract\.extra:/
    ],
    [
      "an authored alias",
      (f) => {
        color(f.dark).input = { $value: "{palette.neutral.dark.8}" };
      },
      /dark\.tokens\.json: color\.input: contract\.derived-authored:/
    ],
    [
      "an authored fixed name",
      (f) => {
        f.base.oxy = { "duration-fast": { $type: "number", $value: 120 } };
      },
      /base\.tokens\.json: oxy\.duration-fast: contract\.fixed-override:/
    ],
    [
      "a per-mode token in base",
      (f) => {
        f.base.color = { $type: "color", brand: { $value: "{palette.neutral.light.9}" } };
      },
      /base\.tokens\.json: color\.brand: files\.placement:/
    ],
    [
      "a palette entry outside primitives",
      (f) => {
        f.light.palette = {
          x: {
            $type: "color",
            $value: { colorSpace: "srgb", components: [0, 0, 0], hex: "#000000" }
          }
        };
      },
      /light\.tokens\.json: palette\.x: files\.placement:/
    ],
    [
      "an alias cycle",
      (f) => {
        color(f.light).background = { $value: "{color.foreground}" };
        color(f.light).foreground = { $value: "{color.background}" };
      },
      /light\.tokens\.json: color\.(background|foreground): alias\.cycle:/
    ],
    [
      "an alias to nothing",
      (f) => {
        color(f.dark).card = { $value: "{palette.neutral.dark.99}" };
      },
      /dark\.tokens\.json: color\.card: alias\.unresolved:/
    ],
    [
      "an alias of the wrong type",
      (f) => {
        color(f.light).card = { $value: "{radius.control}" };
      },
      /light\.tokens\.json: color\.card: alias\.type:/
    ],
    [
      "a three-digit hex",
      (f) => {
        const ramp = group(group(group(f.primitives, "palette"), "neutral"), "light");
        ramp["1"] = { $value: { colorSpace: "srgb", components: [1, 1, 1], hex: "#fff" } };
      },
      /primitives\.tokens\.json: palette\.neutral\.light\.1: color\.syntax: hex must be 6-digit/
    ],
    [
      "a colour without its object form",
      (f) => {
        color(f.light).scrim = { $value: "#00000080" };
      },
      /light\.tokens\.json: color\.scrim: color\.syntax:/
    ],
    [
      "a token whose group lost its $type",
      (f) => {
        delete color(f.light).$type;
      },
      /light\.tokens\.json: color\.row-hover: contract\.type:/
    ],
    [
      "a folder that is not its id",
      (f) => {
        meta(f.base).direction = "x9";
      },
      /base\.tokens\.json: \$extensions\.com\.oxygen\.direction: files\.meta: is "x9" but the folder is "x0"/
    ],
    [
      "an id outside [a-z][a-z0-9-]*",
      (f) => {
        meta(f.base).direction = "X0";
      },
      /base\.tokens\.json: \$extensions\.com\.oxygen\.direction: files\.meta: must match/
    ],
    [
      "another contract version",
      (f) => {
        meta(f.base).contract = 2;
      },
      /base\.tokens\.json: \$extensions\.com\.oxygen\.contract: contract\.version:/
    ],
    [
      "a name outside the alphabet",
      (f) => {
        color(f.light).Background = { $value: "{palette.neutral.light.2}" };
      },
      /light\.tokens\.json: color\.Background: names\.syntax:/
    ],
    [
      "an em token without its unit",
      (f) => {
        delete group(group(group(f.base, "type"), "body"), "tracking").$extensions;
      },
      /base\.tokens\.json: type\.body\.tracking: number\.unit:/
    ]
  ];

  it.each(failures)("fails on %s, naming file, path and rule", (name, edit, message) => {
    const r = build("--emit-css", brokenX0(name.replace(/\W+/g, "-"), edit));
    expect(r.status, r.stderr).toBe(1);
    expect(r.stdout).toBe("");
    expect(r.stderr).toMatch(message);
  });

  it("writes only the paths it owns", () => {
    const out = join(scratch, "owned");
    expect(build("--out", out).status).toBe(0);
    const owned = [
      "sdk/typescript/src/theme/",
      "mobile/design/theme/",
      "web-app/src/styles/shadcn/sdk-theme.css"
    ];
    for (const f of files(out))
      expect(
        owned.some((p) => f.startsWith(p)),
        f
      ).toBe(true);
  });
});

// The full build over a copy of src/theme (--src): what only the full build checks, namely the
// direction list and the fonts, fails naming the file and the rule, and writes nothing. The
// reference self-hosts no faces, so the font cases add one to a copy first.
describe("build-theme.mjs --src", () => {
  /** A copy of the inputs (contract, tokens, fonts), edited. */
  function source(name: string, edit: (dir: string) => void = () => {}): string {
    const dir = join(scratch, `src-${name}`);
    for (const part of ["contract", "tokens"])
      cpSync(join(here, part), join(dir, part), { recursive: true });
    edit(dir);
    return dir;
  }
  const fontsJson = (dir: string, id: string) => join(dir, "tokens", id, "fonts.json");
  type Faces = { faces: { family: string }[] } & Json;
  const editFonts = (dir: string, id: string, edit: (f: Faces) => void) => {
    const data = JSON.parse(readFileSync(fontsJson(dir, id), "utf8")) as Faces;
    edit(data);
    writeFileSync(fontsJson(dir, id), JSON.stringify(data));
  };

  it("reads a copy: unchanged, it generates exactly the committed outputs", () => {
    const r = build("--src", source("same"), "--check");
    expect(r.stderr).toBe("");
    expect(r.status).toBe(0);
    expect(r.stdout).toMatch(/outputs match the tokens/);
  });

  it("writes only under --out, and needs a contract to read", () => {
    const write = build("--src", source("no-out"));
    expect(write.status).toBe(2);
    expect(write.stderr).toContain("--src writes only under --out");
    const empty = join(scratch, "src-empty");
    mkdirSync(empty, { recursive: true });
    const r = build("--src", empty, "--check");
    expect(r.status).toBe(2);
    expect(r.stderr).toContain("has no contract/contract.json");
  });

  /** Make the copy's font.sans lead with a self-hosted family, as a direction with its own face. */
  const leadWith = (dir: string, family: string) => {
    const file = join(dir, "tokens", "reference", "base.tokens.json");
    const base = JSON.parse(readFileSync(file, "utf8")) as { font: { sans: { $value: string[] } } };
    base.font.sans.$value = [family, ...base.font.sans.$value];
    writeFileSync(file, JSON.stringify(base));
  };
  const fullBuildFailures: [string, (dir: string) => void, RegExp][] = [
    [
      "a token folder that is not a direction",
      (dir) =>
        cpSync(join(dir, "tokens", "reference"), join(dir, "tokens", "d5"), { recursive: true }),
      /tokens\/d5: \(root\): files\.directions: is not one of reference/
    ],
    [
      "a missing direction",
      (dir) => rmSync(join(dir, "tokens", "reference"), { recursive: true }),
      /tokens\/reference: \(root\): files\.directions: is missing/
    ],
    [
      "a font token whose family has no face",
      (dir) => leadWith(dir, "Chivo"),
      /: font\.face-missing: --font-sans leads with "Chivo", which tokens\/reference\/fonts\.json does not serve/
    ],
    [
      "a font file no face uses",
      (dir) => {
        mkdirSync(join(dir, "fonts"));
        writeFileSync(join(dir, "fonts", "Unused-Face.woff2"), "");
      },
      /fonts\/Unused-Face\.woff2: \(root\): font\.orphan: no face in any fonts\.json uses it/
    ],
    [
      "a face whose file is missing",
      (dir) =>
        editFonts(dir, "reference", (f) =>
          f.faces.push({
            family: "Chivo",
            file: "Chivo-Variable.woff2",
            weight: "100 900",
            style: "normal",
            licence: "OFL-Chivo.txt"
          } as never)
        ),
      /tokens\/reference\/fonts\.json: faces\[0\]: font\.face: .+Chivo-Variable\.woff2 does not exist/
    ],
    [
      "a missing fonts.json",
      (dir) => rmSync(fontsJson(dir, "reference")),
      /tokens\/reference\/fonts\.json: \(root\): font\.face: is missing/
    ]
  ];

  it.each(fullBuildFailures)("fails on %s, and writes nothing", (name, edit, message) => {
    const dir = source(name.replace(/\W+/g, "-"), edit);
    const out = join(scratch, `out-${name.replace(/\W+/g, "-")}`);
    const r = build("--src", dir, "--out", out);
    expect(r.status, r.stderr).toBe(1);
    expect(r.stdout).toBe("");
    expect(r.stderr).toMatch(/nothing written/);
    expect(r.stderr).toMatch(message);
    expect(existsSync(out)).toBe(false);
    expect(build("--src", dir, "--check").status).toBe(1);
  });
});
