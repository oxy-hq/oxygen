/**
 * `oxyc home`, driven through the real binary against a fake deployment.
 *
 * A LOCAL SERVER, for `experiments.test.ts`'s reason: what is under test is the
 * wire contract with the home routes (the paths, the revision a write carries,
 * what a 409 and a 422 become) and the text an agent reads, not the server.
 */

import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer, type Server, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { HomeDescription } from "@oxy-hq/sdk/manifest";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { ExitCode } from "../util/errors.js";
import type { WidgetsFeed } from "./home-client.js";
import { renderHome } from "./home-render.js";

const BIN = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "dist", "main.mjs");
const SCRATCH: string[] = [];
afterAll(() => {
  for (const dir of SCRATCH) rmSync(dir, { recursive: true, force: true });
});

const ORG_ID = "0b6f3c5e-2d52-4a53-9d1c-6f1f5a1d9c11";
const HOME = `/api/orgs/${ORG_ID}/home`;
const LAYOUT = `${HOME}/layout`;
const FEED = "/api/home/widgets?org=poke-house";

const EFFECTIVE = {
  version: 1,
  sections: [
    {
      kind: "widgets",
      items: [
        { widget: "bookkeeping/close", size: "full" },
        { widget: "store-ops/today", size: "regular" }
      ],
      others: "hide"
    },
    { kind: "apps" },
    { kind: "insights" }
  ]
};

function description(revision = 3): HomeDescription {
  return {
    org: { id: ORG_ID, slug: "poke-house", name: "Poke House" },
    revision,
    customized: revision > 0,
    layout: revision > 0 ? (EFFECTIVE as HomeDescription["layout"]) : null,
    effective_layout: EFFECTIVE as HomeDescription["effective_layout"],
    sections: [
      {
        kind: "widgets",
        title: null,
        items: [
          {
            widget: "bookkeeping/close",
            key: "bookkeeping/close",
            id: null,
            app: { id: "a1", slug: "bookkeeping", name: "Bookkeeping" },
            title: "Month-end close",
            size: "full",
            sizes: ["regular", "wide", "full"],
            params: {}
          },
          {
            widget: "store-ops/today",
            key: "store-ops/today",
            id: null,
            app: { id: "a2", slug: "store-ops", name: "Store ops" },
            title: "Today",
            size: "regular",
            sizes: ["regular"],
            params: {}
          }
        ]
      },
      { kind: "apps" },
      { kind: "insights" }
    ],
    catalog: [
      {
        widget: "warehouse/receiving",
        title: "Receiving",
        size: "regular",
        sizes: ["regular"],
        params: null,
        needs_params: false,
        shown: false
      },
      {
        widget: "bookkeeping/close",
        title: "Month-end close",
        size: "wide",
        sizes: ["regular", "wide", "full"],
        params: null,
        needs_params: false,
        shown: true
      }
    ]
  };
}

const app = (id: string, slug: string, name: string) => ({
  id,
  slug,
  name,
  org_slug: "poke-house",
  url: `https://poke-house.example/${slug}/`
});

const FEED_BODY: WidgetsFeed = {
  widgets: [
    {
      app: app("a1", "bookkeeping", "Bookkeeping"),
      id: "close",
      key: "bookkeeping/close",
      instance: null,
      params: {},
      title: "Month-end close",
      size: "full",
      sizes: ["regular", "wide", "full"],
      view: {
        headline: "19 stores ready to post",
        detail: "September · 61 of 84 posted",
        level: "attention",
        progress: { done: 61, total: 84 },
        metric: { value: "$48,210", label: "Posted", delta: "+12%", trend: "up" },
        rows: [
          { title: "Valencia", detail: "Not mapped", state: "blocked" },
          { title: "Mission", state: "ready" },
          { title: "Marina", detail: "Posted", state: "done" },
          { title: "Dogpatch", state: "pending" }
        ],
        buttons: [{ label: "Post 19 ready" }]
      }
    },
    {
      app: app("a2", "store-ops", "Store ops"),
      id: "today",
      key: "store-ops/today",
      instance: null,
      params: {},
      title: "Today",
      size: "regular",
      view: null,
      error: "function timed out"
    }
  ],
  sections: [
    { kind: "widgets", title: null, items: ["bookkeeping/close", "store-ops/today"] },
    { kind: "apps" },
    { kind: "insights" }
  ]
};

let server: Server;
let base: string;
let requests: Array<{ method: string; url: string; body: string }> = [];
let revision = 3;
let putStatus = 200;
let putBody: unknown = null;

