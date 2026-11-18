import { apiClient } from "./axios";

/**
 * The staff reads over an org's app events: the routes `oxyc events` calls
 * (`crates/api-custom-apps/src/app_events/api`). Field names are camelCase on the wire
 * because the CLI's requests are the contract; they are kept as-is here.
 *
 * Reads only. They take app-admin standing (staff with `manage_apps` over the org, or
 * its officers) and answer whether or not the org's `app_events` flag is on. The writes
 * (redeliver, replay, discard, purge) stay in `oxyc events`: the bulk redeliver selects
 * by listener and status across every environment, which a button on a page scoped to
 * one environment would misstate.
 */

/** One listener of one event type in one environment, read live from its deliveries. */
export interface AppEventListenerHealth {
  /** `<app>/<function>`. */
  listener: string;
  app: string;
  function: string;
  environment: string;
  pending: number;
  /** Pending with at least one attempt. */
  retrying: number;
  dead: number;
  held: number;
  /** Seconds the oldest pending delivery has waited (since created or redelivered). */
  oldestPendingAgeSeconds: number | null;
  lastSuccessAt: string | null;
  lastFailureAt: string | null;
  lastFailureOutcome: string | null;
  /** Attempts open on pending deliveries. */
  inFlight: number;
  /** The build the subscription follows. */
  buildId: string | null;
  reconciledAt: string | null;
}

export interface AppEventType {
  /** `<publisher>.<event>`. */
  type: string;
  /** The publishing app's slug. */
  publisher: string;
  event: string;
  schemaRevision: number | null;
  lastPublishedAt: string | null;
  listeners: AppEventListenerHealth[];
}

export interface AppEventTypes {
  types: AppEventType[];
}

export interface AppEventAttempt {
  attempt: number;
  /** `running` while open, else its outcome. */
  status: string;
  outcome: string | null;
  startedAt: string;
  finishedAt: string | null;
  error: string | null;
  invocationId: string | null;
}

export interface AppEventDeadDelivery {
  id: string;
  eventId: string;
  type: string;
  key: string;
  listener: string;
  attemptCount: number;
  lastError: string | null;
  diedAt: string;
  attempts: AppEventAttempt[];
}

/** Dead deliveries that died the same way: outcome plus the error's first line. */
export interface AppEventDeadGroup {
  signature: string;
  outcome: string;
  error: string;
  count: number;
  deliveries: AppEventDeadDelivery[];
}

export interface AppEventDead {
  environment: string;
  groups: AppEventDeadGroup[];
  /** More than the reply's limit died; the newest are listed. */
  truncated: boolean;
}

export interface AppEventFeedItem {
  kind: "published" | "delivery";
  at: string;
  eventId: string;
  type: string;
  key: string;
  environment: string;
  /** A delivery's: `<app>/<function>`. */
  listener?: string;
  status?: string;
  attemptCount?: number;
  lastError?: string;
}

export interface AppEventFeed {
  items: AppEventFeedItem[];
  cursor: string;
}

const enc = encodeURIComponent;
const envQuery = (environment: string) => `environment=${enc(environment)}`;

export class AppEventsService {
  /** Every event type of the org, with each listener's health per environment. */
  static async types(orgSlug: string): Promise<AppEventTypes> {
    const response = await apiClient.get<AppEventTypes>(
      `/customer-apps/${enc(orgSlug)}/events/types`
    );
    return response.data;
  }

  /** One app's dead deliveries in `environment`, grouped by how they died. */
  static async dead(orgSlug: string, appSlug: string, environment: string): Promise<AppEventDead> {
    const response = await apiClient.get<AppEventDead>(
      `/customer-apps/${enc(orgSlug)}/${enc(appSlug)}/events/dead?${envQuery(environment)}`
    );
    return response.data;
  }

  /** One app's publishes and its listeners' deliveries after `since`, oldest first. */
  static async feed(
    orgSlug: string,
    appSlug: string,
    environment: string,
    since: string
  ): Promise<AppEventFeed> {
    const response = await apiClient.get<AppEventFeed>(
      `/customer-apps/${enc(orgSlug)}/${enc(appSlug)}/events/feed?${envQuery(environment)}&since=${enc(since)}`
    );
    return response.data;
  }
}
