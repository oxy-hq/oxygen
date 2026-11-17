// @vitest-environment jsdom

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
// @ts-expect-error -- jsdom ships no types and @types/jsdom is not a dependency; the one
// constructor used here is typed below.
import { JSDOM } from "jsdom";
import { beforeEach, describe, expect, it } from "vitest";
import {
  applyOxyMode,
  applyOxyTheme,
  isOxyModeChoice,
  isOxyThemeApplied,
  isOxyThemeId,
  isOxyThemePreviewAllowed,
  OXY_MODE_PIN_ATTRIBUTE,
  OXY_PERSON_MODE_KEY,
  OXY_THEME_PREPAINT,
  type OxyMode,
  type OxyThemeStorage,
  type OxyThemeWindow,
  readOxyModePin,
  readOxyPersonMode,
  readOxyThemeSelection,
  rememberOxyPersonMode,
  THEMES
} from "./index";
import { oxyThemeRuntime } from "./runtime";

const here = dirname(fileURLToPath(import.meta.url));

interface Page {
  window: Window & typeof globalThis;
}
type PageOptions = {
  url: string;
  runScripts: "dangerously";
  /** Gives the page requestAnimationFrame, which the pre-paint settles on. */
  pretendToBeVisual?: boolean;
  beforeParse(window: Window & typeof globalThis): void;
};
const Dom = JSDOM as new (html: string, options: PageOptions) => Page;

class MemoryStorage implements OxyThemeStorage {
  readonly items = new Map<string, string>();
  constructor(seed: Record<string, string> = {}) {
    for (const [k, v] of Object.entries(seed)) this.items.set(k, v);
  }
  getItem(key: string) {
    return this.items.get(key) ?? null;
  }
  setItem(key: string, value: string) {
    this.items.set(key, value);
  }
}

const INJECTION = '"><img src=x onerror=alert(1)>';

describe("THEMES", () => {
  it("lists the one shipped theme, the reference app's, with a name, carrier and description", () => {
    expect(THEMES.map((t) => t.id)).toEqual(["reference"]);
    for (const t of THEMES) {
      expect(t.name.length).toBeGreaterThan(0);
      expect(t.carrier.length).toBeGreaterThan(0);
      expect(t.description.length).toBeGreaterThan(20);
    }
  });

  it("isOxyThemeId accepts exactly those ids", () => {
    for (const t of THEMES) expect(isOxyThemeId(t.id)).toBe(true);
    for (const v of [
      "x0",
      // The five candidate directions are gone: their ids are refused like any other.
      "argent",
      "folio",
      "Reference",
      "reference ",
      "",
      INJECTION,
      null,
      undefined,
      1,
      {}
    ]) {
      expect(isOxyThemeId(v)).toBe(false);
    }
  });
});

describe("applyOxyTheme", () => {
  const root = document.documentElement;
  beforeEach(() => {
    root.removeAttribute("data-oxy-theme");
    root.removeAttribute("data-oxy-mode");
    root.className = "";
  });

  it("sets both attributes and mirrors dark as the .dark class", () => {
    expect(applyOxyTheme({ theme: "reference", mode: "dark" })).toBe(true);
    expect(root.getAttribute("data-oxy-theme")).toBe("reference");
    expect(root.getAttribute("data-oxy-mode")).toBe("dark");
    expect(root.classList.contains("dark")).toBe(true);
    expect(applyOxyTheme({ theme: "reference", mode: "light" })).toBe(true);
    expect(root.getAttribute("data-oxy-mode")).toBe("light");
    expect(root.classList.contains("dark")).toBe(false);
  });

  it("removes what is null, and leaves .dark to the app once the mode is cleared", () => {
    applyOxyTheme({ theme: "reference", mode: "dark" });
    expect(applyOxyTheme({ theme: null, mode: null })).toBe(true);
    expect(root.hasAttribute("data-oxy-theme")).toBe(false);
    expect(root.hasAttribute("data-oxy-mode")).toBe(false);
    expect(root.classList.contains("dark")).toBe(true);
  });

  it("refuses an unknown id or mode and writes nothing", () => {
    applyOxyTheme({ theme: "reference", mode: "light" });
    for (const choice of [
      { theme: "x0" },
      { theme: INJECTION },
      { theme: "reference", mode: "sepia" }
    ]) {
      expect(applyOxyTheme(choice as never)).toBe(false);
      expect(root.getAttribute("data-oxy-theme")).toBe("reference");
      expect(root.getAttribute("data-oxy-mode")).toBe("light");
    }
    expect(root.outerHTML).not.toContain("<img");
  });

  it("writes to the root it is given", () => {
    const other = document.createElement("div");
    expect(applyOxyTheme({ theme: "reference", mode: "dark" }, other)).toBe(true);
    expect(other.getAttribute("data-oxy-theme")).toBe("reference");
    expect(root.hasAttribute("data-oxy-theme")).toBe(false);
  });
});

