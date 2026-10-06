// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";

afterEach(() => {
  vi.useRealTimers();
  vi.resetModules();
});

// `persist-and-sync` restores a store from storage on a 100 ms timer that reads
// `document`. A test file that finishes sooner has lost its jsdom by then, and
// the read fails the whole run as an unhandled error with every test green —
// on whichever file the worker timing picks. `test-setup.ts` takes the library
// out of unit tests; this pins that it stays out.
it("importing a persisted store leaves no timer for the environment teardown to race", async () => {
  vi.useFakeTimers();
  vi.resetModules();
  await import("@/stores/useIdeBranch");
  await import("@/stores/useDatabaseOperation");
  expect(vi.getTimerCount()).toBe(0);
});
