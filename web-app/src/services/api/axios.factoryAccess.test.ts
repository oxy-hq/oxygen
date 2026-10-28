// @vitest-environment jsdom

import type { AxiosAdapter } from "axios";
import { AxiosError } from "axios";
import { toast } from "sonner";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { apiClient } from "./axios";

vi.mock("sonner", () => ({ toast: { error: vi.fn(), success: vi.fn() } }));

/** Answers every request as the server does a Factory route the caller may not use. */
const refuse: AxiosAdapter = async (config) => {
  throw new AxiosError("Request failed with status code 403", "ERR_BAD_REQUEST", config, null, {
    status: 403,
    statusText: "Forbidden",
    headers: {},
    config,
    data: {
      code: "factory_access_required",
      message: "The Oxygen Factory is not available to you here."
    }
  });
};

beforeEach(() => {
  apiClient.defaults.adapter = refuse;
  vi.mocked(toast.error).mockClear();
});

describe("a refusal from the Oxygen Factory", () => {
  /// A read is a background query — a page load, a refetch — that the person never
  /// asked for, and a toast for each is what a customer saw four times per page.
  it("is not toasted when a read is refused", async () => {
    await expect(apiClient.get("/ws-1/files")).rejects.toThrow();
    expect(toast.error).not.toHaveBeenCalled();
  });

  it("is toasted once, in the server's words, when a person's action is refused", async () => {
    await expect(apiClient.post("/ws-1/files/abc")).rejects.toThrow();
    await expect(apiClient.put("/ws-1/files/abc")).rejects.toThrow();
    expect(toast.error).toHaveBeenCalledWith("The Oxygen Factory is not available to you here.", {
      id: "factory-access-required"
    });
    expect(toast.error).not.toHaveBeenCalledWith("You don't have permission to do this.");
  });
});