describe("readOxyThemeSelection", () => {
  it("reads the query and persists it for the session", () => {
    const storage = new MemoryStorage();
    expect(
      readOxyThemeSelection({
        search: "?oxy-theme=reference&oxy-mode=dark",
        storage,
        prefersDark: false
      })
    ).toEqual({
      theme: "reference",
      mode: "dark"
    });
    expect([...storage.items]).toEqual([
      ["oxy-theme", "reference"],
      ["oxy-mode", "dark"]
    ]);
    expect(readOxyThemeSelection({ search: "", storage, prefersDark: false })).toEqual({
      theme: "reference",
      mode: "dark"
    });
  });

  it("lets the query override the session, one value at a time", () => {
    const storage = new MemoryStorage({ "oxy-theme": "reference", "oxy-mode": "light" });
    expect(
      readOxyThemeSelection({ search: "?oxy-theme=reference", storage, prefersDark: true })
    ).toEqual({
      theme: "reference",
      mode: "light"
    });
    expect(storage.getItem("oxy-theme")).toBe("reference");
  });

  it("falls back to the system preference for the mode only", () => {
    expect(
      readOxyThemeSelection({ search: "?oxy-theme=reference", storage: null, prefersDark: true })
    ).toEqual({
      theme: "reference",
      mode: "dark"
    });
  });

  it("previews the shipped theme when only a mode is asked, and keeps it for the session", () => {
    const storage = new MemoryStorage();
    expect(
      readOxyThemeSelection({ search: "?oxy-mode=dark", storage, prefersDark: false })
    ).toEqual({ theme: "reference", mode: "dark" });
    expect(storage.getItem("oxy-theme")).toBeNull();
    expect(readOxyThemeSelection({ search: "", storage, prefersDark: false })).toEqual({
      theme: "reference",
      mode: "dark"
    });
    expect(
      readOxyThemeSelection({ search: "?oxy-mode=sepia", storage: null, prefersDark: true })
    ).toBeNull();
    expect(readOxyThemeSelection({ search: "", storage: null, prefersDark: true })).toBeNull();
  });

  it("refuses a query naming an unknown theme whole: its mode neither applies nor persists", () => {
    const storage = new MemoryStorage({ "oxy-theme": "reference" });
    expect(
      readOxyThemeSelection({ search: "?oxy-theme=d9&oxy-mode=dark", storage, prefersDark: false })
    ).toBeNull();
    expect(storage.getItem("oxy-mode")).toBeNull();
  });

  it("ignores values outside the allow-list, in the query and in storage", () => {
    const storage = new MemoryStorage({ "oxy-theme": INJECTION, "oxy-mode": "sepia" });
    expect(
      readOxyThemeSelection({
        search: `?oxy-theme=${encodeURIComponent(INJECTION)}`,
        storage,
        prefersDark: false
      })
    ).toBeNull();
    expect(
      readOxyThemeSelection({
        search: "?oxy-theme=x0&oxy-mode=dark",
        storage: new MemoryStorage(),
        prefersDark: false
      })
    ).toBeNull();
    expect(
      readOxyThemeSelection({
        search: "?oxy-theme=reference&oxy-mode=sepia",
        storage,
        prefersDark: false
      })
    ).toEqual({
      theme: "reference",
      mode: "light"
    });
    expect(storage.getItem("oxy-mode")).toBe("sepia");
  });

  it("still applies the query when storage throws", () => {
    const broken: OxyThemeStorage = {
      getItem() {
        throw new Error("SecurityError");
      },
      setItem() {
        throw new Error("QuotaExceededError");
      }
    };
    expect(
      readOxyThemeSelection({ search: "?oxy-theme=reference", storage: broken, prefersDark: false })
    ).toEqual({
      theme: "reference",
      mode: "light"
    });
  });
});

describe("isOxyThemePreviewAllowed", () => {
  const table: [string | undefined, string | undefined, boolean][] = [
    ["staging", "staging--acme--ops.oxygen-hq.com", true],
    ["dev-luong", "dev-luong--acme--ops.oxygen-hq.com", true],
    ["dev-a1-b2", "app.example.com", true],
    ["production", "acme--ops.oxygen-hq.com", false],
    ["production", "app.oxygen-hq.com", false],
    ["production", "127.0.0.1", true],
    ["production", "localhost", true],
    ["production", "ops.localhost", true],
    ["production", "::1", true],
    ["production", "[::1]", true],
    ["production", "localhost.example.com", false],
    ["production", "evil-localhost", false],
    ["production", "127.0.0.1.nip.io", false],
    [undefined, "app.oxygen-hq.com", false],
    [undefined, "localhost", true],
    [undefined, undefined, false],
    ["Staging", "app.oxygen-hq.com", false],
    ["", "app.oxygen-hq.com", false]
  ];
  it.each(table)("environment %s on %s -> %s", (environment, hostname, want) => {
    expect(isOxyThemePreviewAllowed(environment, hostname)).toBe(want);
  });
});

