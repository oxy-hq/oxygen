import { describe, expect, it } from "vitest";
import { sentryEnvironment, sentryIntegrations, sentryOptions } from "./sentry";

// Sentry records errors only: internal-docs/2026-09-15-custom-app-guard-sentry-and-data-shapes-design.md §2, §4.1.
describe("sentry", () => {
  it("installs only error integrations: no tracing, replay, console logs or reporting observer", () => {
    const names = sentryIntegrations().map((integration) => integration.name);

    expect(names).toEqual(["LinkedErrors", "CaptureConsole"]);
    for (const banned of ["BrowserTracing", "Replay", "ConsoleLogs", "ReportingObserver"]) {
      expect(names).not.toContain(banned);
    }
  });

  it("sets no trace or replay sampling, since any tracesSampleRate (0 included) turns tracing on", () => {
    const options = sentryOptions("https://public@o0.ingest.sentry.io/0", "app.oxygen-hq.com");

    const forbidden = [
      "tracesSampleRate",
      "tracesSampler",
      "replaysSessionSampleRate",
      "replaysOnErrorSampleRate",
      "enableLogs"
    ];
    for (const key of forbidden) {
      expect(options).not.toHaveProperty(key);
    }
    expect(options.integrations).toHaveLength(2);
  });
});

// Hosts from oxy-hq/infrastructure: the dev and staging values (`aip.*`) and
// org-subdomain ingresses (`*.dev.oxy.tech`, `*.staging.oxy.tech`), and prod's
// values (`app.oxygen-hq.com`, `app.oxy.tech`) and org-subdomain ingress.
describe("sentryEnvironment", () => {
  it.each([
    ["aip.staging.oxy.tech", "staging"],
    ["pokehouse.staging.oxy.tech", "staging"],
    ["aip.dev.oxy.tech", "dev"],
    ["pokehouse.dev.oxy.tech", "dev"],
    ["app.oxygen-hq.com", "production"],
    ["pokehouse.oxygen-hq.com", "production"],
    ["app.oxy.tech", "production"],
    ["staging.oxy.tech.example.com", "production"],
    ["localhost", "production"]
  ])("maps %s to %s", (hostname, environment) => {
    expect(sentryEnvironment(hostname, undefined)).toBe(environment);
  });

  it("lets a non-empty VITE_SENTRY_ENV override the host", () => {
    expect(sentryEnvironment("aip.staging.oxy.tech", "development")).toBe("development");
    expect(sentryEnvironment("aip.dev.oxy.tech", "")).toBe("dev");
  });
});
