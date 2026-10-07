// @vitest-environment jsdom

import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import useApprovalArmed from "./useApprovalArmed";

// jsdom never reports its document focused, and the second is only counted on a page that is.
const pageFocused = vi.spyOn(document, "hasFocus");

beforeEach(() => {
  vi.useFakeTimers();
  pageFocused.mockReturnValue(true);
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

const wait = (ms: number) =>
  act(() => {
    vi.advanceTimersByTime(ms);
  });

describe("useApprovalArmed", () => {
  it("arms a second after there is something to approve, and not before", () => {
    const { result, rerender } = renderHook(({ ready }) => useApprovalArmed(ready), {
      initialProps: { ready: false }
    });
    wait(5000);
    expect(result.current).toBe(false);

    rerender({ ready: true });
    wait(999);
    expect(result.current).toBe(false);
    wait(1);
    expect(result.current).toBe(true);
  });

  it("stays armed across renders that change nothing about what is approved", () => {
    const { result, rerender } = renderHook(({ approving }) => useApprovalArmed(true, approving), {
      initialProps: { approving: false }
    });
    wait(1000);
    expect(result.current).toBe(true);
    rerender({ approving: false });
    expect(result.current).toBe(true);
  });

  describe("when what Approve would grant changes", () => {
    /** Every value the hook returned, beside what it was asked about in that render. */
    const watch = () => {
      const seen: { approving: boolean; armed: boolean }[] = [];
      const hook = renderHook(
        ({ approving }) => {
          const armed = useApprovalArmed(true, approving);
          seen.push({ approving, armed });
          return armed;
        },
        { initialProps: { approving: false } }
      );
      return { ...hook, seen };
    };

    it("is off in the very render that shows the change, not one effect later", () => {
      const { rerender, seen } = watch();
      wait(1000);
      expect(seen.at(-1)).toEqual({ approving: false, armed: true });

      rerender({ approving: true });
      // No render ever paired the new request with a button that was already on.
      expect(seen.filter((each) => each.approving && each.armed)).toEqual([]);
    });

    it("counts a fresh second for the new request", () => {
      const { result, rerender } = watch();
      wait(1000);
      rerender({ approving: true });
      expect(result.current).toBe(false);
      wait(999);
      expect(result.current).toBe(false);
      wait(1);
      expect(result.current).toBe(true);
    });

    it("counts it again on the way back, so neither direction rides an old second", () => {
      const { result, rerender } = watch();
      wait(1000);
      rerender({ approving: true });
      wait(1000);
      expect(result.current).toBe(true);

      rerender({ approving: false });
      expect(result.current).toBe(false);
      wait(1000);
      expect(result.current).toBe(true);
    });

    it("drops a second half counted for the request as it was", () => {
      const { result, rerender } = watch();
      wait(600);
      rerender({ approving: true });
      // The 400 ms left of the first count arm nothing.
      wait(400);
      expect(result.current).toBe(false);
      wait(600);
      expect(result.current).toBe(true);
    });
  });
});