// The pre-paint string, run as the platform serves a page: the injected __OXY_APP__ in <head>,
// then the inline script at the top of <body>. Each case runs twice, once as that page and once
// through the three exported functions, and both must give the expected result.
interface Case {
  name: string;
  host: string;
  search?: string;
  environment?: string;
  stored?: Record<string, string>;
  prefersDark?: boolean;
  want: {
    theme: string | null;
    mode: OxyMode | null;
    dark: boolean;
    stored: Record<string, string>;
  };
}

const none = (stored: Record<string, string> = {}) => ({
  theme: null,
  mode: null,
  dark: false,
  stored
});
const cases: Case[] = [
  {
    name: "staging, theme and mode in the query",
    host: "staging--acme--ops.oxygen-hq.com",
    environment: "staging",
    search: "?oxy-theme=reference&oxy-mode=dark",
    want: {
      theme: "reference",
      mode: "dark",
      dark: true,
      stored: { "oxy-theme": "reference", "oxy-mode": "dark" }
    }
  },
  {
    name: "dev sandbox, theme only, system light",
    host: "dev-luong--acme--ops.oxygen-hq.com",
    environment: "dev-luong",
    search: "?oxy-theme=reference",
    want: { theme: "reference", mode: "light", dark: false, stored: { "oxy-theme": "reference" } }
  },
  {
    name: "dev sandbox, theme only, system dark",
    host: "dev-x--acme--ops.oxygen-hq.com",
    environment: "dev-x",
    search: "?oxy-theme=reference",
    prefersDark: true,
    want: { theme: "reference", mode: "dark", dark: true, stored: { "oxy-theme": "reference" } }
  },
  {
    name: "production on a real host",
    host: "acme--ops.oxygen-hq.com",
    environment: "production",
    search: "?oxy-theme=reference&oxy-mode=dark",
    want: none()
  },
  {
    name: "production on a real host, with a theme already in the session",
    host: "app.oxygen-hq.com",
    environment: "production",
    stored: { "oxy-theme": "reference" },
    want: none({ "oxy-theme": "reference" })
  },
  {
    name: "production on 127.0.0.1 (local oxy start)",
    host: "127.0.0.1:3000",
    environment: "production",
    search: "?oxy-theme=reference&oxy-mode=dark",
    want: {
      theme: "reference",
      mode: "dark",
      dark: true,
      stored: { "oxy-theme": "reference", "oxy-mode": "dark" }
    }
  },
  {
    name: "production on [::1]",
    host: "[::1]:3000",
    environment: "production",
    search: "?oxy-theme=reference",
    want: { theme: "reference", mode: "light", dark: false, stored: { "oxy-theme": "reference" } }
  },
  {
    name: "no environment (not served by oxy) on a real host",
    host: "acme.example.com",
    search: "?oxy-theme=reference",
    want: none()
  },
  {
    name: "no environment on a local dev server",
    host: "localhost:5173",
    search: "?oxy-theme=reference&oxy-mode=light",
    want: {
      theme: "reference",
      mode: "light",
      dark: false,
      stored: { "oxy-theme": "reference", "oxy-mode": "light" }
    }
  },
  {
    name: "staging, an unknown id",
    host: "staging--acme--ops.oxygen-hq.com",
    environment: "staging",
    search: "?oxy-theme=d9&oxy-mode=dark",
    want: none()
  },
  {
    name: "staging, a mode on its own",
    host: "staging--acme--ops.oxygen-hq.com",
    environment: "staging",
    search: "?oxy-mode=dark",
    want: { theme: "reference", mode: "dark", dark: true, stored: { "oxy-mode": "dark" } }
  },
  {
    name: "staging, the session's mode with no query",
    host: "staging--acme--ops.oxygen-hq.com",
    environment: "staging",
    stored: { "oxy-mode": "dark" },
    want: { theme: "reference", mode: "dark", dark: true, stored: { "oxy-mode": "dark" } }
  },
  {
    name: "production on a real host, a mode on its own",
    host: "acme--ops.oxygen-hq.com",
    environment: "production",
    search: "?oxy-mode=dark",
    want: none()
  },
  {
    name: "staging, a mode off the allow-list on its own",
    host: "staging--acme--ops.oxygen-hq.com",
    environment: "staging",
    search: "?oxy-mode=sepia",
    want: none()
  },
  {
    name: "staging, an injection attempt",
    host: "staging--acme--ops.oxygen-hq.com",
    environment: "staging",
    search: `?oxy-theme=${encodeURIComponent(INJECTION)}`,
    want: none()
  },
  {
    name: "staging, the session's choice with no query",
    host: "staging--acme--ops.oxygen-hq.com",
    environment: "staging",
    stored: { "oxy-theme": "reference", "oxy-mode": "dark" },
    want: {
      theme: "reference",
      mode: "dark",
      dark: true,
      stored: { "oxy-theme": "reference", "oxy-mode": "dark" }
    }
  },
  {
    name: "staging, a tampered session value",
    host: "staging--acme--ops.oxygen-hq.com",
    environment: "staging",
    stored: { "oxy-theme": INJECTION },
    want: none({ "oxy-theme": INJECTION })
  },
  {
    name: "an environment that is not exactly staging",
    host: "staging--acme--ops.oxygen-hq.com",
    environment: "Staging",
    search: "?oxy-theme=reference",
    want: none()
  }
];

