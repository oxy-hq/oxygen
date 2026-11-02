import type { AppVerdict, CustomAppSummary } from "@/types/apps";

export type AlertSeverity = "critical" | "warning" | "info";

/** Where a signal is addressed: a custom app, or Oxygen Factory (the system
 *  layer — for data/source/pipeline issues). */
type SignalTarget = { app: string } | { core: true };

/** An unresolved signal — the shape a real monitors/anomalies feed will emit,
 *  and the shape the demo set conforms to. `useHqAlerts` resolves the `target`
 *  into a concrete destination. */
export interface HqSignalSeed {
  id: string;
  severity: AlertSeverity;
  /** Short signal category shown before the message (e.g. "Labor risk"). */
  category: string;
  title: string;
  target: SignalTarget;
}

/** What a verdict says about an app that needs someone. `null` is "no signal",
 *  which is **not** a claim that the app is healthy — the launcher renders no
 *  green tick for an absence, and an absence of signal is all `quiet`,
 *  `operational`, `draft` and "the server did not say" produce. */
const SIGNAL_FOR_VERDICT: Partial<
  Record<AppVerdict, { severity: AlertSeverity; category: string; title: string }>
> = {
  down: { severity: "critical", category: "App down", title: "requests are failing" },
  degraded: { severity: "warning", category: "App degraded", title: "failing more than it should" },
  // The same three verdicts the backend's `AppHealth::needs_attention` names. Not
  // knowing is more actionable than knowing an app is idle, so an unmeasured app is
  // surfaced — quietly (`info`), because it is a gap in monitoring rather than an
  // outage, and never as healthy.
  not_measured: {
    severity: "info",
    category: "Not measured",
    title: "nothing is watching this app's health"
  }
};

/** Health signals from the server's per-app verdict (`GET /{workspace}/custom-apps`).
 *  Pure, so the product rules are testable without a window: a verdict the server
 *  did not send reads as no signal, never as a good one. */
export function healthSeeds(apps: readonly CustomAppSummary[]): HqSignalSeed[] {
  return apps.flatMap((app): HqSignalSeed[] => {
    const signal = app.verdict ? SIGNAL_FOR_VERDICT[app.verdict] : undefined;
    return signal
      ? [
          {
            ...signal,
            id: `health-${app.slug}`,
            title: `${app.name} ${signal.title}`,
            target: { app: app.slug }
          }
        ]
      : [];
  });
}
