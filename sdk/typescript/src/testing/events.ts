// `createTestEvents()` — the recorder behind `createTestContext({ events })`.
//
// A publish in a test is recorded, never delivered: nothing here runs a
// listener. Ids are the platform's: UUIDv5 of
// `"{org}/{environment}/{type}/{key}"` under the platform's fixed
// event-id namespace.

import { createHash } from "node:crypto";
import type { OxyEvent } from "../custom-app/function-context";

/** The platform's event-id namespace; it never changes. */
const NAMESPACE = "6f0c5b1e-2d47-4a93-9e18-c4a7b35df812";

/** RFC 4122 UUIDv5 of `name` under `namespace`. */
function uuidV5(name: string, namespace: string): string {
  const ns = Buffer.from(namespace.replace(/-/g, ""), "hex");
  const h = createHash("sha1").update(ns).update(name, "utf8").digest();
  h[6] = (h[6] & 0x0f) | 0x50;
  h[8] = (h[8] & 0x3f) | 0x80;
  const hex = h.subarray(0, 16).toString("hex");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

/** The platform's deterministic event id (what the host computes). */
export function eventId(org: string, environment: string, type: string, key: string): string {
  return uuidV5(`${org}/${environment}/${type}/${key}`, NAMESPACE);
}

/** One publish a test context recorded. */
export interface PublishedEvent {
  type: string;
  data: unknown;
  key: string;
  delaySeconds: number;
}

export interface TestEvents {
  /** Every publish that took effect, in order (a rolled-back transaction's are absent). */
  readonly published: ReadonlyArray<PublishedEvent>;
  /** An event envelope for a listener test, with CloudEvents defaults and `overrides` on top. */
  envelope<T>(type: string, data: T, overrides?: Partial<OxyEvent<T>>): OxyEvent<T>;
}

const sinks = new WeakMap<TestEvents, PublishedEvent[]>();

export function createTestEvents(): TestEvents {
  const published: PublishedEvent[] = [];
  const events: TestEvents = {
    published,
    envelope<T>(type: string, data: T, overrides: Partial<OxyEvent<T>> = {}): OxyEvent<T> {
      const key = overrides.key ?? "test-key";
      const environment = overrides.environment ?? "production";
      const id = overrides.id ?? eventId("test-org", environment, type, key);
      const slug = type.includes("/") ? type.slice(0, type.indexOf("/")) : type;
      return {
        specversion: "1.0",
        id,
        type,
        source: `/apps/${slug}`,
        time: new Date(0).toISOString(),
        dataschema: null,
        data,
        key,
        environment,
        invocation: null,
        actor: null,
        causationid: null,
        rootid: id,
        depth: 0,
        ...overrides
      };
    }
  };
  sinks.set(events, published);
  return events;
}

/** Record a publish on `events`. Internal to the test context. */
export function recordPublish(events: TestEvents, event: PublishedEvent): void {
  const sink = sinks.get(events);
  if (!sink) throw new Error("@oxy-hq/sdk/testing: events was not made by createTestEvents()");
  sink.push(event);
}