function snapshot(root: HTMLElement, storage: OxyThemeStorage) {
  const stored: Record<string, string> = {};
  for (const key of ["oxy-theme", "oxy-mode"]) {
    const v = storage.getItem(key);
    if (v !== null) stored[key] = v;
  }
  return {
    theme: root.getAttribute("data-oxy-theme"),
    mode: root.getAttribute("data-oxy-mode"),
    dark: root.classList.contains("dark"),
    stored
  };
}

function runAsPage(c: Case) {
  const app =
    c.environment === undefined
      ? ""
      : `<script>window.__OXY_APP__=JSON.parse(${JSON.stringify(JSON.stringify({ environment: c.environment }))});</script>`;
  const html = `<!doctype html><html><head>${app}</head><body><script>${OXY_THEME_PREPAINT}</script><main></main></body></html>`;
  const page = new Dom(html, {
    url: `http://${c.host}/customer-apps/acme/ops/${c.search ?? ""}`,
    runScripts: "dangerously",
    beforeParse(w) {
      Object.defineProperty(w, "matchMedia", {
        value: (q: string) => ({
          matches: Boolean(c.prefersDark) && q === "(prefers-color-scheme: dark)"
        })
      });
      for (const [k, v] of Object.entries(c.stored ?? {})) w.sessionStorage.setItem(k, v);
    }
  });
  const doc = page.window.document;
  return {
    result: snapshot(doc.documentElement, page.window.sessionStorage),
    images: doc.querySelectorAll("img").length
  };
}

function runAsFunctions(c: Case) {
  const root = document.implementation.createHTMLDocument("").documentElement;
  const storage = new MemoryStorage(c.stored);
  if (isOxyThemePreviewAllowed(c.environment, new URL(`http://${c.host}/`).hostname)) {
    const selection = readOxyThemeSelection({
      search: c.search ?? "",
      storage,
      prefersDark: Boolean(c.prefersDark)
    });
    if (selection) applyOxyTheme(selection, root);
  }
  return snapshot(root, storage);
}

describe("OXY_THEME_PREPAINT", () => {
  it("is one self-contained statement that cannot end its script element", () => {
    expect(OXY_THEME_PREPAINT).toMatch(
      /^\(function oxyThemeRuntime\(ids\) \{[\s\S]*\}\)\(\["reference"\]\)\.prepaint\(window\);$/
    );
    expect(OXY_THEME_PREPAINT).not.toMatch(/<\/script|<!--/i);
  });

  it.each(cases.map((c) => [c.name, c] as const))(
    "%s: the page and the functions agree",
    (_name, c) => {
      const page = runAsPage(c);
      expect(page.result).toEqual(c.want);
      expect(runAsFunctions(c)).toEqual(c.want);
      expect(page.images).toBe(0);
    }
  );

  it("never throws into the page, even without storage or matchMedia", () => {
    const page = new Dom(
      `<!doctype html><html><head></head><body><script>${OXY_THEME_PREPAINT}</script></body></html>`,
      {
        url: "http://localhost:5173/?oxy-theme=reference",
        runScripts: "dangerously",
        beforeParse(w) {
          Object.defineProperty(w, "sessionStorage", {
            get() {
              throw new Error("SecurityError");
            }
          });
        }
      }
    );
    expect(page.window.document.documentElement.getAttribute("data-oxy-theme")).toBe("reference");
    expect(page.window.document.documentElement.getAttribute("data-oxy-mode")).toBe("light");
  });
});

