// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { useTestOltpConnection } from "@/hooks/api/oltp/useAdminOltp";
import type { OltpConnectionTest as Result } from "@/services/api/oltp";
import { OltpConnectionTest } from "./OltpConnectionTest";

afterEach(cleanup);

type Probe = ReturnType<typeof useTestOltpConnection>;

// The mutation is the seam: this file is about what the panel says about a
// result, not React Query's plumbing.
const probe = (over: { data?: Result; isPending?: boolean; mutate?: () => void }): Probe =>
  ({ data: undefined, isPending: false, mutate: vi.fn(), ...over }) as unknown as Probe;

// The refusal behind the 2026-10-21 outage, as `pg_detail` renders it.
const QUOTA_REFUSAL: Result = {
  ok: false,
  stage: "connect",
  error:
    "[XX000] Your account or project has exceeded the quota. Upgrade your plan to increase limits.",
  role: "oxy_analyst_ro",
  host: "ep-shiny-recipe.c-4.us-west-2.aws.neon.tech",
  elapsed_ms: 412
};

describe("OLTP connection test", () => {
  it("probes only when asked", () => {
    const mutate = vi.fn();
    render(<OltpConnectionTest probe={probe({ mutate })} />);
    expect(mutate).not.toHaveBeenCalled();
    expect(screen.queryByTestId("admin-org-oltp-test-verdict")).toBeNull();

    fireEvent.click(screen.getByTestId("admin-org-oltp-test-run"));
    expect(mutate).toHaveBeenCalledTimes(1);
  });

  it("prints a refusal whole, in the server's words, with the stage it stopped at", () => {
    render(<OltpConnectionTest probe={probe({ data: QUOTA_REFUSAL })} />);
    expect(screen.getByTestId("admin-org-oltp-test-verdict").textContent).toContain(
      "The database refused the session"
    );
    expect(screen.getByTestId("admin-org-oltp-test-error").textContent).toBe(QUOTA_REFUSAL.error);
  });

  it("says who answered and how fast, and prints no error, when it connects", () => {
    render(
      <OltpConnectionTest
        probe={probe({
          data: { ...QUOTA_REFUSAL, ok: true, stage: null, error: null, elapsed_ms: 87 }
        })}
      />
    );
    const verdict = screen.getByTestId("admin-org-oltp-test-verdict").textContent ?? "";
    expect(verdict).toContain("oxy_analyst_ro");
    expect(verdict).toContain("87 ms");
    expect(screen.queryByTestId("admin-org-oltp-test-error")).toBeNull();
  });

  it("cannot be fired twice while a probe is out", () => {
    render(<OltpConnectionTest probe={probe({ isPending: true })} />);
    const run = screen.getByTestId("admin-org-oltp-test-run") as HTMLButtonElement;
    expect(run.disabled).toBe(true);
    expect(run.textContent).toContain("Testing");
  });
});
