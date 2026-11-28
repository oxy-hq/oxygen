import { existsSync, readFileSync } from "node:fs";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { parse } from "yaml";
import {
  ensureFixtureServer,
  ensureFixtureServerFor,
  FIXTURE_FLOWS,
  FIXTURE_HOST,
  FIXTURE_PORT,
  FIXTURE_ROUTES
} from "./fixture-server";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "..", "..");

// Port 0: the OS picks a free one, so a parallel test run cannot collide.
describe("fixture server", () => {
  it("answers the Open-Meteo forecast route with the committed fixture, ignoring the query", async () => {
    const fx = await ensureFixtureServer(0);
    try {
      const res = await fetch(`${fx.url}/open-meteo/v1/forecast?latitude=1&longitude=2&daily=x`);
      expect(res.status).toBe(200);
      const body = (await res.json()) as {
        daily: { time: string[]; temperature_2m_max: number[] };
      };
      expect(body.daily.time.length).toBeGreaterThan(0);
      expect(body.daily.temperature_2m_max).toHaveLength(body.daily.time.length);
    } finally {
      await fx.shutdown();
    }
  });

  it("refuses unknown routes and non-GET methods", async () => {
    const fx = await ensureFixtureServer(0);
    try {
      expect((await fetch(`${fx.url}/elsewhere`)).status).toBe(404);
      expect((await fetch(`${fx.url}/open-meteo/v1/forecast`, { method: "POST" })).status).toBe(
        405
      );
    } finally {
      await fx.shutdown();
    }
  });

  it("throws when the port is held by something else", async () => {
    const squatter = createServer((_req, res) => res.end("not the fixture"));
    await new Promise<void>((ok) => squatter.listen(0, FIXTURE_HOST, () => ok()));
    const { port } = squatter.address() as AddressInfo;
    try {
      await expect(ensureFixtureServer(port)).rejects.toThrow(/in use/);
    } finally {
      await new Promise<void>((ok) => squatter.close(() => ok()));
    }
  });

  it("refuses a port another fixture server already holds, rather than sharing it", async () => {
    const first = await ensureFixtureServer(0);
    try {
      await expect(ensureFixtureServer(Number(new URL(first.url).port))).rejects.toThrow(/in use/);
    } finally {
      await first.shutdown();
    }
  });

  // A second agent session on the same machine must be able to run any other
  // flow while one of these holds the port. Proven by holding the port: if
  // the call bound anything it would throw EADDRINUSE.
  it("binds nothing when no selected flow needs a fixture route", async () => {
    const holder = await ensureFixtureServer(0);
    const port = Number(new URL(holder.url).port);
    try {
      const fx = await ensureFixtureServerFor(
        [
          { file: "flows/chat-ask.flow.test.yml" },
          { file: "flows/semantic-monitors-inbox.flow.test.yml" }
        ],
        port
      );
      expect(fx.url).toBeNull();
      await fx.shutdown();
      await expect(
        ensureFixtureServerFor([{ file: "flows/airway-pipeline-run.flow.test.yml" }], port)
      ).rejects.toThrow(/in use/);
    } finally {
      await holder.shutdown();
    }
  });

  it("starts the responder when a selected flow needs it", async () => {
    const fx = await ensureFixtureServerFor(
      [{ file: "/abs/web-app/tests/agentic/flows/airway-pipeline-run.flow.test.yml" }],
      0
    );
    try {
      expect(fx.url).not.toBeNull();
      expect((await fetch(`${fx.url}/open-meteo/v1/forecast`)).status).toBe(200);
    } finally {
      await fx.shutdown();
    }
  });

  it("names only flows that exist", () => {
    for (const stem of FIXTURE_FLOWS) {
      expect(
        existsSync(resolve(REPO_ROOT, `web-app/tests/agentic/flows/${stem}.flow.test.yml`))
      ).toBe(true);
    }
  });

  // The pipeline file cannot interpolate a port, so the constant and the YAML
  // have to agree by hand; this is what keeps them agreeing.
  it("is where weather_fixture.airway.yml looks for it", () => {
    const spec = parse(
      readFileSync(resolve(REPO_ROOT, "examples/pipelines/weather_fixture.airway.yml"), "utf8")
    );
    const base = new URL(spec.source.config.base_url);
    expect(base.hostname).toBe(FIXTURE_HOST);
    expect(Number(base.port)).toBe(FIXTURE_PORT);
    expect(Object.keys(FIXTURE_ROUTES)).toContain(`${base.pathname.replace(/\/$/, "")}/forecast`);
    expect(spec.source.kind).toBe("weather");
    expect(spec.destination.kind).toBe("memory");
  });
});