// The person's mode, which the shell keeps in this origin's localStorage, and the app's pin. With
// theme.css applied the pre-paint shows the pin, else a staging preview's own mode, else the
// person's; without it, nothing, so an app that never imported the theme (its own `.dark` styles
// included) looks exactly as it did.
interface ModeCase {
  name: string;
  host?: string;
  environment?: string;
  search?: string;
  themed: boolean;
  person?: string;
  pin?: string;
  prefersDark?: boolean;
  session?: Record<string, string>;
  want: { mode: string | null; dark: boolean };
}
const THEME_CSS = readFileSync(join(here, "theme.css"), "utf8");
const modeCases: ModeCase[] = [
  {
    name: "production, the person's dark",
    themed: true,
    person: "dark",
    want: { mode: "dark", dark: true }
  },
  {
    name: "production, the person's dark, without theme.css",
    themed: false,
    person: "dark",
    want: { mode: null, dark: false }
  },
  {
    name: "production, system on a dark device",
    themed: true,
    person: "system",
    prefersDark: true,
    want: { mode: "dark", dark: true }
  },
  {
    name: "production, system on a light device",
    themed: true,
    person: "system",
    want: { mode: "light", dark: false }
  },
  {
    name: "production, the app pins light over the person's dark",
    themed: true,
    person: "dark",
    pin: "light",
    want: { mode: "light", dark: false }
  },
  {
    name: "production, a pin without theme.css does nothing",
    themed: false,
    pin: "dark",
    want: { mode: null, dark: false }
  },
  {
    name: "production, nothing remembered: the page as it is",
    themed: true,
    want: { mode: null, dark: false }
  },
  {
    name: "production, a tampered copy and pin are ignored",
    themed: true,
    person: INJECTION,
    pin: "sepia",
    want: { mode: null, dark: false }
  },
  {
    name: "production, a preview's mode is not honoured off the preview gate",
    themed: true,
    person: "light",
    search: "?oxy-mode=dark",
    want: { mode: "light", dark: false }
  },
  {
    name: "staging, a preview's own mode outranks the person's",
    host: "staging--acme--ops.oxygen-hq.com",
    environment: "staging",
    themed: true,
    person: "dark",
    search: "?oxy-mode=light",
    want: { mode: "light", dark: false }
  },
  {
    name: "staging, the pin outranks a preview's mode",
    host: "staging--acme--ops.oxygen-hq.com",
    environment: "staging",
    themed: true,
    pin: "light",
    session: { "oxy-mode": "dark" },
    want: { mode: "light", dark: false }
  }
];

describe("OXY_THEME_PREPAINT: the person's mode and the pin", () => {
  it.each(modeCases.map((c) => [c.name, c] as const))("%s", (_name, c) => {
    const app =
      c.environment === undefined
        ? '<script>window.__OXY_APP__={"environment":"production"};</script>'
        : `<script>window.__OXY_APP__=${JSON.stringify({ environment: c.environment })};</script>`;
    const style = c.themed ? `<style>${THEME_CSS}</style>` : "";
    const pin = c.pin === undefined ? "" : ` data-oxy-mode-pin="${c.pin.replace(/"/g, "&quot;")}"`;
    const html = `<!doctype html><html${pin}><head>${style}${app}</head><body><script>${OXY_THEME_PREPAINT}</script></body></html>`;
    const page = new Dom(html, {
      url: `https://${c.host ?? "acme--ops.customer-apps.oxygen-hq.com"}/${c.search ?? ""}`,
      runScripts: "dangerously",
      beforeParse(w) {
        Object.defineProperty(w, "matchMedia", {
          value: (q: string) => ({
            matches: Boolean(c.prefersDark) && q === "(prefers-color-scheme: dark)"
          })
        });
        if (c.person !== undefined) w.localStorage.setItem("oxy-person-mode", c.person);
        for (const [k, v] of Object.entries(c.session ?? {})) w.sessionStorage.setItem(k, v);
      }
    });
    const root = page.window.document.documentElement;
    expect({
      mode: root.getAttribute("data-oxy-mode"),
      dark: root.classList.contains("dark")
    }).toEqual(c.want);
    expect(page.window.document.querySelectorAll("img").length).toBe(0);
  });

  it("never throws into the page when localStorage throws", () => {
    const page = new Dom(
      `<!doctype html><html><head><style>${THEME_CSS}</style></head><body><script>${OXY_THEME_PREPAINT}</script></body></html>`,
      {
        url: "https://acme--ops.customer-apps.oxygen-hq.com/",
        runScripts: "dangerously",
        beforeParse(w) {
          Object.defineProperty(w, "localStorage", {
            get() {
              throw new Error("SecurityError");
            }
          });
        }
      }
    );
    expect(page.window.document.documentElement.getAttribute("data-oxy-mode")).toBeNull();
  });
});

