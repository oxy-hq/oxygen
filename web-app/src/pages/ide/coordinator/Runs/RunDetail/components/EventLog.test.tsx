// @vitest-environment jsdom

import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { RunEventEntry } from "@/services/api/coordinator";
import { EventLog } from "./EventLog";

// The event types are the backend's `PreaggEvent` (crates/agentic/automation/src/
// preagg_event.rs). Only `preagg_rollup_failed` and `preagg_refresh_key_error` are
// failures; every other outcome used to fall through to the red failed row.

afterEach(() => cleanup());

const event = (
  seq: number,
  event_type: string,
  payload: Record<string, unknown>
): RunEventEntry => ({ seq, event_type, payload });

const rollup = { view: "orders", rollup: "by_day" };

const failedRows = (container: HTMLElement) => container.querySelectorAll(".text-destructive");
const spinners = (container: HTMLElement) => container.querySelectorAll(".animate-spin");

describe("EventLog", () => {
  it("shows a retraction as a removal, not a failure, and settles its spinner", () => {
    const { container } = render(
      <EventLog
        events={[
          event(1, "preagg_rollup_started", rollup),
          event(2, "preagg_rollup_retracted", rollup)
        ]}
      />
    );

    expect(failedRows(container)).toHaveLength(0);
    expect(spinners(container)).toHaveLength(0);
    expect(container.textContent).toContain("orders.by_day");
    expect(container.textContent).toContain("no rows, rollup removed");
  });

  it("shows a rollup skipped for a missing datasource as a skip, naming the datasource", () => {
    const { container } = render(
      <EventLog
        events={[event(1, "preagg_rollup_skipped_no_datasource", { ...rollup, database: "local" })]}
      />
    );

    expect(failedRows(container)).toHaveLength(0);
    expect(container.textContent).toContain("orders.by_day");
    expect(container.textContent).toContain('skipped, datasource "local" not configured');
  });

  it("keeps a failed rollup red, with its error, and settles its spinner", () => {
    const { container } = render(
      <EventLog
        events={[
          event(1, "preagg_rollup_started", rollup),
          event(2, "preagg_rollup_failed", { ...rollup, error: "connector unavailable" })
        ]}
      />
    );

    expect(failedRows(container)).toHaveLength(1);
    expect(spinners(container)).toHaveLength(0);
    expect(container.textContent).toContain("connector unavailable");
  });

  it("shows a refresh-key error as a failure, with its error", () => {
    const { container } = render(
      <EventLog
        events={[
          event(1, "preagg_refresh_key_error", { rollup_hash: "abc123", error: "probe timed out" })
        ]}
      />
    );

    expect(failedRows(container)).toHaveLength(1);
    expect(container.textContent).toContain("refresh key check failed (abc123)");
    expect(container.textContent).toContain("probe timed out");
  });

  it("does not paint an event type it does not know as a failure", () => {
    const { container } = render(
      <EventLog events={[event(1, "preagg_rollup_something_new", rollup)]} />
    );

    expect(failedRows(container)).toHaveLength(0);
    expect(container.textContent).toContain("orders.by_day");
    expect(container.textContent).toContain("preagg_rollup_something_new");
  });

  it("keeps the spinner on a rollup that has started and not settled", () => {
    const { container } = render(<EventLog events={[event(1, "preagg_rollup_started", rollup)]} />);

    expect(spinners(container)).toHaveLength(1);
  });
});
