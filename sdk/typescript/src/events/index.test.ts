import { describe, expect, it } from "vitest";
import type { OxyFunctionContext } from "../custom-app/function-context";
import { createTestEvents } from "../testing/events";
import { isTableChange, NonRetryableError, onEvent } from "./index";

const ctx = {} as OxyFunctionContext;

function req(body: unknown) {
  return {
    method: "POST",
    headers: {},
    body: typeof body === "string" ? body : JSON.stringify(body)
  };
}

describe("isTableChange", () => {
  it("tells a table change from an app event by its type", () => {
    const events = createTestEvents();
    const table = events.tableChange({
      table: "pokehouse:bookkeeping.lines",
      snapshotId: 3,
      inserted: 1,
      deleted: 0
    });
    expect(isTableChange(table)).toBe(true);
    expect(table.data.snapshotId).toBe(3);
    expect(isTableChange(events.envelope("locations/statement.uploaded", {}))).toBe(false);
  });
});

describe("onEvent", () => {
  it("passes the parsed envelope and answers 204 when the handler returns nothing", async () => {
    const envelope = createTestEvents().envelope("locations/statement.uploaded", { id: 7 });
    let seen: unknown;
    const handler = onEvent<{ id: number }>((event) => {
      seen = event;
    });
    const res = await handler(req(envelope), ctx);
    expect(res.status).toBe(204);
    expect(seen).toEqual(envelope);
  });

  it("returns the handler's Response", async () => {
    const envelope = createTestEvents().envelope("a/b", {});
    const handler = onEvent(async () => Response.json({ ok: true }, { status: 202 }));
    const res = await handler(req(envelope), ctx);
    expect(res.status).toBe(202);
    expect(await res.json()).toEqual({ ok: true });
  });

  it("answers 400 when the body is not an event", async () => {
    const handler = onEvent(() => {
      throw new Error("must not run");
    });
    for (const body of ["", "not json", "{}", JSON.stringify({ type: "a/b" }), "[1]", "null"]) {
      const res = await handler(req(body), ctx);
      expect(res.status).toBe(400);
    }
  });
});

describe("NonRetryableError", () => {
  it("has name NonRetryableError and survives instanceof", () => {
    const err = new NonRetryableError("bad payload");
    expect(err.name).toBe("NonRetryableError");
    expect(err).toBeInstanceOf(NonRetryableError);
    expect(err).toBeInstanceOf(Error);
    expect(err.message).toBe("bad payload");
  });
});