// The browser's own chrome in the page ground of the mode in force, before first paint: with the
// theme, every `<meta name="theme-color">` takes `--background` as <html> computes it (the theme's
// #f6f6f7 light, #0a0a0a dark), and a page with none gets one; without it, the page's own stays.
describe("OXY_THEME_PREPAINT: theme-color", () => {
  const prepaint = (opts: { themed: boolean; person?: string; pin?: string; meta?: string }) => {
    const style = opts.themed ? `<style>${THEME_CSS}</style>` : "";
    const pin = opts.pin ? ` data-oxy-mode-pin="${opts.pin}"` : "";
    const meta = opts.meta === undefined ? "" : `<meta name="theme-color" content="${opts.meta}">`;
    const page = new Dom(
      `<!doctype html><html${pin}><head>${meta}${style}<script>window.__OXY_APP__={"environment":"production"};</script></head><body><script>${OXY_THEME_PREPAINT}</script></body></html>`,
      {
        url: "https://acme--ops.customer-apps.oxygen-hq.com/",
        runScripts: "dangerously",
        beforeParse(w) {
          if (opts.person) w.localStorage.setItem("oxy-person-mode", opts.person);
        }
      }
    );
    return [...page.window.document.querySelectorAll('meta[name="theme-color"]')].map((m) =>
      m.getAttribute("content")
    );
  };

  it("is the dark ground for the person's dark", () => {
    expect(prepaint({ themed: true, person: "dark", meta: "#ffffff" })).toEqual(["#0a0a0a"]);
  });

  it("is the light ground under a light pin, whatever the person picked", () => {
    expect(prepaint({ themed: true, person: "dark", pin: "light", meta: "#ffffff" })).toEqual([
      "#f6f6f7"
    ]);
  });

  it("is the light ground before anyone chose, and is added where the page has no meta", () => {
    expect(prepaint({ themed: true })).toEqual(["#f6f6f7"]);
  });

  it("follows a later change of mode on <html>, whoever makes it", async () => {
    const page = new Dom(
      `<!doctype html><html><head><meta name="theme-color" content="#ffffff"><style>${THEME_CSS}</style><script>window.__OXY_APP__={"environment":"production"};</script></head><body><script>${OXY_THEME_PREPAINT}</script></body></html>`,
      {
        url: "https://acme--ops.customer-apps.oxygen-hq.com/",
        runScripts: "dangerously",
        beforeParse() {}
      }
    );
    const doc = page.window.document;
    const content = () => doc.querySelector('meta[name="theme-color"]')?.getAttribute("content");
    expect(content()).toBe("#f6f6f7");
    doc.documentElement.classList.add("dark");
    await new Promise((done) => setTimeout(done, 0));
    expect(content()).toBe("#0a0a0a");
  });

  it("is left alone without theme.css", () => {
    expect(prepaint({ themed: false, person: "dark", meta: "#ffffff" })).toEqual(["#ffffff"]);
    expect(prepaint({ themed: false, person: "dark" })).toEqual([]);
  });
});

describe("the mode helpers the shell uses", () => {
  const root = document.documentElement;
  beforeEach(() => {
    root.removeAttribute("data-oxy-theme");
    root.removeAttribute("data-oxy-mode");
    root.removeAttribute("data-oxy-mode-pin");
    root.className = "";
    localStorage.clear();
  });

  it("applyOxyMode sets the mode and .dark and leaves a preview's theme alone", () => {
    root.setAttribute("data-oxy-theme", "reference");
    expect(applyOxyMode("dark")).toBe(true);
    expect(root.getAttribute("data-oxy-mode")).toBe("dark");
    expect(root.classList.contains("dark")).toBe(true);
    expect(root.getAttribute("data-oxy-theme")).toBe("reference");
    expect(applyOxyMode("light")).toBe(true);
    expect(root.classList.contains("dark")).toBe(false);
    expect(applyOxyMode("sepia" as OxyMode)).toBe(false);
    expect(root.getAttribute("data-oxy-mode")).toBe("light");
  });

  it("keeps and reads this origin's copy of the person's mode, allow-listed", () => {
    expect(readOxyPersonMode()).toBeNull();
    rememberOxyPersonMode("system");
    expect(readOxyPersonMode()).toBe("system");
    localStorage.setItem(OXY_PERSON_MODE_KEY, INJECTION);
    expect(readOxyPersonMode()).toBeNull();
    expect(isOxyModeChoice("dark")).toBe(true);
    expect(isOxyModeChoice("auto")).toBe(false);
  });

  it("reads the pin and the theme's marker from the root", () => {
    expect(readOxyModePin()).toBeNull();
    root.setAttribute(OXY_MODE_PIN_ATTRIBUTE, "light");
    expect(readOxyModePin()).toBe("light");
    root.setAttribute(OXY_MODE_PIN_ATTRIBUTE, "system");
    expect(readOxyModePin()).toBeNull();
    expect(isOxyThemeApplied()).toBe(false);
    root.style.setProperty("--oxy-theme-contract", "1");
    expect(isOxyThemeApplied()).toBe(true);
    root.style.removeProperty("--oxy-theme-contract");
  });
});

