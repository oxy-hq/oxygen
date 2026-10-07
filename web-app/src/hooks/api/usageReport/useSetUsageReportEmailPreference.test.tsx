// @vitest-environment jsdom
import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { UsageReportService } from "@/services/api/usageReport";
import type {
  UsageReportEmailPreference,
  UsageReportRecipientsResponse
} from "@/types/usageReport";
import {
  deferred,
  everyone,
  LIST_KEY,
  OWN_KEY,
  ownPreference,
  person,
  seededClient
} from "./testSupport";

vi.mock("@/services/api/usageReport", () => ({
  UsageReportService: {
    setEmailPreference: vi.fn(),
    getEmailPreference: vi.fn(),
    recipients: vi.fn()
  }
}));

import { useSetUsageReportEmailPreference } from "./useSetUsageReportEmailPreference";

const setEmailPreference = vi.mocked(UsageReportService.setEmailPreference);

const mountWith = (seed: {
  own?: UsageReportEmailPreference;
  list?: UsageReportRecipientsResponse;
}) => {
  const client = seededClient(seed);
  const { result } = renderHook(() => useSetUsageReportEmailPreference(), {
    wrapper: client.wrapper
  });
  return { result, ...client };
};

/** Starts a save the test settles by hand, and waits until the optimistic write has landed. */
const startSaving = async (
  mounted: ReturnType<typeof mountWith>,
  enabled: boolean
): Promise<ReturnType<typeof deferred<UsageReportEmailPreference>>> => {
  const inFlight = deferred<UsageReportEmailPreference>();
  setEmailPreference.mockReturnValue(inFlight.promise);
  act(() => {
    mounted.result.current.mutate({ enabled });
  });
  await waitFor(() => expect(setEmailPreference).toHaveBeenCalled());
  return inFlight;
};

const settle = async <T,>(inFlight: ReturnType<typeof deferred<T>>, outcome: T | Error) => {
  await act(async () => {
    if (outcome instanceof Error) inFlight.reject(outcome);
    else inFlight.resolve(outcome);
    await inFlight.promise.catch(() => {});
  });
};

afterEach(() => {
  vi.clearAllMocks();
});

describe("useSetUsageReportEmailPreference", () => {
  it("sends the chosen value to the server", async () => {
    setEmailPreference.mockResolvedValue({ ...ownPreference, enabled: false });
    const { result } = mountWith({ own: ownPreference });
    await act(async () => {
      await result.current.mutateAsync({ enabled: false });
    });
    expect(setEmailPreference.mock.calls).toEqual([[false]]);
  });

  it("moves the switch before the server answers, keeping the rest of the preference", async () => {
    const mounted = mountWith({ own: ownPreference });
    const inFlight = await startSaving(mounted, false);
    // Still in flight: the cache already shows the choice, with the address and the
    // delivery mode it had before.
    await waitFor(() => expect(mounted.own()?.enabled).toBe(false));
    expect(mounted.own()).toEqual({ email: "luong@oxy.tech", enabled: false, delivery: "email" });
    await settle(inFlight, { ...ownPreference, enabled: false });
  });

  it("puts the switch back when the server refuses", async () => {
    const mounted = mountWith({ own: ownPreference });
    const inFlight = await startSaving(mounted, false);
    await waitFor(() => expect(mounted.own()?.enabled).toBe(false));

    await settle(inFlight, new Error("refused"));
    // A switch left showing "off" after a refused save would be a setting nobody holds.
    await waitFor(() => expect(mounted.result.current.isError).toBe(true));
    expect(mounted.own()?.enabled).toBe(true);
  });

  it("does not invent a preference when none has been loaded", async () => {
    const mounted = mountWith({});
    const inFlight = await startSaving(mounted, false);
    // Nothing to be optimistic about: half a preference in the cache would render a page
    // with no address and no delivery mode.
    expect(mounted.own()).toBeUndefined();
    expect(mounted.list()).toBeUndefined();
    await settle(inFlight, { ...ownPreference, enabled: false });
  });
});

/**
 * The caller's row in the list of recipients is this same setting. Someone who can see
 * that list has both on one screen, so a switch that moved only the one they pressed
 * would leave the page saying yes and no about the same email.
 */
describe("useSetUsageReportEmailPreference — the caller's own row in the list", () => {
  it("moves with the switch, before the server answers", async () => {
    const mounted = mountWith({ own: ownPreference, list: everyone() });
    const inFlight = await startSaving(mounted, false);
    await waitFor(() => expect(mounted.row("luong@oxy.tech")?.enabled).toBe(false));
    await settle(inFlight, { ...ownPreference, enabled: false });
  });

  it("is the only row that moves", async () => {
    const mounted = mountWith({ own: ownPreference, list: everyone() });
    const inFlight = await startSaving(mounted, false);
    await waitFor(() => expect(mounted.row("luong@oxy.tech")?.enabled).toBe(false));
    expect(mounted.row("ada@oxy.tech")).toEqual(person({ email: "ada@oxy.tech" }));
    expect(mounted.row("mel@oxy.tech")).toEqual(person({ email: "mel@oxy.tech" }));
    await settle(inFlight, { ...ownPreference, enabled: false });
  });

  it("goes back with the switch when the server refuses", async () => {
    const mounted = mountWith({ own: ownPreference, list: everyone() });
    const inFlight = await startSaving(mounted, false);
    await waitFor(() => expect(mounted.row("luong@oxy.tech")?.enabled).toBe(false));

    await settle(inFlight, new Error("refused"));
    await waitFor(() => expect(mounted.result.current.isError).toBe(true));
    expect(mounted.row("luong@oxy.tech")?.enabled).toBe(true);
    expect(mounted.list()).toEqual(everyone());
  });

  it("is left alone when the list has no row for the caller", async () => {
    // Someone who can read the list without being on it.
    const others = { recipients: [person({ email: "ada@oxy.tech" })] };
    const mounted = mountWith({ own: ownPreference, list: others });
    const inFlight = await startSaving(mounted, false);
    await waitFor(() => expect(mounted.own()?.enabled).toBe(false));
    expect(mounted.list()).toEqual(others);
    await settle(inFlight, { ...ownPreference, enabled: false });
  });

  it("marks both stale once the save settles, so both are re-read from the server", async () => {
    const mounted = mountWith({ own: ownPreference, list: everyone() });
    const inFlight = await startSaving(mounted, false);
    await settle(inFlight, { ...ownPreference, enabled: false });
    await waitFor(() =>
      expect(mounted.queryClient.getQueryState(OWN_KEY)?.isInvalidated).toBe(true)
    );
    expect(mounted.queryClient.getQueryState(LIST_KEY)?.isInvalidated).toBe(true);
  });

  it("does not ask for the list on behalf of someone who is not showing it", async () => {
    // Nobody is rendering the list here — which is the case for a person who may not see
    // it. Marking it stale must not turn into a request the server answers with a 403.
    const mounted = mountWith({ own: ownPreference });
    const inFlight = await startSaving(mounted, false);
    await settle(inFlight, { ...ownPreference, enabled: false });
    await waitFor(() => expect(mounted.result.current.isSuccess).toBe(true));
    expect(UsageReportService.recipients).not.toHaveBeenCalled();
    expect(mounted.list()).toBeUndefined();
  });
});
