// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { TokenEndpoints } from "@/hooks/api/apiKeys/tokenEndpoints";
import type { ApiKey } from "@/types/apiKey";
import ExtendApiKeyPopover from ".";

vi.setConfig({ testTimeout: 20000 });

const mutate = vi.fn();
const useExtend = vi.fn((_endpoints: TokenEndpoints) => ({ mutate, isPending: false }));
// Never called: the mutation hook is mocked. The popover only hands it on.
const endpoints = { keys: { lists: [], activity: () => [] } } as unknown as TokenEndpoints;

// The mutation hook is the seam: this file is about which body the popover sends.
vi.mock("@/hooks/api/apiKeys/useApiKeyMutations", () => ({
  useExtendApiKey: (given: TokenEndpoints) => useExtend(given)
}));

afterEach(() => {
  cleanup();
  mutate.mockReset();
});

const key = (over: Partial<ApiKey> = {}): ApiKey => ({
  id: "k1",
  name: "CI deploy",
  created_at: "2026-01-01T00:00:00Z",
  is_active: true,
  expires_at: new Date(Date.now() + 5 * 24 * 60 * 60 * 1000).toISOString(),
  ...over
});

const open = async (apiKey: ApiKey) => {
  const user = userEvent.setup({ delay: null });
  render(
    <ExtendApiKeyPopover token={apiKey} endpoints={endpoints}>
      <button type='button'>Extend</button>
    </ExtendApiKeyPopover>
  );
  await user.click(screen.getByRole("button", { name: "Extend" }));
  return user;
};

describe("ExtendApiKeyPopover", () => {
  it("defaults to 30 days and sends { days }", async () => {
    const apiKey = key();
    const user = await open(apiKey);
    await user.click(screen.getByTestId("api-key-extend-submit"));
    expect(mutate).toHaveBeenCalledWith(
      { token: apiKey, request: { days: 30 } },
      expect.objectContaining({ onSuccess: expect.any(Function) })
    );
  });

  it("extends through the endpoints it was given, not a fixed route", async () => {
    await open(key());
    expect(useExtend).toHaveBeenCalledWith(endpoints);
  });

  it("sends expires_at: null for No expiry, and the button says so", async () => {
    const apiKey = key();
    const user = await open(apiKey);
    await user.click(screen.getByTestId("api-key-extend-option-never"));
    const submit = screen.getByTestId("api-key-extend-submit");
    expect(submit).toHaveTextContent("Remove expiry");
    await user.click(submit);
    expect(mutate.mock.calls[0][0]).toEqual({ token: apiKey, request: { expires_at: null } });
  });

  it("can't submit the date option until a date is picked", async () => {
    const user = await open(key());
    await user.click(screen.getByTestId("api-key-extend-option-date"));
    expect(screen.getByTestId("api-key-extend-submit")).toBeDisabled();
    expect(screen.getByTestId("api-key-extend-submit")).toHaveTextContent("Pick a date");
  });

  it("tells an expired key's owner that extending brings it back", async () => {
    await open(key({ expires_at: "2020-01-01T00:00:00Z" }));
    expect(screen.getByTestId("api-key-extend-popover")).toHaveTextContent(
      "Extending makes it work again with the same secret."
    );
  });
});
