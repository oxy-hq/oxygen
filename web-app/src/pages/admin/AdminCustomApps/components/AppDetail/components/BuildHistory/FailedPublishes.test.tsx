// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { PublishAttempt } from "@/types/appPublishAttempts";
import { FAILED_SHOWN, FailedPublishes } from "./FailedPublishes";

afterEach(cleanup);

/** A refusal long enough that any truncation would show. */
const LONG_ERROR = `migration 0004_backfill.sql failed: ${'relation "orders" does not exist; '.repeat(
  20
)}end of report`;

const attempt = (over: Partial<PublishAttempt> = {}): PublishAttempt => ({
  id: "a-ok",
  app_build_id: "row-1",
  build_id: "ci-1",
  environment: "staging",
  outcome: "success",
  failed_stage: null,
  error: null,
  started_at: new Date().toISOString(),
  finished_at: new Date().toISOString(),
  duration_ms: 900,
  stages: [
    { stage: "admit", ms: 10 },
    { stage: "inflate", ms: 90 }
  ],
  published_by_email: "dev@acme.com",
  published_via: null,
  published_token_id: null,
  ...over
});

const failure = (id: string, over: Partial<PublishAttempt> = {}) =>
  attempt({
    id,
    outcome: "failed",
    app_build_id: null,
    build_id: `build-${id}`,
    failed_stage: "promote",
    error: LONG_ERROR,
    environment: "production",
    stages: [
      { stage: "admit", ms: 10 },
      { stage: "inflate", ms: 90 },
      { stage: "store", ms: 400 },
      { stage: "record", ms: 20 },
      { stage: "promote", ms: 1200 }
    ],
    ...over
  });

describe("FailedPublishes", () => {
  it("shows nothing when the window has no failure", () => {
    const { container } = render(<FailedPublishes attempts={[attempt(), attempt({ id: "b" })]} />);
    expect(container.innerHTML).toBe("");
    const { container: empty } = render(<FailedPublishes attempts={undefined} />);
    expect(empty.innerHTML).toBe("");
  });

  it("lists each failure with its stage, its error in full and wrapped, and its timings", () => {
    render(<FailedPublishes attempts={[attempt(), failure("f1")]} />);
    const list = screen.getByTestId("admin-app-failed-publishes");
    expect(list.querySelector("h3")?.textContent).toBe("Failed publishes 1");

    expect(screen.getByTestId("admin-app-failed-publish-f1-stage").textContent).toBe(
      "Failed at promote"
    );
    const error = screen.getByTestId("admin-app-failed-publish-f1-error");
    expect(error.textContent).toBe(LONG_ERROR);
    expect(error.className).toContain("whitespace-pre-wrap");
    expect(error.className).toContain("break-words");
    expect(error.className).not.toContain("truncate");

    const row = screen.getByTestId("admin-app-failed-publish-f1");
    expect(row.textContent).toContain("dev@acme.com");
    expect(row.textContent).toContain("to production");
    expect(screen.getByTestId("admin-app-failed-publish-f1-stages-label-promote").textContent).toBe(
      "promote 1.2s"
    );
    expect(screen.queryByTestId("admin-app-failed-publish-a-ok")).toBeNull();
  });

  it("says Failed alone for a refusal outside any stage", () => {
    render(<FailedPublishes attempts={[failure("f1", { failed_stage: null, stages: [] })]} />);
    expect(screen.getByTestId("admin-app-failed-publish-f1-stage").textContent).toBe("Failed");
    expect(screen.queryByTestId("admin-app-failed-publish-f1-stages")).toBeNull();
  });

  it("folds the failures past the newest few behind a disclosure", () => {
    const failures = Array.from({ length: FAILED_SHOWN + 2 }, (_, i) => failure(`f${i}`));
    render(<FailedPublishes attempts={failures} />);
    expect(screen.getByTestId("admin-app-failed-publishes").querySelector("h3")?.textContent).toBe(
      `Failed publishes ${FAILED_SHOWN + 2}`
    );
    expect(screen.getByTestId(`admin-app-failed-publish-f${FAILED_SHOWN - 1}`)).toBeTruthy();
    expect(screen.queryByTestId(`admin-app-failed-publish-f${FAILED_SHOWN}`)).toBeNull();

    const older = screen.getByTestId("admin-app-failed-publishes-older");
    const trigger = older.querySelector("button");
    expect(trigger?.textContent).toBe("2 older failures");
    fireEvent.click(trigger as HTMLButtonElement);
    expect(screen.getByTestId(`admin-app-failed-publish-f${FAILED_SHOWN + 1}`)).toBeTruthy();
  });
});
