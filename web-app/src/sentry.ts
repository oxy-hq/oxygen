import * as Sentry from "@sentry/react";

const SENTRY_DSN = import.meta.env.VITE_SENTRY_DSN;
const SENTRY_ENV = import.meta.env.VITE_SENTRY_ENV;
const SENTRY_RELEASE = import.meta.env.VITE_SENTRY_RELEASE || import.meta.env.VITE_APP_VERSION;

type SentryOptions = Parameters<typeof Sentry.init>[0];

function isDsnDefined(dsn: unknown): dsn is string {
  return typeof dsn === "string" && dsn.length > 0 && dsn !== '""';
}

// Sentry records errors only. None of these: browser tracing, session replay,
// console output shipped as logs, or the reporting observer (every browser report
// it sees is an `info` message). Nothing below `error` becomes an event; a
// `console.warn` still lands as a breadcrumb through the SDK's default breadcrumbs
// integration. `sentry.test.ts` pins this list by name.
export function sentryIntegrations() {
  return [
    Sentry.linkedErrorsIntegration({ limit: 5 }),
    Sentry.captureConsoleIntegration({ levels: ["error"] })
  ];
}

// Which deployment this page is on. One image serves staging first and prod
// after, so this is read from the page's host at runtime, not baked in at build
// time. Hosts are oxy-hq/infrastructure's ingresses: `aip.staging.oxy.tech` and
// `*.staging.oxy.tech` (org subdomains) are staging; `aip.dev.oxy.tech` and
// `*.dev.oxy.tech` are dev; everything else (app.oxygen-hq.com, *.oxygen-hq.com,
// app.oxy.tech) is production. A non-empty VITE_SENTRY_ENV (local dev) wins.
export function sentryEnvironment(hostname: string, override: string | undefined): string {
  if (override) return override;
  const host = hostname.toLowerCase();
  if (host === "staging.oxy.tech" || host.endsWith(".staging.oxy.tech")) return "staging";
  if (host === "dev.oxy.tech" || host.endsWith(".dev.oxy.tech")) return "dev";
  return "production";
}

const beforeSend: SentryOptions["beforeSend"] = (event) => {
  try {
    if (event?.request) {
      const req = event.request as { data?: unknown } & Record<string, unknown>;
      if (Object.hasOwn(req, "data")) {
        delete req.data;
      }
    }
  } catch {
    // ignore
  }

  try {
    const exception = event.exception;
    const values = exception?.values;
    if (exception && Array.isArray(values)) {
      exception.values = values.filter((v) => {
        const msg = v?.value ?? "";
        return !(typeof msg === "string" && msg.includes("ResizeObserver loop limit exceeded"));
      });
    }
  } catch {
    // ignore
  }

  return event;
};

// No `tracesSampleRate`, `tracesSampler` or `replays*SampleRate`, on purpose: the SDK
// treats tracing as on whenever `tracesSampleRate` is set at all, `0` included
// (`hasSpansEnabled` checks `!= null`).
export function sentryOptions(dsn: string, hostname: string): SentryOptions {
  return {
    dsn,
    environment: sentryEnvironment(hostname, SENTRY_ENV),
    release: SENTRY_RELEASE,
    integrations: sentryIntegrations(),
    beforeSend
  };
}

export function initSentry() {
  if (!isDsnDefined(SENTRY_DSN)) return;
  Sentry.init(sentryOptions(SENTRY_DSN, window.location.hostname));
}

// Re-export ErrorBoundary for convenience
export const ErrorBoundary = Sentry.ErrorBoundary;
