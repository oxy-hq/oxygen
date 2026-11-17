#!/usr/bin/env node
// The theme generator: DTCG token files in, theme CSS and native token files out.
//
//   node scripts/build-theme.mjs                 write every output
//   node scripts/build-theme.mjs --check         write nothing; exit 1 naming each output that differs
//   node scripts/build-theme.mjs --out DIR       write the outputs under DIR instead of the repo root
//                                                (same repo-relative layout); with --check, compare there
//   node scripts/build-theme.mjs --src DIR       read the inputs from DIR instead of src/theme (a copy
//                                                laid out the same way). It writes only with --out, so
//                                                a scratch source never rewrites the repo's outputs;
//                                                --check without --out compares against the repo's
//   node scripts/build-theme.mjs --emit-css DIR  print the CSS of one direction folder (the four token
//                                                files, or a tokens/ folder holding them) and nothing else
//
// --emit-css is the flattener alone, the counterpart of the design validator's --emit-css: it takes
// any direction folder, the x0 fixture included, and checks that folder's structure against the
// contract. The direction list and the fonts belong to the full build, which checks them.
//
// Inputs, all under src/theme/ (or --src) and nothing else: contract/contract.json (the names every
// direction fills), tokens/<id>/{primitives,base,light,dark}.tokens.json (Design Tokens Community
// Group format 2025.10, copied verbatim from the design work), tokens/<id>/fonts.json (the faces,
// the identity carrier and a one-line description), and fonts/ (unmodified woff2 files and their
// OFL licences).
//
// Outputs: src/theme/theme.css and themes.gen.ts; mobile/design/theme/OxyTheme.swift, OxyTheme.kt and
// README.md; and a byte-identical copy for the web-app, which builds without the SDK's sources
// (web-app/src/styles/shadcn/sdk-theme.css). When a direction self-hosts faces (fonts.json), also
// src/theme/theme-fonts.css and the web-app's theme-fonts.css and fonts/; with none (the reference
// uses the system face) those are not written, and --check names any left behind. Plain Node, no
// dependencies, deterministic: the same inputs give the same bytes.
//
// What is checked here is structure (where a token lives, that it has its contract name and type,
// that its aliases resolve) and value syntax. Colour maths (hex against components, gamut, contrast)
// is the design validator's job and src/theme/contrast.test.ts's, which reads what this writes.
import {
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync
} from "node:fs";
import { basename, dirname, join, relative, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const PKG = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const REPO = resolve(PKG, "..", "..");
/** Where the inputs are read from: src/theme, or the --src folder (set once, by main). */
let SRC = join(PKG, "src", "theme");

/**
 * The directions theme.css carries, in the order it emits them. The first is the shipped theme:
 * importing theme.css applies it, with no attribute. Any later one applies under
 * <html data-oxy-theme="<id>">. One today: the oxygen reference app's design (oxy-hq/reference-app).
 */
const DIRECTIONS = ["reference"];
const CONTRACT_VERSION = 1;
const TOKEN_FILES = ["primitives", "base", "light", "dark"];
const MODES = ["light", "dark"];

const OUT = {
  css: "sdk/typescript/src/theme/theme.css",
  fontsCss: "sdk/typescript/src/theme/theme-fonts.css",
  themesTs: "sdk/typescript/src/theme/themes.gen.ts",
  swift: "mobile/design/theme/OxyTheme.swift",
  kotlin: "mobile/design/theme/OxyTheme.kt",
  readme: "mobile/design/theme/README.md",
  webCss: "web-app/src/styles/shadcn/sdk-theme.css",
  webFontsCss: "web-app/src/styles/shadcn/theme-fonts.css",
  webFonts: "web-app/src/styles/shadcn/fonts"
};

const DTCG_TYPES = new Set([
  "color",
  "dimension",
  "fontFamily",
  "fontWeight",
  "duration",
  "cubicBezier",
  "number",
  "strokeStyle",
  "border",
  "transition",
  "shadow",
  "gradient",
  "typography"
]);
const GROUP_PROPS = new Set(["$type", "$description", "$extensions", "$deprecated"]);
const TOKEN_PROPS = new Set(["$value", "$type", "$description", "$extensions", "$deprecated"]);
const NAME_OK = /^[a-z0-9][a-z0-9-]*$/;
const ID_OK = /^[a-z][a-z0-9-]*$/;
const CSS_NAME_OK = /^--[a-z][a-z0-9-]*$/;
const REF = /^\{([^{}]+)\}$/;

/** Contract type -> [DTCG $type, required $extensions["com.oxygen"].cssUnit]. */
const CONTRACT_TYPES = {
  color: ["color", null],
  length: ["dimension", null],
  number: ["number", null],
  em: ["number", "em"],
  percent: ["number", "%"],
  "font-family": ["fontFamily", null],
  "font-weight": ["fontWeight", null],
  shadow: ["shadow", null],
  // A colour written as the reference's select chevron stroked in it (`chevronCss`): a data URI
  // cannot take currentColor, so each mode carries its own image.
  chevron: ["color", null]
};
const FONT_WEIGHTS = {
  thin: 100,
  hairline: 100,
  "extra-light": 200,
  "ultra-light": 200,
  light: 300,
  normal: 400,
  regular: 400,
  book: 400,
  medium: 500,
  "semi-bold": 600,
  "demi-bold": 600,
  bold: 700,
  "extra-bold": 800,
  "ultra-bold": 800,
  black: 900,
  heavy: 900,
  "extra-black": 950,
  "ultra-black": 950
};
/** Written bare in CSS; every other family name is double-quoted. */
const GENERIC_FAMILIES = new Set([
  "system-ui",
  "sans-serif",
  "serif",
  "monospace",
  "ui-monospace",
  "ui-sans-serif",
  "ui-serif",
  "-apple-system",
  "BlinkMacSystemFont",
  "cursive",
  "fantasy",
  "math"
]);
/** A family name that can sit inside "…" in CSS and in a Swift or Kotlin string. */
function isPlainFamily(s) {
  if (typeof s !== "string" || !s || s.trim() !== s || /["'\\;{}<>*$]/.test(s)) return false;
  for (let i = 0; i < s.length; i += 1) {
    const code = s.charCodeAt(i);
    if (code < 32 || code === 127) return false;
  }
  return true;
}

// ---------------------------------------------------------------------------------------------
// Problems: every rule names the file, the token path and the rule id
// ---------------------------------------------------------------------------------------------

class Problems {
  constructor() {
    this.list = [];
  }
  add(file, path, rule, message) {
    this.list.push(`${file}: ${path || "(root)"}: ${rule}: ${message}`);
  }
  get size() {
    return this.list.length;
  }
}

class ResolveError extends Error {
  constructor(rule, message) {
    super(message);
    this.rule = rule;
  }
}

const isObject = (v) => typeof v === "object" && v !== null && !Array.isArray(v);
const isNumber = (v) => typeof v === "number" && Number.isFinite(v);
const show = (v) => {
  const s = JSON.stringify(v);
  return s === undefined ? String(v) : s.length > 60 ? `${s.slice(0, 57)}...` : s;
};
/** A path for messages: repo-relative inside the repo, absolute outside it (a --src copy). */
const repoPath = (abs) => {
  const rel = relative(REPO, abs).split("\\").join("/");
  return rel === ".." || rel.startsWith("../") ? abs.split("\\").join("/") : rel;
};

function readJson(file, problems, rule = "files.load") {
  let text;
  try {
    text = readFileSync(file, "utf8");
  } catch {
    problems.add(repoPath(file), "", rule, "is missing");
    return undefined;
  }
  try {
    return JSON.parse(text);
  } catch (err) {
    problems.add(repoPath(file), "", rule, `is not valid JSON (${err.message})`);
    return undefined;
  }
}

/** DTCG path to CSS custom property: drop every "$root", drop a leading "color", join with "-". */
function flatten(path) {
  const segs = path.filter((s) => s !== "$root");
  if (segs[0] === "color") segs.shift();
  return `--${segs.join("-")}`;
}

// ---------------------------------------------------------------------------------------------
// Numbers, written as the design validator writes them (Python's format(x, "g") and round(x, 3)):
// six significant digits, ties to even on the exact binary value, no trailing zeros
// ---------------------------------------------------------------------------------------------

/** |x| as int / 10^scale, exactly. */
function exactParts(x) {
  const view = new DataView(new ArrayBuffer(8));
  view.setFloat64(0, Math.abs(x));
  const bits = view.getBigUint64(0);
  const biased = Number((bits >> 52n) & 0x7ffn);
  const fraction = bits & 0xfffffffffffffn;
  const mantissa = biased === 0 ? fraction : fraction | 0x10000000000000n;
  const exp2 = (biased === 0 ? 1 : biased) - 1075;
  if (exp2 >= 0) return { int: mantissa << BigInt(exp2), scale: 0 };
  return { int: mantissa * 5n ** BigInt(-exp2), scale: -exp2 };
}

function formatNumber(x) {
  if (!Number.isFinite(x)) throw new Error(`not a finite number: ${x}`);
  if (x === 0) return Object.is(x, -0) ? "-0" : "0";
  const { int, scale } = exactParts(x);
  let digits = int.toString();
  let exp = digits.length - 1 - scale;
  if (digits.length > 6) {
    const head = BigInt(digits.slice(0, 6));
    const rest = digits.slice(6);
    const half = `5${"0".repeat(rest.length - 1)}`;
    const up = rest > half || (rest === half && head % 2n === 1n);
    digits = (up ? head + 1n : head).toString();
    if (digits.length > 6) {
      digits = digits.slice(0, 6);
      exp += 1;
    }
  }
  digits = digits.replace(/0+$/, "") || "0";
  const sign = x < 0 ? "-" : "";
  if (exp < -4 || exp >= 6) {
    const mantissa = digits.length > 1 ? `${digits[0]}.${digits.slice(1)}` : digits;
    return `${sign}${mantissa}e${exp < 0 ? "-" : "+"}${String(Math.abs(exp)).padStart(2, "0")}`;
  }
  if (exp < 0) return `${sign}0.${"0".repeat(-exp - 1)}${digits}`;
  const whole = digits.slice(0, exp + 1).padEnd(exp + 1, "0");
  const frac = digits.slice(exp + 1);
  return `${sign}${whole}${frac ? `.${frac}` : ""}`;
}

function roundTo(x, places) {
  const { int, scale } = exactParts(x);
  if (scale <= places) return x;
  const div = 10n ** BigInt(scale - places);
  let q = int / div;
  const twice = (int % div) * 2n;
  if (twice > div || (twice === div && q % 2n === 1n)) q += 1n;
  const out = Number(q) / 10 ** places;
  // Python keeps the sign: round(-0.0004, 3) and round(-0.0, 3) are both -0.0, written "-0".
  return x < 0 || Object.is(x, -0) ? -out : out;
}

// ---------------------------------------------------------------------------------------------
// The contract
// ---------------------------------------------------------------------------------------------

function loadContract(file, problems) {
  const where = repoPath(file);
  const raw = readJson(file, problems, "contract.parse");
  if (!isObject(raw) || !Array.isArray(raw.tokens)) {
    if (raw !== undefined)
      problems.add(where, "tokens", "contract.parse", "must be an object with a tokens array");
    return undefined;
  }
  if (raw.version !== CONTRACT_VERSION) {
    problems.add(
      where,
      "version",
      "contract.version",
      `this generator reads contract ${CONTRACT_VERSION}, found ${show(raw.version)}`
    );
  }
  const contract = {
    version: raw.version,
    authored: [],
    aliases: [],
    fixed: [],
    byName: new Map()
  };
  for (const [i, row] of raw.tokens.entries()) {
    const at = `tokens[${i}]`;
    if (!isObject(row) || typeof row.name !== "string" || !CSS_NAME_OK.test(row.name)) {
      problems.add(
        where,
        at,
        "contract.parse",
        `name must match ${CSS_NAME_OK} (got ${show(row?.name)})`
      );
      continue;
    }
    if (contract.byName.has(row.name))
      problems.add(where, at, "contract.parse", `${row.name} appears twice`);
    contract.byName.set(row.name, row);
    if (row.type === "alias") {
      contract.aliases.push(row);
    } else if (row.type === "fixed") {
      contract.fixed.push(row);
    } else if (Object.hasOwn(CONTRACT_TYPES, row.type)) {
      if (row.modes !== "per-mode" && row.modes !== "both") {
        problems.add(
          where,
          at,
          "contract.parse",
          `${row.name}: modes must be per-mode or both (got ${show(row.modes)})`
        );
      }
      const flat = typeof row.path === "string" ? flatten(row.path.split(".")) : undefined;
      if (flat !== row.name) {
        problems.add(
          where,
          at,
          "contract.parse",
          `${row.name}: its path ${show(row.path)} flattens to ${flat}`
        );
      }
      contract.authored.push(row);
    } else {
      problems.add(
        where,
        at,
        "contract.parse",
        `${row.name}: unknown contract type ${show(row.type)}`
      );
    }
  }
  checkContractAliases(contract, where, problems);
  const counts = {
    authored: contract.authored.length,
    perMode: contract.authored.filter((t) => t.modes === "per-mode").length,
    both: contract.authored.filter((t) => t.modes === "both").length,
    aliases: contract.aliases.length,
    fixed: contract.fixed.length
  };
  for (const [key, n] of Object.entries(counts)) {
    if (raw.counts?.[key] !== n) {
      problems.add(
        where,
        `counts.${key}`,
        "contract.counts",
        `says ${show(raw.counts?.[key])}, the rows give ${n}`
      );
    }
  }
  return contract;
}

function checkContractAliases(contract, where, problems) {
  for (const row of contract.aliases) {
    let target = row.target;
    const seen = new Set([row.name]);
    while (contract.byName.get(target)?.type === "alias" && !seen.has(target)) {
      seen.add(target);
      target = contract.byName.get(target).target;
    }
    const end = contract.byName.get(target);
    if (!end || end.type === "alias" || end.type === "fixed") {
      problems.add(
        where,
        row.name,
        "contract.parse",
        `alias ${row.name} -> ${show(row.target)} does not end at an authored token`
      );
    }
  }
}

// ---------------------------------------------------------------------------------------------
// One direction: load, place, cover, resolve, serialise
// ---------------------------------------------------------------------------------------------

function walk(node, path, inherited, file, out, problems) {
  let type = inherited;
  if (Object.hasOwn(node, "$type")) {
    type = node.$type;
    if (!DTCG_TYPES.has(type))
      problems.add(file, path.join("."), "names.syntax", `unknown $type ${show(type)}`);
  }
  for (const [key, child] of Object.entries(node)) {
    if (key.startsWith("$") && key !== "$root") {
      if (!GROUP_PROPS.has(key))
        problems.add(file, path.join("."), "names.syntax", `unknown group property ${key}`);
      continue;
    }
    const childPath = [...path, key];
    const at = childPath.join(".");
    if (key !== "$root" && !NAME_OK.test(key)) {
      problems.add(
        file,
        at,
        "names.syntax",
        `name ${show(key)} breaks the DTCG name rules or the alphabet [a-z0-9-]`
      );
      continue;
    }
    if (!isObject(child)) {
      problems.add(file, at, "names.syntax", "is neither a token nor a group (not an object)");
      continue;
    }
    if (!Object.hasOwn(child, "$value")) {
      walk(child, childPath, type, file, out, problems);
      continue;
    }
    const extra = Object.keys(child).filter((k) => !TOKEN_PROPS.has(k));
    if (extra.length)
      problems.add(file, at, "names.syntax", `unknown token properties ${extra.join(", ")}`);
    const own = Object.hasOwn(child, "$type") ? child.$type : type;
    if (own != null && !DTCG_TYPES.has(own))
      problems.add(file, at, "names.syntax", `unknown $type ${show(own)}`);
    out.set(at, {
      key: at,
      path: childPath,
      type: own ?? null,
      value: child.$value,
      ext: child.$extensions,
      file
    });
  }
}

/** The folder that holds the four token files: `dir` itself, or `dir/tokens`. */
function tokenFolder(dir) {
  return existsSync(join(dir, "tokens", "base.tokens.json")) ? join(dir, "tokens") : dir;
}

/** The direction's own folder name: the token folder, or its parent when that is named "tokens". */
function directionFolderName(tokenDir) {
  return basename(tokenDir) === "tokens" ? basename(dirname(tokenDir)) : basename(tokenDir);
}

function loadDirection(dir, contract, problems) {
  const tdir = tokenFolder(dir);
  const files = {};
  const tokens = {};
  for (const f of TOKEN_FILES) {
    const file = join(tdir, `${f}.tokens.json`);
    const data = readJson(file, problems);
    if (data === undefined) continue;
    if (!isObject(data)) {
      problems.add(repoPath(file), "", "files.load", "the root must be an object");
      continue;
    }
    files[f] = repoPath(file);
    tokens[f] = new Map();
    walk(data, [], null, files[f], tokens[f], problems);
    if (f === "base") tokens.meta = data.$extensions?.["com.oxygen"];
  }
  if (TOKEN_FILES.some((f) => !tokens[f])) return undefined;
  const d = {
    files,
    tokens,
    folder: directionFolderName(tdir),
    id: undefined,
    name: undefined,
    views: {},
    css: {},
    data: {}
  };
  checkMeta(d, contract, problems);
  checkPlacement(d, contract, problems);
  checkPrimitives(d, problems);
  for (const mode of MODES) buildView(d, mode, contract, problems);
  for (const mode of MODES) resolveMode(d, mode, contract, problems);
  return d;
}

/** Every palette entry is a literal colour object, used or not. */
function checkPrimitives(d, problems) {
  for (const t of d.tokens.primitives.values()) {
    if (t.path[0] !== "palette") continue;
    if (t.type !== "color") {
      problems.add(
        t.file,
        t.key,
        "color.syntax",
        `palette entries are colours (got $type ${show(t.type)})`
      );
    } else if (typeof t.value === "string") {
      problems.add(
        t.file,
        t.key,
        "color.syntax",
        `palette entries are literal colours, not aliases (${show(t.value)})`
      );
    } else {
      try {
        parseColor(t.value);
      } catch (err) {
        problems.add(t.file, t.key, err.rule, err.message);
      }
    }
  }
}

function checkMeta(d, contract, problems) {
  const meta = d.tokens.meta;
  const at = "$extensions.com.oxygen";
  if (!isObject(meta)) {
    problems.add(
      d.files.base,
      at,
      "files.meta",
      "base.tokens.json needs {direction, name, contract} here"
    );
    return;
  }
  if (typeof meta.direction !== "string" || !ID_OK.test(meta.direction)) {
    problems.add(
      d.files.base,
      `${at}.direction`,
      "files.meta",
      `must match ${ID_OK} (got ${show(meta.direction)})`
    );
  } else if (meta.direction !== d.folder) {
    problems.add(
      d.files.base,
      `${at}.direction`,
      "files.meta",
      `is ${show(meta.direction)} but the folder is ${show(d.folder)}`
    );
  }
  if (typeof meta.name !== "string" || !meta.name.trim() || /\*\/|[\r\n]/.test(meta.name)) {
    problems.add(
      d.files.base,
      `${at}.name`,
      "files.meta",
      `must be a non-empty one-line string without */ (got ${show(meta.name)})`
    );
  }
  if (meta.contract !== contract.version) {
    problems.add(
      d.files.base,
      `${at}.contract`,
      "contract.version",
      `declares contract ${show(meta.contract)}; contract.json is version ${contract.version}`
    );
  }
  d.id = meta.direction;
  d.name = meta.name;
}

function checkPlacement(d, contract, problems) {
  for (const f of TOKEN_FILES) {
    for (const t of d.tokens[f].values()) {
      const file = d.files[f];
      if (f === "primitives") {
        if (t.path[0] !== "palette")
          problems.add(file, t.key, "files.placement", "primitives hold only palette.* tokens");
        continue;
      }
      if (t.path[0] === "palette") {
        problems.add(file, t.key, "files.placement", "palette.* belongs in primitives.tokens.json");
        continue;
      }
      const name = flatten(t.path);
      const spec = contract.byName.get(name);
      if (!spec && name.startsWith("--oxy-")) {
        problems.add(
          file,
          t.key,
          "contract.fixed-override",
          `${name} is owned outside direction files (motion, the contract marker)`
        );
      } else if (!spec) {
        problems.add(
          file,
          t.key,
          "contract.extra",
          `${name} is not a contract name; propose it in the contract first`
        );
      } else if (spec.type === "alias") {
        problems.add(
          file,
          t.key,
          "contract.derived-authored",
          `${name} is generated as var(${spec.target}); a direction may not author it`
        );
      } else if (spec.type === "fixed") {
        problems.add(
          file,
          t.key,
          "contract.fixed-override",
          `${name} is owned by ${spec.owner}; a direction may not set it`
        );
      } else if (spec.modes === "both" && f !== "base") {
        problems.add(
          file,
          t.key,
          "files.placement",
          `${name} is mode-independent (both); it belongs in base.tokens.json`
        );
      } else if (spec.modes === "per-mode" && f === "base") {
        problems.add(
          file,
          t.key,
          "files.placement",
          `${name} is per-mode; it belongs in light.tokens.json and dark.tokens.json`
        );
      }
    }
  }
}

/** Each mode sees primitives + base + that mode's file; a path may be defined once in that view. */
function buildView(d, mode, contract, problems) {
  const view = new Map();
  const names = new Map();
  for (const f of ["primitives", "base", mode]) {
    for (const t of d.tokens[f].values()) {
      // A clash between primitives and base shows in both views: report it once, from light.
      const prior = view.get(t.key);
      if (prior && (mode === "light" || f === mode)) {
        problems.add(
          t.file,
          t.key,
          "files.placement",
          `is also defined in ${basename(prior.file)}`
        );
      }
      view.set(t.key, t);
      if (f === "primitives") continue;
      const name = flatten(t.path);
      const other = names.get(name);
      if (other && other.key !== t.key && (mode === "light" || f === mode)) {
        problems.add(
          t.file,
          t.key,
          "names.unique",
          `${other.key} (${basename(other.file)}) also flattens to ${name}`
        );
      }
      names.set(name, t);
    }
  }
  for (const spec of contract.authored) {
    if (!names.has(spec.name)) {
      const file = spec.modes === "both" ? d.files.base : d.files[mode];
      problems.add(
        file,
        spec.path,
        "contract.missing",
        `${spec.name} (${spec.type}, ${spec.modes}) is not defined`
      );
    }
  }
  d.views[mode] = { tokens: view, names };
}

function resolveValue(value, expect, view, stack, refs) {
  if (typeof value === "string") {
    const m = REF.exec(value);
    if (!m) return { value, type: expect };
    const key = m[1];
    if (stack.includes(key))
      throw new ResolveError("alias.cycle", `cycle ${[...stack, key].join(" -> ")}`);
    const target = view.get(key);
    if (!target)
      throw new ResolveError("alias.unresolved", `${value} is not a token (missing, or a group)`);
    if (expect && target.type && target.type !== expect) {
      throw new ResolveError(
        "alias.type",
        `a ${expect} token references ${value}, which is a ${target.type}`
      );
    }
    refs.push(target);
    return resolveValue(target.value, expect ?? target.type, view, [...stack, key], refs);
  }
  if (Array.isArray(value) && expect === "shadow") {
    const layers = [];
    for (const v of value) {
      const r = resolveValue(v, "shadow", view, stack, refs);
      if (Array.isArray(r.value)) layers.push(...r.value);
      else layers.push(r.value);
    }
    return { value: layers, type: "shadow" };
  }
  if (isObject(value) && expect === "shadow") {
    const sub = {
      color: "color",
      offsetX: "dimension",
      offsetY: "dimension",
      blur: "dimension",
      spread: "dimension"
    };
    const out = {};
    for (const [k, v] of Object.entries(value)) {
      out[k] = sub[k] ? resolveValue(v, sub[k], view, stack, refs).value : v;
    }
    return { value: out, type: "shadow" };
  }
  return { value, type: expect };
}

function resolveMode(d, mode, contract, problems) {
  const { tokens: view, names } = d.views[mode];
  const css = new Map();
  const data = new Map();
  for (const spec of contract.authored) {
    const t = names.get(spec.name);
    if (!t) continue;
    if (mode === "dark" && spec.modes === "both") {
      if (d.css.light?.has(spec.name)) {
        css.set(spec.name, d.css.light.get(spec.name));
        data.set(spec.name, d.data.light.get(spec.name));
      }
      continue;
    }
    const out = resolveToken(t, spec, view, d, problems);
    if (out) {
      css.set(spec.name, out.css);
      data.set(spec.name, out.data);
    }
  }
  d.css[mode] = css;
  d.data[mode] = data;
}

function resolveToken(t, spec, view, d, problems) {
  const [want, unit] = CONTRACT_TYPES[spec.type];
  const cssUnit =
    isObject(t.ext) && isObject(t.ext["com.oxygen"]) ? (t.ext["com.oxygen"].cssUnit ?? null) : null;
  if (cssUnit !== unit) {
    problems.add(
      t.file,
      t.key,
      "number.unit",
      `${spec.name} must carry $extensions["com.oxygen"].cssUnit = ${show(unit)} (has ${show(cssUnit)})`
    );
    return undefined;
  }
  let resolved;
  const refs = [];
  try {
    resolved = resolveValue(t.value, t.type, view, [t.key], refs);
  } catch (err) {
    if (!(err instanceof ResolveError)) throw err;
    problems.add(t.file, t.key, err.rule, err.message);
    return undefined;
  }
  if (resolved.type !== want) {
    problems.add(
      t.file,
      t.key,
      "contract.type",
      `${spec.name} must be DTCG ${want} (contract type ${spec.type}); it is ${show(resolved.type)}`
    );
    return undefined;
  }
  if (t.file === d.files.base) {
    const perMode = refs.find((r) => r.file === d.files.light || r.file === d.files.dark);
    if (perMode) {
      problems.add(
        t.file,
        t.key,
        "files.placement",
        `${spec.name} aliases ${perMode.key}, a per-mode token, so it is not mode-independent`
      );
      return undefined;
    }
  }
  try {
    return serialise(spec, resolved.value);
  } catch (err) {
    problems.add(t.file, t.key, err.rule ?? "value.syntax", err.message);
    return undefined;
  }
}

// ---------------------------------------------------------------------------------------------
// Values: parse the resolved DTCG value, write CSS text and keep the numbers for the native files
// ---------------------------------------------------------------------------------------------

function fail(rule, message) {
  const err = new Error(message);
  err.rule = rule;
  return err;
}

function parseColor(v) {
  if (!isObject(v))
    throw fail(
      "color.syntax",
      `colour $value must be an object with colorSpace, components and hex (got ${show(v)})`
    );
  const extra = Object.keys(v).filter(
    (k) => !["colorSpace", "components", "alpha", "hex"].includes(k)
  );
  if (extra.length) throw fail("color.syntax", `unknown colour properties ${extra.join(", ")}`);
  const { colorSpace: cs, components: c, hex } = v;
  const alpha = v.alpha ?? 1;
  if (cs !== "srgb" && cs !== "oklch")
    throw fail("color.syntax", `colorSpace ${show(cs)} is not srgb or oklch`);
  if (!Array.isArray(c) || c.length !== 3 || !c.every(isNumber))
    throw fail("color.syntax", `components must be three numbers (got ${show(c)})`);
  const inRange =
    cs === "srgb"
      ? c.every((x) => x >= 0 && x <= 1)
      : c[0] >= 0 && c[0] <= 1 && c[1] >= 0 && c[2] >= 0 && c[2] < 360;
  if (!inRange) throw fail("color.syntax", `${cs} components out of range: ${show(c)}`);
  if (!isNumber(alpha) || alpha < 0 || alpha > 1)
    throw fail("color.syntax", `alpha must be a number in 0..1 (got ${show(v.alpha)})`);
  if (typeof hex !== "string" || !/^#[0-9a-fA-F]{6}$/.test(hex))
    throw fail("color.syntax", `hex must be 6-digit #rrggbb (got ${show(hex)})`);
  const n = Number.parseInt(hex.slice(1), 16);
  return { hex: hex.toLowerCase(), r: (n >> 16) & 255, g: (n >> 8) & 255, b: n & 255, alpha };
}

/**
 * A colour as CSS: hex when opaque, else the legacy `rgba(r, g, b, a)`. Not `rgb(r g b / a)`: Chrome
 * paints the two differently (the legacy form's alpha is held to 8 bits), and the reference app and
 * the apps adopting the theme write `rgba()`, so the same token would land a channel apart from
 * the line it replaces (measured: rgba(0, 0, 0, 0.08) over #e6e6e8 paints 212,212,214; the
 * modern form 212,212,213).
 */
function colorCss(c) {
  return c.alpha >= 1
    ? c.hex
    : `rgba(${c.r}, ${c.g}, ${c.b}, ${formatNumber(roundTo(c.alpha, 3))})`;
}

function parseDimension(v) {
  const ok =
    isObject(v) &&
    Object.keys(v).every((k) => k === "value" || k === "unit") &&
    isNumber(v.value) &&
    (v.unit === "px" || v.unit === "rem");
  if (!ok)
    throw fail(
      "dimension.syntax",
      `dimension must be {value, unit} with unit px or rem (got ${show(v)})`
    );
  return { value: v.value, unit: v.unit, px: v.value * (v.unit === "rem" ? 16 : 1) };
}

function parseFamilies(v) {
  const list = typeof v === "string" ? [v] : v;
  if (!Array.isArray(list) || !list.length || !list.every((f) => typeof f === "string")) {
    throw fail("font.syntax", `fontFamily must be a name or a list of names (got ${show(v)})`);
  }
  for (const f of list) {
    if (!isPlainFamily(f)) {
      throw fail(
        "font.syntax",
        `family name ${show(f)} is empty or holds a character CSS or a native string cannot carry`
      );
    }
  }
  return list;
}

function familiesCss(list) {
  return list.map((f) => (GENERIC_FAMILIES.has(f) ? f : `"${f}"`)).join(", ");
}

function parseWeight(v) {
  if (isNumber(v) && v >= 1 && v <= 1000) return v;
  if (typeof v === "string" && Object.hasOwn(FONT_WEIGHTS, v)) return FONT_WEIGHTS[v];
  throw fail("font.syntax", `fontWeight must be 1..1000 or a DTCG keyword (got ${show(v)})`);
}

function parseShadow(v) {
  const layers = Array.isArray(v) ? v : [v];
  if (!layers.length)
    throw fail("shadow.syntax", "shadow needs a layer (one all-zero, alpha-0 layer means none)");
  return layers.map((lay) => {
    if (!isObject(lay))
      throw fail("shadow.syntax", `shadow layer must be an object (got ${show(lay)})`);
    const keys = ["color", "offsetX", "offsetY", "blur", "spread"];
    const missing = keys.filter((k) => !Object.hasOwn(lay, k));
    if (missing.length) throw fail("shadow.syntax", `shadow layer lacks ${missing.join(", ")}`);
    const extra = Object.keys(lay).filter((k) => !keys.includes(k) && k !== "inset");
    if (extra.length) throw fail("shadow.syntax", `unknown shadow properties ${extra.join(", ")}`);
    if (Object.hasOwn(lay, "inset") && typeof lay.inset !== "boolean")
      throw fail("shadow.syntax", "inset must be a boolean");
    const [x, y, blur, spread] = keys.slice(1).map((k) => parseDimension(lay[k]).px);
    if (blur < 0) throw fail("shadow.syntax", "blur must be >= 0");
    return { color: parseColor(lay.color), x, y, blur, spread, inset: lay.inset === true };
  });
}

function shadowCss(layers) {
  const shown = layers.filter(
    (l) => !(l.color.alpha === 0 && !l.x && !l.y && !l.blur && !l.spread)
  );
  if (!shown.length) return "none";
  const px = (n) => (n ? `${formatNumber(n)}px` : "0");
  return shown
    .map(
      (l) =>
        `${l.inset ? "inset " : ""}${px(l.x)} ${px(l.y)} ${px(l.blur)} ${px(l.spread)} ${colorCss(l.color)}`
    )
    .join(", ");
}

/**
 * The reference's select chevron (reference-app@7c1ee7e src/index.css:140, 558): a 12x8 down
 * stroke, 1.5px, round caps and joins, in `c`, as the url() a native <select> paints at its right
 * (`appearance: none`, `background-size: 12px 8px`). The SVG text is the reference's, byte for
 * byte, with the stroke's hex in place.
 */
function chevronCss(c) {
  return `url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='8' viewBox='0 0 12 8' fill='none'%3E%3Cpath d='M1 1.5 6 6.5 11 1.5' stroke='%23${c.hex.slice(1)}' stroke-width='1.5' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E")`;
}

function serialise(spec, value) {
  switch (spec.type) {
    case "color": {
      const c = parseColor(value);
      return { css: colorCss(c), data: c };
    }
    case "length": {
      const dim = parseDimension(value);
      return { css: `${formatNumber(dim.value)}${dim.unit}`, data: dim.px };
    }
    case "number":
    case "em":
    case "percent": {
      if (!isNumber(value)) throw fail("number.syntax", `number expected (got ${show(value)})`);
      const unit = spec.type === "em" ? "em" : spec.type === "percent" ? "%" : "";
      return { css: `${formatNumber(value)}${unit}`, data: value };
    }
    case "font-family": {
      const list = parseFamilies(value);
      return { css: familiesCss(list), data: list };
    }
    case "font-weight": {
      const w = parseWeight(value);
      return { css: formatNumber(w), data: w };
    }
    case "shadow": {
      const layers = parseShadow(value);
      return { css: shadowCss(layers), data: layers };
    }
    case "chevron": {
      const c = parseColor(value);
      if (c.alpha < 1) throw fail("color.syntax", "a chevron's stroke must be opaque");
      return { css: chevronCss(c), data: c };
    }
    default:
      throw fail("contract.type", `no serialiser for contract type ${spec.type}`);
  }
}

// ---------------------------------------------------------------------------------------------
// theme.css
// ---------------------------------------------------------------------------------------------

const WIDTH = 100;

/** Split a CSS value at its top-level ", " (outside parentheses and quotes). */
function topLevelItems(value) {
  const items = [];
  let depth = 0;
  let quoted = false;
  let start = 0;
  for (let i = 0; i < value.length; i += 1) {
    const ch = value[i];
    if (ch === '"') quoted = !quoted;
    else if (!quoted && ch === "(") depth += 1;
    else if (!quoted && ch === ")") depth -= 1;
    else if (!quoted && depth === 0 && ch === "," && value[i + 1] === " ") {
      items.push(value.slice(start, i));
      start = i + 2;
    }
  }
  items.push(value.slice(start));
  return items;
}

/**
 * One declaration, laid out the way the repo's formatter (Biome, 100 columns) lays it out, so the
 * hook that formats a commit leaves generated CSS byte-identical: a line that fits stays; a longer
 * comma list moves under the name, four spaces in, filled to 100 columns with each item's comma
 * counted and the final semicolon not.
 */
function cssDecl(name, value) {
  const line = `  ${name}: ${value};`;
  if (line.length <= WIDTH) return line;
  const items = topLevelItems(value);
  // One item the formatter cannot break (the chevron's url()) stays on its line, as Biome keeps it.
  if (items.length < 2) {
    if (/^url\("[^"]*"\)$/.test(value)) return line;
    throw new Error(`${name}: a ${line.length}-column value without a comma list to wrap`);
  }
  const lines = [];
  let cur = `    ${items[0]},`;
  for (let i = 1; i < items.length; i += 1) {
    const piece = i < items.length - 1 ? `${items[i]},` : items[i];
    if (cur.length + 1 + piece.length <= WIDTH) cur += ` ${piece}`;
    else {
      lines.push(cur);
      cur = `    ${piece}`;
    }
  }
  lines.push(`${cur};`);
  return `  ${name}:\n${lines.join("\n")}`;
}

/**
 * The light and dark selectors of one direction. The shipped theme (the first) is `:where(:root)`,
 * so importing theme.css applies it; any other keys on data-oxy-theme. Dark is the `.dark` class
 * (the SDK shell's own contract) or data-oxy-mode="dark". All of them sit inside `:where()`, so they
 * weigh nothing: an app's own `:root { --border: … }` or `.dark { … }` still wins over the theme in
 * either import order, while Tailwind's `@layer theme` (layered) still loses to it.
 */
function directionSelectors(d) {
  const root = d.id === DIRECTIONS[0] ? ":root" : `:root[data-oxy-theme="${d.id}"]`;
  return {
    light: `:where(${root})`,
    dark: `:where(${root}.dark, ${root}[data-oxy-mode="dark"])`
  };
}

function directionBlocks(d, contract) {
  const sel = directionSelectors(d);
  const both = contract.authored.filter((t) => t.modes === "both");
  const perMode = contract.authored.filter((t) => t.modes === "per-mode");
  const light = [
    "  color-scheme: light;",
    cssDecl("--oxy-theme-contract", String(contract.version)),
    ...both.map((t) => cssDecl(t.name, d.css.light.get(t.name))),
    ...perMode.map((t) => cssDecl(t.name, d.css.light.get(t.name)))
  ];
  const dark = [
    "  color-scheme: dark;",
    ...perMode.map((t) => cssDecl(t.name, d.css.dark.get(t.name)))
  ];
  return `${sel.light} {\n${light.join("\n")}\n}\n\n${sel.dark} {\n${dark.join("\n")}\n}\n`;
}

/**
 * The shared alias block: the old names (`--radius`, `--font-inter`, `--color-border`, …) as var()
 * of the theme's. It sits in `@layer theme`, Tailwind's own theme layer, so an app's declaration of
 * one of these names wins wherever it is made, its Tailwind `@theme` included (an unlayered alias
 * would beat `@theme` and take an app's `bg-card` or `border-border` away from it). An app that
 * never names one gets the theme's. The selector is the document element under its own name, one
 * selector apart from the shipped theme's `:where(:root)`, and weighs nothing either.
 */
const ALIAS_SELECTOR = ":where(html)";

function aliasBlock(contract) {
  const lines = contract.aliases.map((t) =>
    cssDecl(t.name, `var(${t.target})`).replace(/^/gm, "  ")
  );
  return `@layer theme {\n  ${ALIAS_SELECTOR} {\n${lines.join("\n")}\n  }\n}\n`;
}

function themeCss(directions, contract) {
  const header = [
    "/*",
    " * Oxygen theme: the oxygen reference app's design as CSS custom properties. Generated by",
    " * sdk/typescript/scripts/build-theme.mjs from src/theme/tokens and contract/contract.json",
    ` * (contract ${contract.version}). Do not edit: change the tokens, then run \`pnpm build:theme\` in sdk/typescript.`,
    " *",
    " * Importing it applies the shipped theme, the oxygen reference app's design (oxy-hq/reference-app):",
    " * a light block, which also holds the mode-independent tokens, and a dark block for the .dark",
    ' * class or data-oxy-mode="dark" on <html>. Values are literals; only the shared block at the end',
    " * uses var(). Motion is not here: motion.css owns it.",
    " *",
    " * Every selector is inside :where(), so it weighs nothing: an app's own token declarations win in",
    " * either import order, and Tailwind's @layer theme, being layered, loses to it. The old names at",
    " * the end are aliases inside @layer theme, so an app's own (its Tailwind @theme included) still",
    " * win there too. The SDK shell and phone kit read these names (their fallbacks are what an app",
    " * without this file has always seen).",
    " */"
  ].join("\n");
  const fontsImport = hasFaces(directions) ? '@import "./theme-fonts.css";\n\n' : "";
  const parts = [`${fontsImport}${header}\n`];
  for (const d of directions) {
    parts.push(
      `/* ${d.name} (${d.id}): ${d.fonts.description} */\n${directionBlocks(d, contract)}`
    );
  }
  parts.push(`/* Derived names, shared by every direction. */\n${aliasBlock(contract)}`);
  return parts.join("\n");
}

/** One direction folder's CSS, for --emit-css. */
function directionCss(d, contract) {
  return `/* Direction ${d.id}, contract ${contract.version}: build-theme.mjs --emit-css. */\n${directionBlocks(d, contract)}\n/* Derived names, shared by every direction. */\n${aliasBlock(contract)}`;
}

// ---------------------------------------------------------------------------------------------
// Fonts: tokens/<id>/fonts.json -> theme-fonts.css
// ---------------------------------------------------------------------------------------------

const FACE_KEYS = new Set([
  "family",
  "file",
  "weight",
  "style",
  "stretch",
  "unicodeRange",
  "licence"
]);

function loadFonts(id, problems) {
  const file = join(SRC, "tokens", id, "fonts.json");
  const where = repoPath(file);
  const raw = readJson(file, problems, "font.face");
  if (raw === undefined) return undefined;
  const fonts = { carrier: raw?.carrier, description: raw?.description, faces: [] };
  for (const key of ["carrier", "description"]) {
    if (typeof raw?.[key] !== "string" || !raw[key].trim() || /["\\\n]|\*\//.test(raw[key])) {
      problems.add(
        where,
        key,
        "font.face",
        "must be a non-empty one-line string without quotes, backslashes or */"
      );
    }
  }
  const extraTop = isObject(raw)
    ? Object.keys(raw).filter((k) => !["carrier", "description", "faces"].includes(k))
    : [];
  if (extraTop.length)
    problems.add(where, "", "font.face", `unknown properties ${extraTop.join(", ")}`);
  if (!Array.isArray(raw?.faces)) {
    problems.add(where, "faces", "font.face", "must be a list of faces (empty: the system face)");
    return fonts;
  }
  for (const [i, face] of raw.faces.entries()) {
    const checked = checkFace(face, `faces[${i}]`, where, problems);
    if (checked) fonts.faces.push(checked);
  }
  const licenceOf = new Map();
  for (const face of fonts.faces) {
    const prior = licenceOf.get(face.family);
    if (prior && prior !== face.licence) {
      problems.add(
        where,
        "faces",
        "font.face",
        `${face.family} names two licences (${prior}, ${face.licence})`
      );
    }
    licenceOf.set(face.family, face.licence);
  }
  return fonts;
}

function checkFace(face, at, where, problems) {
  const bad = (rule, msg) => {
    problems.add(where, at, rule, msg);
    return undefined;
  };
  if (!isObject(face)) return bad("font.face", "must be an object");
  const extra = Object.keys(face).filter((k) => !FACE_KEYS.has(k));
  if (extra.length) return bad("font.face", `unknown properties ${extra.join(", ")}`);
  if (!isPlainFamily(face.family)) {
    return bad("font.face", `family ${show(face.family)} is not a plain family name`);
  }
  if (typeof face.file !== "string" || !/^[A-Za-z0-9._-]+\.woff2$/.test(face.file)) {
    return bad("font.face", `file ${show(face.file)} must be a .woff2 file name`);
  }
  const fontFile = join(SRC, "fonts", face.file);
  if (!existsSync(fontFile)) return bad("font.face", `${repoPath(fontFile)} does not exist`);
  const weights = typeof face.weight === "string" ? face.weight.split(" ").map(Number) : [];
  const weightOk =
    /^\d{1,4}( \d{1,4})?$/.test(face.weight ?? "") &&
    weights.every((w) => w >= 1 && w <= 1000) &&
    weights[0] <= (weights[1] ?? weights[0]);
  if (!weightOk)
    return bad("font.face", `weight ${show(face.weight)} must be "N" or "MIN MAX" within 1..1000`);
  if (face.style !== "normal" && face.style !== "italic")
    return bad("font.face", `style ${show(face.style)} must be normal or italic`);
  if (face.stretch !== undefined && !/^\d{1,3}(\.\d+)?%( \d{1,3}(\.\d+)?%)?$/.test(face.stretch)) {
    return bad("font.face", `stretch ${show(face.stretch)} must be "N%" or "MIN% MAX%"`);
  }
  const range = /^U\+[0-9A-Fa-f?]{1,6}(-[0-9A-Fa-f]{1,6})?$/;
  if (
    face.unicodeRange !== undefined &&
    !(
      typeof face.unicodeRange === "string" &&
      face.unicodeRange.split(/,\s*/).every((r) => range.test(r))
    )
  ) {
    return bad("font.face", `unicodeRange ${show(face.unicodeRange)} must be a list of U+ ranges`);
  }
  if (typeof face.licence !== "string" || !/^OFL-[A-Za-z0-9]+\.txt$/.test(face.licence)) {
    return bad("font.face", `licence ${show(face.licence)} must name an OFL-<Family>.txt file`);
  }
  const licenceFile = join(SRC, "fonts", face.licence);
  if (!existsSync(licenceFile)) return bad("font.face", `${repoPath(licenceFile)} does not exist`);
  const copyright = readFileSync(licenceFile, "utf8").split(/\r?\n/)[0].trim();
  if (!copyright.startsWith("Copyright") || copyright.includes("*/")) {
    return bad("font.face", `${repoPath(licenceFile)} must open with its copyright line`);
  }
  return { ...face, copyright };
}

/** The leading family of every font token must be generic or have a face in fonts.json. */
function checkFontCoverage(d, contract, problems) {
  const families = new Set(d.fonts.faces.map((f) => f.family));
  for (const spec of contract.authored.filter((t) => t.type === "font-family")) {
    const list = d.data.light.get(spec.name);
    if (!list) continue;
    const lead = list[0];
    if (!GENERIC_FAMILIES.has(lead) && !families.has(lead)) {
      const t = d.views.light.names.get(spec.name);
      problems.add(
        t.file,
        t.key,
        "font.face-missing",
        `${spec.name} leads with ${show(lead)}, which tokens/${d.folder}/fonts.json does not serve`
      );
    }
  }
}

function fontFaceCss(face) {
  const lines = [
    `  font-family: "${face.family}";`,
    `  src: url("./fonts/${face.file}") format("woff2");`,
    `  font-weight: ${face.weight};`,
    `  font-style: ${face.style};`
  ];
  if (face.stretch) lines.push(`  font-stretch: ${face.stretch};`);
  if (face.unicodeRange) lines.push(`  unicode-range: ${face.unicodeRange};`);
  lines.push("  font-display: swap;");
  return `@font-face {\n${lines.join("\n")}\n}\n`;
}

function themeFontsCss(directions) {
  const faces = directions.flatMap((d) => d.fonts.faces);
  const legal = new Map();
  for (const face of faces) {
    if (!legal.has(face.family))
      legal.set(
        face.family,
        `/*! ${face.family}: ${face.copyright}. SIL Open Font License 1.1, fonts/${face.licence} */`
      );
  }
  const header = [
    "/*",
    " * Oxygen theme fonts: every direction's faces. Generated by sdk/typescript/scripts/build-theme.mjs",
    " * from src/theme/tokens/<id>/fonts.json. Do not edit.",
    " *",
    " * A browser downloads a face only when something on the page uses its family, so declaring",
    " * every direction costs nothing until one is applied. The files are unmodified OFL fonts; each",
    " * family's licence sits beside it in fonts/.",
    " *",
    " * The licence comments come first on purpose: Vite's CSS minifier keeps a legal (bang) comment",
    " * only at the very start of a bundle, so they survive when theme.css is imported first.",
    " */"
  ].join("\n");
  return [`${[...legal.values()].join("\n")}\n${header}\n`, ...faces.map(fontFaceCss)].join("\n");
}

// ---------------------------------------------------------------------------------------------
// themes.gen.ts
// ---------------------------------------------------------------------------------------------

function themesTs(directions) {
  const entries = directions.map((d) =>
    [
      "  {",
      `    id: ${JSON.stringify(d.id)},`,
      `    name: ${JSON.stringify(d.name)},`,
      `    carrier: ${JSON.stringify(d.fonts.carrier)},`,
      `    description:\n      ${JSON.stringify(d.fonts.description)}`,
      "  }"
    ].join("\n")
  );
  return [
    "// Generated by scripts/build-theme.mjs from src/theme/tokens/<id>/base.tokens.json (id, name) and",
    "// fonts.json (carrier, description). Do not edit: run `pnpm build:theme`.",
    "",
    "/** The design directions theme.css carries, in the order it emits them. */",
    `export const THEMES = [\n${entries.join(",\n")}\n] as const;`,
    ""
  ].join("\n");
}

// ---------------------------------------------------------------------------------------------
// Native: SwiftUI and Compose token files (not wired into the apps yet)
// ---------------------------------------------------------------------------------------------

const SWIFT_KEYWORDS = new Set(
  "associatedtype class deinit enum extension fileprivate func import init inout internal let open operator private precedencegroup protocol public rethrows static struct subscript typealias var break case catch continue default defer do else fallthrough for guard if in repeat return throw switch where while as false is nil self super throws true try".split(
    " "
  )
);
const KOTLIN_KEYWORDS = new Set(
  "as break class continue do else false for fun if in interface is null object package return super this throw true try typealias typeof val var when while".split(
    " "
  )
);

const camel = (s) => s.replace(/^--/, "").replace(/-([a-z0-9])/g, (_, c) => c.toUpperCase());
const swiftId = (s) => (SWIFT_KEYWORDS.has(s) ? `\`${s}\`` : s);
const kotlinId = (s) => (KOTLIN_KEYWORDS.has(s) ? `\`${s}\`` : s);
/** A decimal literal for Swift and Kotlin: at most four decimals, always with a fraction part. */
const decimal = (x) => {
  const s = String(Number(x.toFixed(4)));
  return s.includes(".") || s.includes("e") ? s : `${s}.0`;
};
const quoteNative = (s) => JSON.stringify(s).replace(/\$/g, "\\$");

/** Which contract tokens each native group carries, picked by contract group and type. */
function nativeGroups(contract) {
  const a = contract.authored;
  const roles = [];
  for (const t of a.filter((x) => x.group === "Type roles")) {
    const role = t.name.replace(/^--type-/, "").replace(/-(family|size|line|weight|tracking)$/, "");
    if (!roles.includes(role)) roles.push(role);
  }
  return {
    colors: a.filter((t) => t.type === "color"),
    radius: a.filter(
      (t) => t.type === "length" && (t.group === "Shape" || t.name.endsWith("-radius"))
    ),
    space: a.filter(
      (t) => t.type === "length" && (t.group === "Space and layout" || t.name === "--card-padding")
    ),
    controlHeight: a.filter((t) => t.group === "Density"),
    roles,
    tile: a.filter((t) => t.group === "Module-icon tile" && t.type !== "color")
  };
}

function leadFamily(list) {
  return GENERIC_FAMILIES.has(list[0]) ? null : list[0];
}

function firstShadow(layers) {
  return layers.find((l) => !(l.color.alpha === 0 && !l.x && !l.y && !l.blur && !l.spread)) ?? null;
}

const hex6 = (c) => `0x${c.hex.slice(1).toUpperCase()}`;

function swiftColor(c) {
  return c.alpha >= 1 ? `oxyRGB(${hex6(c)})` : `oxyRGB(${hex6(c)}, ${decimal(c.alpha)})`;
}

function swiftShadow(layers) {
  const l = firstShadow(layers);
  if (!l) return "nil";
  return `OxyThemeShadow(color: ${swiftColor(l.color)}, radius: ${decimal(l.blur / 2)}, x: ${decimal(l.x)}, y: ${decimal(l.y)})`;
}

function swiftTileValue(d, spec) {
  const one = (mode) => {
    const v = d.data[mode].get(spec.name);
    return spec.type === "shadow" ? swiftShadow(v) : decimal(v);
  };
  return spec.modes === "per-mode"
    ? `OxyThemeByMode(light: ${one("light")}, dark: ${one("dark")})`
    : one("light");
}

function swiftDirection(d, g) {
  const L = d.data.light;
  const D = d.data.dark;
  const arg = (indent, list) => list.join(`,\n${indent}`);
  const colors = g.colors.map(
    (t) =>
      `${swiftId(camel(t.name))}: oxyThemeColor(light: ${swiftColor(L.get(t.name))}, dark: ${swiftColor(D.get(t.name))})`
  );
  const lengths = (list) => list.map((t) => `${swiftId(camel(t.name))}: ${decimal(L.get(t.name))}`);
  const roles = g.roles.map((r) => {
    const v = (p) => L.get(`--type-${r}-${p}`);
    const family = leadFamily(v("family"));
    return `${swiftId(camel(r))}: OxyThemeTypeRole(family: ${family ? quoteNative(family) : "nil"}, size: ${decimal(v("size"))}, lineHeight: ${decimal(v("line"))}, weight: ${v("weight")}, tracking: ${decimal(v("tracking"))})`;
  });
  const tile = g.tile.map((t) => `${swiftId(camel(t.name))}: ${swiftTileValue(d, t)}`);
  const i3 = "            ";
  return `    static let ${swiftId(camel(d.id))} = OxyThemeDirection(
        id: ${quoteNative(d.id)},
        name: ${quoteNative(d.name)},
        colors: OxyThemeColors(
            ${arg(i3, colors)}
        ),
        radius: OxyThemeRadius(
            ${arg(i3, lengths(g.radius))}
        ),
        space: OxyThemeSpace(
            ${arg(i3, lengths(g.space))}
        ),
        controlHeight: OxyThemeControlHeight(
            ${arg(i3, lengths(g.controlHeight))}
        ),
        type: OxyThemeTypeRoles(
            ${arg(i3, roles)}
        ),
        tile: OxyThemeTile(
            ${arg(i3, tile)}
        )
    )`;
}

function swiftTileType(spec) {
  const base = spec.type === "shadow" ? "OxyThemeShadow?" : "Double";
  return spec.modes === "per-mode" ? `OxyThemeByMode<${base}>` : base;
}

function swiftStruct(name, doc, fields) {
  return `/// ${doc}\nstruct ${name}: Sendable {\n${fields.map((f) => `    let ${f}`).join("\n")}\n}\n`;
}

function swiftFile(directions, contract) {
  const g = nativeGroups(contract);
  const field = (t, type) => `${swiftId(camel(t.name))}: ${type}`;
  const structs = [
    swiftStruct(
      "OxyThemeColors",
      "Every colour token, each following the system appearance.",
      g.colors.map((t) => field(t, "Color"))
    ),
    swiftStruct(
      "OxyThemeRadius",
      "Corner radii in points.",
      g.radius.map((t) => field(t, "CGFloat"))
    ),
    swiftStruct(
      "OxyThemeSpace",
      "Spacing and layout in points.",
      g.space.map((t) => field(t, "CGFloat"))
    ),
    swiftStruct(
      "OxyThemeControlHeight",
      "Control and row heights in points.",
      g.controlHeight.map((t) => field(t, "CGFloat"))
    ),
    swiftStruct(
      "OxyThemeTypeRoles",
      "The type roles.",
      g.roles.map((r) => `${swiftId(camel(r))}: OxyThemeTypeRole`)
    ),
    swiftStruct(
      "OxyThemeTile",
      "The module-icon tile: percentages of the tile side, a fraction, and its lift.",
      g.tile.map((t) => field(t, swiftTileType(t)))
    )
  ];
  return `// OxyTheme.swift: the Oxygen design directions as SwiftUI values.
// Generated by sdk/typescript/scripts/build-theme.mjs from sdk/typescript/src/theme/tokens
// (contract ${contract.version}). Do not edit: change the tokens, then run \`pnpm build:theme\` in
// sdk/typescript. Not wired into the app yet; see README.md beside this file.
//
// Units: lengths are points (a CSS px is a point; rem x 16). Type sizes are points at the default
// text size; scale them with Dynamic Type by role. Tracking is in em: kerning = tracking x size.

import SwiftUI
#if canImport(UIKit)
import UIKit
#elseif canImport(AppKit)
import AppKit
#endif

/// A colour that follows the appearance: \`light\` in light mode, \`dark\` in dark mode.
private func oxyThemeColor(light: Color, dark: Color) -> Color {
    #if canImport(UIKit)
    return Color(uiColor: UIColor { traits in
        traits.userInterfaceStyle == .dark ? UIColor(dark) : UIColor(light)
    })
    #elseif canImport(AppKit)
    return Color(nsColor: NSColor(name: nil) { appearance in
        appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? NSColor(dark) : NSColor(light)
    })
    #endif
}

/// An sRGB colour from 0xRRGGBB and an opacity.
private func oxyRGB(_ hex: UInt32, _ opacity: Double = 1) -> Color {
    Color(
        .sRGB,
        red: Double((hex >> 16) & 0xFF) / 255,
        green: Double((hex >> 8) & 0xFF) / 255,
        blue: Double(hex & 0xFF) / 255,
        opacity: opacity
    )
}

/// A value tuned separately for light and dark mode.
struct OxyThemeByMode<Value: Sendable>: Sendable {
    let light: Value
    let dark: Value
}

/// One shadow layer, as SwiftUI draws it: radius is half the CSS blur. Extra layers are dropped.
struct OxyThemeShadow: Sendable {
    let color: Color
    let radius: CGFloat
    let x: CGFloat
    let y: CGFloat
}

/// One type role.
struct OxyThemeTypeRole: Sendable {
    /// The leading family of the CSS stack, or nil for the system font.
    let family: String?
    /// Points at the default text size.
    let size: CGFloat
    /// A multiple of the size.
    let lineHeight: CGFloat
    /// 1...1000, as in CSS.
    let weight: Int
    /// In em.
    let tracking: CGFloat

    /// The tracking in points at this role's size.
    var kerning: CGFloat { tracking * size }

    /// The nearest SwiftUI weight.
    var fontWeight: Font.Weight {
        switch weight {
        case ..<150: return .ultraLight
        case ..<250: return .thin
        case ..<350: return .light
        case ..<450: return .regular
        case ..<550: return .medium
        case ..<650: return .semibold
        case ..<750: return .bold
        case ..<850: return .heavy
        default: return .black
        }
    }
}

${structs.join("\n")}
/// One design direction.
struct OxyThemeDirection: Sendable, Identifiable {
    let id: String
    let name: String
    let colors: OxyThemeColors
    let radius: OxyThemeRadius
    let space: OxyThemeSpace
    let controlHeight: OxyThemeControlHeight
    let type: OxyThemeTypeRoles
    let tile: OxyThemeTile
}

/// The design directions, in the order theme.css carries them.
enum OxyTheme {
    static let all: [OxyThemeDirection] = [${directions.map((d) => swiftId(camel(d.id))).join(", ")}]

${directions.map((d) => swiftDirection(d, g)).join("\n\n")}
}
`;
}

function kotlinColor(c) {
  const a = Math.round(c.alpha * 255)
    .toString(16)
    .toUpperCase()
    .padStart(2, "0");
  return `Color(0x${a}${c.hex.slice(1).toUpperCase()})`;
}

const dp = (x) => `${decimal(x)}.dp`;

function kotlinShadow(layers) {
  const l = firstShadow(layers);
  if (!l) return "null";
  return `OxyThemeShadow(color = ${kotlinColor(l.color)}, x = ${dp(l.x)}, y = ${dp(l.y)}, blur = ${dp(l.blur)}, spread = ${dp(l.spread)})`;
}

function kotlinTileValue(d, spec) {
  const one = (mode) => {
    const v = d.data[mode].get(spec.name);
    return spec.type === "shadow" ? kotlinShadow(v) : `${decimal(v)}f`;
  };
  return spec.modes === "per-mode"
    ? `OxyThemeByMode(light = ${one("light")}, dark = ${one("dark")})`
    : one("light");
}

function kotlinDirection(d, g) {
  const L = d.data.light;
  const colorSet = (mode) =>
    g.colors
      .map(
        (t) =>
          `            override val ${kotlinId(camel(t.name))} = ${kotlinColor(d.data[mode].get(t.name))}`
      )
      .join("\n");
  const lengths = (list) =>
    list.map((t) => `            ${kotlinId(camel(t.name))} = ${dp(L.get(t.name))}`).join(",\n");
  const roles = g.roles
    .map((r) => {
      const v = (p) => L.get(`--type-${r}-${p}`);
      const family = leadFamily(v("family"));
      return `            ${kotlinId(camel(r))} = OxyThemeTypeRole(family = ${family ? quoteNative(family) : "null"}, size = ${decimal(v("size"))}.sp, lineHeight = ${decimal(v("line"))}f, weight = ${v("weight")}, letterSpacing = (${decimal(v("tracking"))}).em)`;
    })
    .join(",\n");
  const tile = g.tile
    .map((t) => `            ${kotlinId(camel(t.name))} = ${kotlinTileValue(d, t)}`)
    .join(",\n");
  const name = `OxyTheme${camel(`-${d.id}`)}`;
  return `/** ${d.name} (${d.id}). */
object ${name} : OxyThemeDirection {
    override val id = ${quoteNative(d.id)}
    override val name = ${quoteNative(d.name)}
    override val light: OxyThemeColors =
        object : OxyThemeColors {
${colorSet("light")}
        }
    override val dark: OxyThemeColors =
        object : OxyThemeColors {
${colorSet("dark")}
        }
    override val radius =
        OxyThemeRadius(
${lengths(g.radius)},
        )
    override val space =
        OxyThemeSpace(
${lengths(g.space)},
        )
    override val controlHeight =
        OxyThemeControlHeight(
${lengths(g.controlHeight)},
        )
    override val type =
        OxyThemeTypeRoles(
${roles},
        )
    override val tile =
        OxyThemeTile(
${tile},
        )
}
`;
}

function kotlinTileType(spec) {
  const base = spec.type === "shadow" ? "OxyThemeShadow?" : "Float";
  return spec.modes === "per-mode" ? `OxyThemeByMode<${base}>` : base;
}

function kotlinDataClass(name, doc, fields) {
  return `/** ${doc} */\ndata class ${name}(\n${fields.map((f) => `    val ${f},`).join("\n")}\n)\n`;
}

function kotlinFile(directions, contract) {
  const g = nativeGroups(contract);
  const field = (t, type) => `${kotlinId(camel(t.name))}: ${type}`;
  const objects = directions.map((d) => `OxyTheme${camel(`-${d.id}`)}`);
  return `// OxyTheme.kt: the Oxygen design directions as Compose values.
// Generated by sdk/typescript/scripts/build-theme.mjs from sdk/typescript/src/theme/tokens
// (contract ${contract.version}). Do not edit: change the tokens, then run \`pnpm build:theme\` in
// sdk/typescript. Not wired into the app yet; see README.md beside this file.
//
// Units: lengths are dp (a CSS px is a dp; rem x 16), type sizes sp, tracking em. Percentages
// (tile radius and tints) are 0..100 of the tile side or of the mix.

package com.oxygenhq.app.ui.theme

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp

/** Every colour token of one direction in one mode. */
interface OxyThemeColors {
${g.colors.map((t) => `    val ${kotlinId(camel(t.name))}: Color`).join("\n")}
}

/** A value tuned separately for light and dark mode. */
data class OxyThemeByMode<T>(
    val light: T,
    val dark: T,
)

/** One shadow layer; extra CSS layers are dropped. */
data class OxyThemeShadow(
    val color: Color,
    val x: Dp,
    val y: Dp,
    val blur: Dp,
    val spread: Dp,
)

/** One type role. lineHeight is a multiple of size; weight is 1..1000 (FontWeight(weight)). */
data class OxyThemeTypeRole(
    val family: String?,
    val size: TextUnit,
    val lineHeight: Float,
    val weight: Int,
    val letterSpacing: TextUnit,
)

${kotlinDataClass(
  "OxyThemeRadius",
  "Corner radii.",
  g.radius.map((t) => field(t, "Dp"))
)}
${kotlinDataClass(
  "OxyThemeSpace",
  "Spacing and layout.",
  g.space.map((t) => field(t, "Dp"))
)}
${kotlinDataClass(
  "OxyThemeControlHeight",
  "Control and row heights.",
  g.controlHeight.map((t) => field(t, "Dp"))
)}
${kotlinDataClass(
  "OxyThemeTypeRoles",
  "The type roles.",
  g.roles.map((r) => `${kotlinId(camel(r))}: OxyThemeTypeRole`)
)}
${kotlinDataClass(
  "OxyThemeTile",
  "The module-icon tile: percentages of the tile side, a fraction, and its lift.",
  g.tile.map((t) => field(t, kotlinTileType(t)))
)}
/** One design direction: a light and a dark colour set, and its mode-independent tokens. */
interface OxyThemeDirection {
    val id: String
    val name: String
    val light: OxyThemeColors
    val dark: OxyThemeColors
    val radius: OxyThemeRadius
    val space: OxyThemeSpace
    val controlHeight: OxyThemeControlHeight
    val type: OxyThemeTypeRoles
    val tile: OxyThemeTile
}

/** The design directions, in the order theme.css carries them. */
object OxyTheme {
    val all: List<OxyThemeDirection> = listOf(${objects.join(", ")})
}

${directions.map((d) => kotlinDirection(d, g)).join("\n")}`;
}

function nativeReadme(directions) {
  return `# Oxygen theme for the native apps (generated, not wired)

\`OxyTheme.swift\` (SwiftUI) and \`OxyTheme.kt\` (Compose) carry the same design directions as
\`@oxy-hq/sdk/theme.css\`: ${directions.map((d) => `${d.id}`).join(", ")}. They are generated by
\`sdk/typescript/scripts/build-theme.mjs\` from \`sdk/typescript/src/theme/tokens\`, so do not edit them.

They are **not wired** into \`mobile/ios\` or \`mobile/android\`: neither project compiles these files,
and the apps still read their own \`OxyColor\` (iOS) and \`OxyColors\` (Android) tokens. Wiring means
adding the file to the app target, bundling each direction's fonts, and choosing the direction at
runtime (\`OxyTheme.all\`).

What each file carries, per direction: every colour token (Swift: one dynamic colour per token;
Kotlin: a light and a dark set), radii, spacing, control and row heights, the nine type roles
(size in pt or sp, line height as a multiple, weight 1..1000, tracking in em) and the module-icon
tile tokens. A CSS px is a point or a dp; a rem is 16 of them.

Regenerate after changing tokens (this also rewrites theme.css and the web-app copies):

\`\`\`bash
cd sdk/typescript
pnpm build:theme   # write
pnpm check:theme   # exit 1 when a generated file is stale
\`\`\`
`;
}

// ---------------------------------------------------------------------------------------------
// Assemble, write, check
// ---------------------------------------------------------------------------------------------

function loadAll(problems) {
  const contract = loadContract(join(SRC, "contract", "contract.json"), problems);
  if (!contract) return undefined;
  const tokensDir = join(SRC, "tokens");
  const present = existsSync(tokensDir)
    ? readdirSync(tokensDir, { withFileTypes: true })
        .filter((e) => e.isDirectory() && !e.name.startsWith("."))
        .map((e) => e.name)
        .sort()
    : [];
  for (const name of present) {
    if (!DIRECTIONS.includes(name)) {
      problems.add(
        repoPath(join(tokensDir, name)),
        "",
        "files.directions",
        `is not one of ${DIRECTIONS.join(", ")} (add it to DIRECTIONS in build-theme.mjs)`
      );
    }
  }
  const directions = [];
  for (const id of DIRECTIONS) {
    const dir = join(tokensDir, id);
    if (!existsSync(dir)) {
      problems.add(repoPath(dir), "", "files.directions", "is missing");
      continue;
    }
    const d = loadDirection(dir, contract, problems);
    if (!d) continue;
    d.fonts = loadFonts(id, problems);
    if (d.fonts && d.css.light) checkFontCoverage(d, contract, problems);
    directions.push(d);
  }
  checkFontFiles(directions, problems);
  return { contract, directions };
}

/** src/theme/fonts/, sorted, without dotfiles (a Finder .DS_Store is not a font). */
function fontFiles() {
  const dir = join(SRC, "fonts");
  return existsSync(dir)
    ? readdirSync(dir)
        .filter((f) => !f.startsWith("."))
        .sort()
    : [];
}

/** One family per direction; every file in fonts/ is used by a face. */
function checkFontFiles(directions, problems) {
  const owner = new Map();
  const used = new Set();
  for (const d of directions) {
    for (const face of d.fonts?.faces ?? []) {
      const prior = owner.get(face.family);
      if (prior && prior !== d.folder) {
        problems.add(
          repoPath(join(SRC, "tokens", d.folder, "fonts.json")),
          "faces",
          "font.face",
          `${face.family} is already served by ${prior}`
        );
      }
      owner.set(face.family, d.folder);
      used.add(face.file);
      used.add(face.licence);
    }
  }
  for (const file of fontFiles()) {
    if (!used.has(file))
      problems.add(
        repoPath(join(SRC, "fonts", file)),
        "",
        "font.orphan",
        "no face in any fonts.json uses it"
      );
  }
}

/** True when some direction self-hosts a face, so theme-fonts.css and the font copies exist. */
function hasFaces(directions) {
  return directions.some((d) => (d.fonts?.faces ?? []).length > 0);
}

/** Outputs a build without faces does not write; --check names them stale when they linger. */
function retiredOutputs(outputs) {
  return [OUT.fontsCss, OUT.webFontsCss].filter((p) => !outputs.has(p));
}

function buildOutputs({ contract, directions }) {
  const css = themeCss(directions, contract);
  const out = new Map([
    [OUT.css, css],
    [OUT.themesTs, themesTs(directions)],
    [OUT.swift, swiftFile(directions, contract)],
    [OUT.kotlin, kotlinFile(directions, contract)],
    [OUT.readme, nativeReadme(directions)],
    [OUT.webCss, css]
  ]);
  if (hasFaces(directions)) {
    const fontsCss = themeFontsCss(directions);
    out.set(OUT.fontsCss, fontsCss);
    out.set(OUT.webFontsCss, fontsCss);
    for (const file of fontFiles()) {
      out.set(`${OUT.webFonts}/${file}`, readFileSync(join(SRC, "fonts", file)));
    }
  }
  return out;
}

/** Generate every output in memory: { outputs: Map<repo path, string | Buffer>, problems: string[] }. */
function generate() {
  const problems = new Problems();
  const loaded = loadAll(problems);
  if (!loaded || problems.size) return { outputs: new Map(), problems: problems.list };
  return { outputs: buildOutputs(loaded), problems: [] };
}

function sameBytes(file, content) {
  if (!existsSync(file) || !statSync(file).isFile()) return false;
  return readFileSync(file).equals(
    Buffer.isBuffer(content) ? content : Buffer.from(content, "utf8")
  );
}

/** Files in the generated fonts folder that no output names, and retired outputs still on disk. */
function staleFonts(root, outputs) {
  const dir = join(root, OUT.webFonts);
  const fonts = existsSync(dir)
    ? readdirSync(dir)
        .filter((f) => !f.startsWith("."))
        .map((f) => `${OUT.webFonts}/${f}`)
        .filter((p) => !outputs.has(p))
    : [];
  const retired = retiredOutputs(outputs).filter((p) => existsSync(join(root, p)));
  return [...fonts, ...retired].sort();
}

function writeOutputs(root, outputs) {
  let written = 0;
  for (const [path, content] of outputs) {
    const file = join(root, path);
    if (sameBytes(file, content)) continue;
    mkdirSync(dirname(file), { recursive: true });
    writeFileSync(file, content);
    written += 1;
  }
  for (const path of staleFonts(root, outputs)) rmSync(join(root, path));
  // With no faces left the copied fonts folder goes too, so the web-app tree holds no empty dir.
  const fontsDir = join(root, OUT.webFonts);
  if (existsSync(fontsDir) && readdirSync(fontsDir).every((f) => f.startsWith(".")))
    rmSync(fontsDir, { recursive: true });
  return written;
}

function checkOutputs(root, outputs) {
  const differ = [];
  for (const [path, content] of outputs) {
    if (!sameBytes(join(root, path), content))
      differ.push(existsSync(join(root, path)) ? `${path} (differs)` : `${path} (missing)`);
  }
  for (const path of staleFonts(root, outputs)) differ.push(`${path} (stale: no output writes it)`);
  return differ;
}

function emitCss(dir) {
  const problems = new Problems();
  const contract = loadContract(join(SRC, "contract", "contract.json"), problems);
  const d = contract && loadDirection(resolve(dir), contract, problems);
  if (!d || problems.size) return { problems: problems.list };
  return { css: directionCss(d, contract), problems: [] };
}

function parseArgs(argv) {
  const args = { check: false, out: "", emit: "", src: "" };
  const takesFolder = { "--out": "out", "--emit-css": "emit", "--src": "src" };
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    if (a === "--check") args.check = true;
    else if (Object.hasOwn(takesFolder, a)) args[takesFolder[a]] = argv[++i];
    else throw new Error(`unknown argument ${a}`);
    if (Object.hasOwn(takesFolder, a) && !argv[i]) throw new Error(`${a} needs a folder`);
  }
  if (args.src && !args.out && !args.check && !args.emit) {
    throw new Error(
      "--src writes only under --out: the repo's outputs come from the repo's tokens (or add --check)"
    );
  }
  if (args.src && !existsSync(join(resolve(args.src), "contract", "contract.json"))) {
    throw new Error(`--src ${args.src} has no contract/contract.json (it mirrors src/theme)`);
  }
  return args;
}

function main(argv) {
  let args;
  try {
    args = parseArgs(argv);
  } catch (err) {
    console.error(`build-theme: ${err.message}`);
    return 2;
  }
  if (args.src) SRC = resolve(args.src);
  if (args.emit) {
    const { css, problems } = emitCss(args.emit);
    if (problems.length) {
      console.error(`build-theme: ${problems.length} problem(s):\n  ${problems.join("\n  ")}`);
      return 1;
    }
    process.stdout.write(css);
    return 0;
  }
  const { outputs, problems } = generate();
  if (problems.length) {
    console.error(
      `build-theme: ${problems.length} problem(s); nothing written:\n  ${problems.join("\n  ")}`
    );
    return 1;
  }
  const root = args.out ? resolve(args.out) : REPO;
  if (args.check) {
    const differ = checkOutputs(root, outputs);
    if (differ.length) {
      console.error(
        `build-theme --check: ${differ.length} output(s) differ from what the tokens generate:\n  ${differ.join("\n  ")}\nRun \`pnpm build:theme\` in sdk/typescript.`
      );
      return 1;
    }
    console.log(`build-theme --check: ${outputs.size} outputs match the tokens`);
    return 0;
  }
  const written = writeOutputs(root, outputs);
  console.log(
    `build-theme: ${outputs.size} outputs, ${written} written${args.out ? ` under ${args.out}` : ""}`
  );
  return 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  process.exitCode = main(process.argv.slice(2));
}