beforeEach(() => {
  requests = [];
  revision = 3;
  putStatus = 200;
  putBody = null;
});

function json(res: ServerResponse, value: unknown, status = 200): void {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(JSON.stringify(value));
}

function route(method: string, url: string, body: string, res: ServerResponse) {
  if (method === "GET" && url === "/api/orgs") {
    return json(res, [{ id: ORG_ID, slug: "poke-house", name: "Poke House" }]);
  }
  if (method === "GET" && url === HOME) return json(res, description(revision));
  if (method === "GET" && url === FEED) return json(res, FEED_BODY);
  if (method === "PUT" && url === LAYOUT) {
    if (putStatus !== 200) return json(res, putBody, putStatus);
    const sent = JSON.parse(body) as { dry_run?: boolean };
    return json(res, description(sent.dry_run ? revision : revision + 1));
  }
  if (method === "DELETE" && url === LAYOUT) return json(res, description(0));
  res.writeHead(404);
  res.end("{}");
}

beforeAll(async () => {
  server = createServer((req, res) => {
    let body = "";
    req.on("data", (c) => {
      body += c;
    });
    req.on("end", () => {
      const url = req.url ?? "";
      const method = req.method ?? "";
      requests.push({ method, url, body });
      route(method, url, body, res);
    });
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});
afterAll(() => server.close());

function scratch(): string {
  const dir = mkdtempSync(join(tmpdir(), "oxyc-home-"));
  SCRATCH.push(dir);
  return dir;
}

function oxyc(...args: string[]): Promise<{ status: number; stdout: string; stderr: string }> {
  if (!existsSync(BIN)) throw new Error(`${BIN} missing — run \`pnpm build\``);
  const home = scratch();
  return new Promise((done) => {
    const child = spawn(process.execPath, [BIN, ...args, "--target", base], {
      env: {
        ...process.env,
        HOME: home,
        OXY_CREDENTIALS_PATH: join(home, "credentials.json"),
        OXYC_CACHE_DIR: join(home, "cache"),
        OXY_TOKEN: "test-token",
        NO_COLOR: "1"
      }
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (d) => {
      stdout += d;
    });
    child.stderr.on("data", (d) => {
      stderr += d;
    });
    child.on("close", (status) => done({ status: status ?? -1, stdout, stderr }));
  });
}

function layoutFile(content: string, name = "layout.json"): string {
  const file = join(scratch(), name);
  writeFileSync(file, content);
  return file;
}

const sent = () => requests.map((q) => `${q.method} ${q.url}`);
const put = () => requests.find((q) => q.method === "PUT");

describe("renderHome", () => {
  it("draws sections in order, each widget with its headline, level, progress, metric and 3 rows", () => {
    const text = renderHome(description(), FEED_BODY);
    expect(text).toContain("Poke House (poke-house) home — customized, revision 3");
    // A phone's order: the apps, Today, then insights.
    expect(text.indexOf("Your apps")).toBeLessThan(text.indexOf("Today"));
    expect(text.indexOf("Today")).toBeLessThan(text.indexOf("Insights"));
    expect(text).toContain("bookkeeping/close · Month-end close · full");
    expect(text).toContain("19 stores ready to post [attention]");
    expect(text).toContain("September · 61 of 84 posted");
    expect(text).toContain("progress 61/84");
    expect(text).toContain("metric $48,210 · Posted · +12% · up");
    expect(text).toContain("- [blocked] Valencia — Not mapped");
    expect(text).toContain("- [ready] Mission");
    expect(text).toContain("- [done] Marina — Posted");
    expect(text).not.toContain("Dogpatch");
    expect(text).toContain("… 1 more");
    expect(text).toContain("buttons: Post 19 ready");
  });

  it("says what a widget that failed or returned nothing shows", () => {
    const text = renderHome(description(), FEED_BODY);
    expect(text).toContain("store-ops/today · Today · regular");
    expect(text).toContain("error: function timed out");
    expect(renderHome(description(), undefined)).toContain("(no view)");
  });

  it("lists the widgets that are not shown at the end", () => {
    const text = renderHome(description(), FEED_BODY);
    expect(text.trimEnd().split("\n").pop()).toBe("not shown: warehouse/receiving (Receiving)");
  });

  it("says a preview has no live view for a widget the saved layout does not draw", () => {
    expect(renderHome(description(), undefined, { previewing: true })).toContain(
      "(no live view until it is saved)"
    );
  });
});

/** Two titled groups, one widget placed twice, and a widget that needs settings. */
function storesHome(): HomeDescription {
  const base = description();
  const first = base.sections[0];
  const close = first?.kind === "widgets" ? first.items[0] : undefined;
  const app = { id: "a2", slug: "store-ops", name: "Store ops" };
  const copy = (id: string, title: string, store: string) => ({
    widget: "store-ops/today",
    key: `store-ops/today#${id}`,
    id,
    app,
    title,
    size: "regular" as const,
    sizes: ["regular" as const],
    params: { store }
  });
  return {
    ...base,
    sections: [
      {
        kind: "widgets",
        title: "Finance",
        items: close ? [close] : []
      },
      {
        kind: "widgets",
        title: "Stores",
        items: [copy("mission", "Mission St", "mission"), copy("valencia", "Valencia", "valencia")]
      },
      { kind: "apps" }
    ],
    catalog: [
      {
        widget: "store-ops/today",
        title: "Today",
        size: "regular",
        sizes: ["regular"],
        params: {
          type: "object",
          properties: { store: { type: "string" }, late_after: { type: "integer", default: 60 } },
          required: ["store", "late_after"]
        },
        needs_params: true,
        shown: true
      },
      {
        widget: "store-ops/week",
        title: "Week",
        size: "wide",
        sizes: ["wide"],
        params: {
          type: "object",
          properties: { store: { type: "string" }, late_after: { type: "integer", default: 60 } },
          required: ["store", "late_after"]
        },
        needs_params: true,
        shown: false
      },
      ...base.catalog.filter((c) => !c.shown)
    ]
  };
}

const storesFeed = (): WidgetsFeed => {
  const card = (id: string, title: string, store: string, headline: string, refresh?: number) => ({
    app: app("a2", "store-ops", "Store ops"),
    id: "today",
    key: `store-ops/today#${id}`,
    instance: id,
    params: { store },
    title,
    size: "regular",
    view: { headline, level: "ok", refresh_after: refresh }
  });
  return {
    widgets: [
      FEED_BODY.widgets[0] as WidgetsFeed["widgets"][number],
      card("mission", "Mission St", "mission", "12 open orders", 300),
      card("valencia", "Valencia", "valencia", "3 open orders")
    ],
    sections: [
      { kind: "widgets", title: "Finance", items: ["bookkeeping/close"] },
      {
        kind: "widgets",
        title: "Stores",
        items: ["store-ops/today#mission", "store-ops/today#valencia"]
      },
      { kind: "apps" }
    ]
  };
};

describe("renderHome folds calm widgets", () => {
  it("lists a calm widget on one line after the cards with something to do", () => {
    const feed = storesFeed();
    const loud = feed.widgets[1];
    if (loud?.view)
      loud.view = {
        ...loud.view,
        level: "attention",
        rows: [{ title: "Walk-in", state: "failed" }]
      };
    const text = renderHome(storesHome(), feed);
    const stores = text.slice(text.indexOf("Stores"));
    expect(stores).toContain("store-ops/today#mission · Mission St · regular");
    expect(stores).toContain("- [failed] Walk-in");
    // Valencia is calm: its headline only.
    expect(stores).toContain(
      "store-ops/today#valencia · Valencia · compact\n    params: store=valencia\n    3 open orders [ok]"
    );
    expect(stores.indexOf("#mission")).toBeLessThan(stores.indexOf("#valencia"));
  });

  it("keeps a calm-looking widget whole when it offers a button", () => {
    const feed = storesFeed();
    const card = feed.widgets[2];
    if (card?.view) card.view = { ...card.view, buttons: [{ label: "Remind" }] };
    const text = renderHome(storesHome(), feed);
    expect(text).toContain("store-ops/today#valencia · Valencia · regular");
    // The calm Mission folds and moves after Valencia, though the layout lists it first.
    expect(text.indexOf("#mission")).toBeGreaterThan(text.indexOf("#valencia"));
  });
});

describe("renderHome with groups, copies and settings", () => {
  const text = renderHome(storesHome(), storesFeed());
  const lines = text.split("\n");

  it("titles a group with something to do, and lists calm widgets under All clear", () => {
    expect(lines).toContain("Finance");
    // Both Stores copies are calm, so the group has no heading of its own.
    expect(lines).not.toContain("Stores");
    expect(lines).toContain("All clear");
    expect(text.indexOf("Finance")).toBeLessThan(text.indexOf("All clear"));
    expect(text.indexOf("Your apps")).toBeLessThan(text.indexOf("Finance"));
    // An untitled group's widgets sit straight under Today.
    expect(renderHome(description(), FEED_BODY).split("\n")).not.toContain("Widgets");
  });

  it("draws a widget placed twice as two copies, each with its id, title, settings and view", () => {
    // Both copies are calm (ok, no button), so they are folded, as the home draws them.
    expect(text).toContain("store-ops/today#mission · Mission St · compact");
    expect(text).toContain("store-ops/today#valencia · Valencia · compact");
    expect(text).toContain("  params: store=mission");
    expect(text).toContain("  params: store=valencia");
    expect(text).toContain("12 open orders [ok]");
    expect(text).toContain("3 open orders [ok]");
    // A widget with no settings has no params line.
    expect(text.slice(text.indexOf("bookkeeping/close"), text.indexOf("All clear"))).not.toContain(
      "params:"
    );
  });

  it("says how often a view asks to be read again, only when it does", () => {
    expect(text).toContain("  refreshes every 5m");
    expect(text.match(/refreshes every/g)).toHaveLength(1);
  });

  it("lists a widget that needs settings and is not placed, and what it needs", () => {
    const last = text.trimEnd().split("\n").pop() ?? "";
    expect(last).toBe(
      "not shown: store-ops/week (Week, needs settings: store), warehouse/receiving (Receiving)"
    );
  });
});

describe("home show", () => {
  it("combines the description and the widgets feed, resolving the slug to an id", async () => {
    const r = await oxyc("home", "show", "--org", "poke-house");
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual(["GET /api/orgs", `GET ${HOME}`, `GET ${FEED}`]);
    expect(r.stdout).toContain("bookkeeping/close · Month-end close · full");
    expect(r.stdout).toContain("19 stores ready to post [attention]");
    expect(r.stdout).toContain("not shown: warehouse/receiving (Receiving)");
  });

  it("--json is both answers, raw", async () => {
    const r = await oxyc("home", "show", "--org", "poke-house", "--json");
    expect(r.status, r.stderr).toBe(0);
    const out = JSON.parse(r.stdout);
    expect(out.home.revision).toBe(3);
    expect(out.widgets.widgets).toHaveLength(2);
  });

  it("needs an org", async () => {
    const r = await oxyc("home", "show");
    expect(r.status).toBe(ExitCode.USAGE);
    expect(requests).toEqual([]);
  });
});

describe("home get", () => {
  it("prints the effective layout with the revision it was read at, alone on stdout", async () => {
    const r = await oxyc("home", "get", "--org", "poke-house");
    expect(r.status, r.stderr).toBe(0);
    expect(JSON.parse(r.stdout)).toEqual({ revision: 3, layout: EFFECTIVE });
  });
});

/** What `home get` prints, at `rev`. */
const gotFile = (rev = 3) => layoutFile(JSON.stringify({ revision: rev, layout: EFFECTIVE }));

describe("home apply", () => {
  it("sends the revision from the file `get` printed, not one read now", async () => {
    // The layout moved on to revision 3 after this file was read at 2: the write
    // must carry 2, so the server refuses it instead of overwriting unseen.
    const r = await oxyc("home", "apply", "--org", "poke-house", "-f", gotFile(2));
    expect(sent()).toEqual(["GET /api/orgs", `PUT ${LAYOUT}`]);
    expect(JSON.parse(put()?.body ?? "")).toEqual({ layout: EFFECTIVE, revision: 2 });
    expect(r.status, r.stderr).toBe(0);
  });

  it("saves, and says the new revision", async () => {
    const r = await oxyc("home", "apply", "--org", "poke-house", "-f", gotFile());
    expect(r.status, r.stderr).toBe(0);
    expect(JSON.parse(put()?.body ?? "")).toEqual({ layout: EFFECTIVE, revision: 3 });
    expect(r.stderr).toContain("saved the home layout (revision 4)");
    expect(r.stdout).toBe("");
  });

  it("takes a bare layout with --revision, and reads YAML as well as JSON", async () => {
    const file = layoutFile("version: 1\nsections:\n  - kind: apps\n", "layout.yml");
    const r = await oxyc("home", "apply", "--org", "poke-house", "-f", file, "--revision", "5");
    expect(JSON.parse(put()?.body ?? "").revision).toBe(5);
    expect(r.status, r.stderr).toBe(0);
    expect(JSON.parse(put()?.body ?? "").layout).toEqual({
      version: 1,
      sections: [{ kind: "apps" }]
    });
  });

  it("refuses a bare layout with no revision unless forced, sending nothing", async () => {
    const r = await oxyc(
      "home",
      "apply",
      "--org",
      "poke-house",
      "-f",
      layoutFile(JSON.stringify(EFFECTIVE))
    );
    expect(r.status).toBe(ExitCode.USAGE);
    expect(r.stderr).toContain("no revision to send");
    expect(requests).toEqual([]);
  });

  it("refuses a revision that is not a whole number, sending nothing", async () => {
    for (const bad of ["abc", "", "1.5", "-1"]) {
      const r = await oxyc(
        "home",
        "apply",
        "--org",
        "poke-house",
        "-f",
        gotFile(),
        "--revision",
        bad
      );
      expect(r.status, bad).toBe(ExitCode.USAGE);
      expect(r.stderr).toContain("must be a whole number");
    }
    const inFile = layoutFile(JSON.stringify({ revision: "three", layout: EFFECTIVE }));
    expect((await oxyc("home", "apply", "--org", "poke-house", "-f", inFile)).status).toBe(
      ExitCode.USAGE
    );
    expect(requests).toEqual([]);
  });

  it("--force sends no revision", async () => {
    const r = await oxyc("home", "apply", "--org", "poke-house", "-f", gotFile(), "--force");
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual(["GET /api/orgs", `PUT ${LAYOUT}`]);
    expect(JSON.parse(put()?.body ?? "")).toEqual({ layout: EFFECTIVE });
  });

  it("--dry-run asks for a dry run and prints the result as text, saving nothing", async () => {
    const r = await oxyc("home", "apply", "--org", "poke-house", "-f", gotFile(), "--dry-run");
    expect(r.status, r.stderr).toBe(0);
    expect(JSON.parse(put()?.body ?? "")).toEqual({
      layout: EFFECTIVE,
      revision: 3,
      dry_run: true
    });
    expect(r.stdout).toContain("— preview");
    expect(r.stdout).toContain("bookkeeping/close · Month-end close · full");
    expect(r.stdout).toContain("19 stores ready to post [attention]");
    expect(r.stderr).toContain("dry run: nothing was saved");
  });

  it("prints every 422 problem as path: message (fix) and exits with the request code", async () => {
    putStatus = 422;
    putBody = {
      error: "layout_invalid",
      problems: [
        {
          path: "sections[0].items[0].size",
          message: 'bookkeeping/close does not allow "tall"',
          fix: "Use one of: regular, wide, full."
        },
        {
          path: "sections[0].items[2].widget",
          message: "no widget named warehouse/recieving",
          fix: "Did you mean warehouse/receiving?"
        }
      ]
    };
    const r = await oxyc("home", "apply", "--org", "poke-house", "-f", gotFile());
    expect(r.status).toBe(ExitCode.REQUEST);
    expect(r.stderr).toContain("2 problems");
    expect(r.stderr).toContain(
      'sections[0].items[0].size: bookkeeping/close does not allow "tall" (Use one of: regular, wide, full.)'
    );
    expect(r.stderr).toContain(
      "sections[0].items[2].widget: no widget named warehouse/recieving (Did you mean warehouse/receiving?)"
    );
    expect(r.stdout).toBe("");
  });

  it("says someone changed the layout on a 409, and exits 8", async () => {
    putStatus = 409;
    putBody = { error: "stale_revision" };
    const r = await oxyc("home", "apply", "--org", "poke-house", "-f", gotFile());
    expect(r.status).toBe(ExitCode.REFUSED);
    expect(r.stderr).toContain("the home layout changed since it was read");
    expect(r.stderr).toContain("--force");
  });

  it("needs a file, and a file that holds an object", async () => {
    const none = await oxyc("home", "apply", "--org", "poke-house");
    expect(none.status).toBe(ExitCode.USAGE);
    const list = await oxyc("home", "apply", "--org", "poke-house", "-f", layoutFile("[1, 2]"));
    expect(list.status).toBe(ExitCode.USAGE);
    expect(requests).toEqual([]);
  });

  it("maps a 403 to the auth exit code", async () => {
    putStatus = 403;
    putBody = { error: "forbidden" };
    const r = await oxyc("home", "apply", "--org", "poke-house", "-f", gotFile());
    expect(r.status).toBe(ExitCode.AUTH);
    expect(r.stderr).toContain("org owner or admin");
  });
});

describe("home reset", () => {
  it("deletes the layout", async () => {
    const r = await oxyc("home", "reset", "--org", "poke-house");
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual(["GET /api/orgs", `DELETE ${LAYOUT}`]);
    expect(r.stderr).toContain("back to the default layout");
  });
});
