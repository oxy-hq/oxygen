// @vitest-environment jsdom
//
// `useQuery` hands its `params` to the server instead of writing them into
// the SQL: the request carries the template as the author wrote it and each
// value as the characters the user typed.

import * as React from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { _resetCustomAppManifestCacheForTest } from "./manifest";
import { __clearQueryCache } from "./query-cache";
import { OxyAppProvider, useQuery } from "./react";

const SQL = "SELECT * FROM t WHERE name = {{ params.name | sqlquote }}";

let container: HTMLDivElement;
let root: Root;
/** The JSON body of every request `useQuery` made. */
let sent: unknown[];

const fetcher = (async (_input: RequestInfo | URL, init?: RequestInit) => {
  if (typeof init?.body !== "string") throw new Error("useQuery sends a JSON string body");
  sent.push(JSON.parse(init.body));
  return { ok: true, status: 200, json: async () => ({ columns: ["n"], rows: [[1]] }) } as Response;
}) as typeof fetch;

beforeEach(() => {
  _resetCustomAppManifestCacheForTest();
  __clearQueryCache();
  sent = [];
  window.__OXY_APP__ = {
    appId: "app-uuid",
    slug: "test-app",
    orgId: "org-uuid",
    orgSlug: "acme",
    projectId: "proj-uuid",
    branch: "main",
    apiBaseUrl: ""
  };
  globalThis.fetch = (async () =>
    ({
      ok: true,
      status: 200,
      json: async () => ({
        schemaVersion: 2,
        name: "Test App",
        slug: "test-app",
        orgSlug: "acme",
        projectId: "proj-uuid"
      })
    }) as Response) as typeof fetch;
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(async () => {
  await React.act(async () => {
    root.unmount();
  });
  container.remove();
});

/** Mount `useQuery(SQL, { params: { name } })` and return a way to change `name`. */
async function mount(name: string): Promise<(next: string) => Promise<void>> {
  let drive: ((v: string) => void) | undefined;
  function Probe(): React.JSX.Element {
    const [value, setValue] = React.useState(name);
    drive = setValue;
    useQuery({ sql: SQL, database: "wh" }, { params: { name: value } });
    return <div />;
  }
  await React.act(async () => {
    root.render(
      <OxyAppProvider fetcher={fetcher}>
        <Probe />
      </OxyAppProvider>
    );
  });
  return async (next) => {
    await React.act(async () => {
      drive?.(next);
    });
  };
}

describe("useQuery params", () => {
  it("sends the template and the value, not SQL with the value written in", async () => {
    await mount("x\\' OR 1=1 -- ");
    expect(sent).toEqual([{ sql: SQL, database: "wh", params: { name: "x\\' OR 1=1 -- " } }]);
  });

  it("re-runs when a param changes, and not when it does not", async () => {
    const setName = await mount("west");
    await setName("west");
    expect(sent).toHaveLength(1);
    await setName("C:\\");
    expect(sent).toEqual([
      { sql: SQL, database: "wh", params: { name: "west" } },
      { sql: SQL, database: "wh", params: { name: "C:\\" } }
    ]);
  });
});