// The pre-paint in <head>, ahead of the stylesheets (README "Light and dark"): at the top of <body>
// Chrome could paint the empty, light page before it ran, one light frame for a dark person. In
// <head> theme.css is not applied yet when it runs, so the mode goes on <html> at once, and the rest
// is settled on the first animation frame, which a browser runs once the stylesheets in <head> have
// loaded and before it paints (jsdom runs frames with `pretendToBeVisual`).
describe("OXY_THEME_PREPAINT in <head>, ahead of the stylesheets", () => {
  interface HeadCase {
    themed: boolean;
    person?: string;
    /** <html>'s own attributes, as an app's index.html writes them. */
    html?: string;
    /** Markup right after the pre-paint, before the stylesheet (a pin script, a meta). */
    after?: string;
    host?: string;
    environment?: string;
    search?: string;
  }
  const open = (c: HeadCase) => {
    const style = c.themed ? `<style>${THEME_CSS}</style>` : "";
    // The platform splices __OXY_APP__ in just before </head>: after the pre-paint.
    const app = `<script>window.__OXY_APP__=${JSON.stringify({ environment: c.environment ?? "production" })};</script>`;
    const page = new Dom(
      `<!doctype html><html${c.html ?? ""}><head><meta name="theme-color" content="#ffffff"><script>${OXY_THEME_PREPAINT}</script>${c.after ?? ""}${style}${app}</head><body><main></main></body></html>`,
      {
        url: `https://${c.host ?? "acme--ops.customer-apps.oxygen-hq.com"}/${c.search ?? ""}`,
        runScripts: "dangerously",
        pretendToBeVisual: true,
        beforeParse(w) {
          if (c.person !== undefined) w.localStorage.setItem("oxy-person-mode", c.person);
        }
      }
    );
    const w = page.window;
    const root = w.document.documentElement;
    const state = () => ({
      mode: root.getAttribute("data-oxy-mode"),
      dark: root.classList.contains("dark"),
      color: w.document.querySelector('meta[name="theme-color"]')?.getAttribute("content") ?? null
    });
    // Registered after the pre-paint's own frame callback, so it reads what that frame paints.
    const firstFrame = () =>
      new Promise<ReturnType<typeof state>>((done) => w.requestAnimationFrame(() => done(state())));
    return { w, state, firstFrame };
  };

  it("puts the person's dark on <html> at once, and theme-color on the dark ground by the first frame", async () => {
    const page = open({ themed: true, person: "dark" });
    expect(page.state()).toEqual({ mode: "dark", dark: true, color: "#ffffff" });
    expect(await page.firstFrame()).toEqual({ mode: "dark", dark: true, color: "#0a0a0a" });
  });

  it("lets a pin written after it win by the first frame, theme-color on the light ground", async () => {
    const pin =
      '<script>document.documentElement.setAttribute("data-oxy-mode-pin", "light");</script>';
    const page = open({ themed: true, person: "dark", after: pin });
    expect(await page.firstFrame()).toEqual({ mode: "light", dark: false, color: "#f6f6f7" });
  });

  // Store Ops' tablet, the pre-paint ahead of its pin script: the body's ground was computed dark
  // before the first frame, so without this the switch to light eased in, one dark frame on screen.
  it("lands a mode it changes on the first frame with every transition off, then puts them back", async () => {
    const pin =
      '<script>document.documentElement.setAttribute("data-oxy-mode-pin", "light");</script>';
    const page = open({ themed: true, person: "dark", after: pin });
    const seen: string[] = [];
    new page.w.MutationObserver((list) => {
      for (const m of list) {
        for (const n of m.addedNodes) seen.push(`+${n.textContent}`);
        for (const n of m.removedNodes) seen.push(`-${n.textContent}`);
      }
    }).observe(page.w.document.head, { childList: true });
    expect(await page.firstFrame()).toMatchObject({ mode: "light", dark: false });
    const still = "*,*::before,*::after{transition:none!important}";
    expect(seen).toEqual([`+${still}`, `-${still}`]);
    expect(page.w.document.head.querySelectorAll("style").length).toBe(1);
  });

  it("leaves transitions alone when the first frame changes nothing", async () => {
    const page = open({ themed: true, person: "dark" });
    let changes = 0;
    new page.w.MutationObserver(() => changes++).observe(page.w.document.head, { childList: true });
    await page.firstFrame();
    expect(changes).toBe(0);
  });

  it("still lets a pin in index.html win at once", async () => {
    const page = open({ themed: true, person: "dark", html: ' data-oxy-mode-pin="light"' });
    expect(page.state()).toMatchObject({ mode: "light", dark: false });
    expect(await page.firstFrame()).toEqual({ mode: "light", dark: false, color: "#f6f6f7" });
  });

  it("shows a staging preview's own mode once __OXY_APP__, spliced in after it, says staging", async () => {
    const page = open({
      themed: true,
      person: "dark",
      host: "staging--acme--ops.oxygen-hq.com",
      environment: "staging",
      search: "?oxy-mode=light"
    });
    expect(await page.firstFrame()).toEqual({ mode: "light", dark: false, color: "#f6f6f7" });
    expect(page.w.sessionStorage.getItem("oxy-mode")).toBe("light");
  });

  it("leaves the page as it is before anyone chose, theme-color on the light ground", async () => {
    const page = open({ themed: true });
    expect(await page.firstFrame()).toEqual({ mode: null, dark: false, color: "#f6f6f7" });
  });

  it("keeps a mode someone else set after it, and theme-color follows that one", async () => {
    const other =
      '<script>document.documentElement.setAttribute("data-oxy-mode", "light"); document.documentElement.classList.remove("dark");</script>';
    const page = open({ themed: true, person: "dark", after: other });
    expect(await page.firstFrame()).toEqual({ mode: "light", dark: false, color: "#f6f6f7" });
  });

  it("without theme.css, takes the mode back off by the first frame and leaves theme-color alone", async () => {
    const page = open({ themed: false, person: "dark" });
    expect(page.state()).toEqual({ mode: "dark", dark: true, color: "#ffffff" });
    expect(await page.firstFrame()).toEqual({ mode: null, dark: false, color: "#ffffff" });
  });

  it("without theme.css, gives an app's own .dark back", async () => {
    const page = open({ themed: false, person: "light", html: ' class="dark"' });
    expect(page.state()).toMatchObject({ mode: "light", dark: false });
    expect(await page.firstFrame()).toEqual({ mode: null, dark: true, color: "#ffffff" });
  });

  // A browser that runs a frame before a stylesheet in <head> has loaded must not be read as a
  // page without theme.css: the pre-paint waits while a linked sheet is still loading.
  describe("a linked stylesheet still loading", () => {
    const fake = (loads: "theme" | "other") => {
      const frames: (() => void)[] = [];
      const attrs = new Map<string, string>();
      const classes = new Set<string>();
      const link: { sheet: unknown; setAttribute(): void } = { sheet: null, setAttribute() {} };
      let color: string | null = null;
      const meta = { setAttribute: (_: string, v: string) => (color = v) };
      let contract = "";
      const root = {
        setAttribute: (n: string, v: string) => void attrs.set(n, v),
        removeAttribute: (n: string) => void attrs.delete(n),
        getAttribute: (n: string) => attrs.get(n) ?? null,
        classList: {
          toggle(t: string, force?: boolean) {
            const on = force ?? !classes.has(t);
            if (on) classes.add(t);
            else classes.delete(t);
            return on;
          },
          contains: (t: string) => classes.has(t)
        }
      };
      const w: OxyThemeWindow = {
        __OXY_APP__: { environment: "production" },
        location: { hostname: "acme--ops.customer-apps.oxygen-hq.com", search: "" },
        localStorage: new MemoryStorage({ "oxy-person-mode": "dark" }),
        getComputedStyle: () => ({
          getPropertyValue: (n: string) =>
            n === "--oxy-theme-contract"
              ? contract
              : n === "--background"
                ? classes.has("dark")
                  ? "#0a0a0a"
                  : "#f6f6f7"
                : ""
        }),
        document: {
          documentElement: root,
          readyState: "interactive",
          querySelectorAll: (sel: string) => (sel.includes("stylesheet") ? [link] : [meta])
        },
        requestAnimationFrame: (cb: () => void) => frames.push(cb)
      };
      oxyThemeRuntime(["reference"]).prepaint(w);
      const shown = () => ({
        mode: attrs.get("data-oxy-mode") ?? null,
        dark: classes.has("dark"),
        color
      });
      const frame = () => frames.shift()?.();
      const load = () => {
        link.sheet = {};
        if (loads === "theme") contract = "1";
      };
      return { frames, shown, frame, load };
    };

    it("keeps the mode on and waits, then settles once theme.css has loaded", () => {
      const page = fake("theme");
      expect(page.shown()).toEqual({ mode: "dark", dark: true, color: null });
      page.frame();
      expect(page.shown()).toEqual({ mode: "dark", dark: true, color: null });
      expect(page.frames).toHaveLength(1);
      page.load();
      page.frame();
      expect(page.shown()).toEqual({ mode: "dark", dark: true, color: "#0a0a0a" });
      expect(page.frames).toHaveLength(0);
    });

    it("takes the mode back off once the sheet has loaded without theme.css", () => {
      const page = fake("other");
      page.frame();
      expect(page.shown().mode).toBe("dark");
      page.load();
      page.frame();
      expect(page.shown()).toEqual({ mode: null, dark: false, color: null });
    });
  });
});
