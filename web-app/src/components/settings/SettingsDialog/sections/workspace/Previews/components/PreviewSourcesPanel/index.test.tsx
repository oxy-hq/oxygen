// @vitest-environment jsdom

/**
 * The Sandbox sources panel: list (`GET …/previews/sources`), register
 * (`PUT`), edit an existing row, and the four save refusals mapped to field
 * text — driven down to what reaches `apiClient`, same style as
 * `Previews.test.tsx`.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AxiosError, AxiosHeaders, type InternalAxiosRequestConfig } from "axios";
import { createElement } from "react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { PreviewSourceItem } from "@/types/workspace";

const mocks = vi.hoisted(() => ({ get: vi.fn(), put: vi.fn(), toastError: vi.fn() }));
vi.mock("@/services/api/axios", () => ({ apiClient: { get: mocks.get, put: mocks.put } }));
vi.mock("sonner", () => ({ toast: { error: mocks.toastError, success: vi.fn() } }));

import PreviewSourcesPanel from "./index";

// Radix's Select (the token-type picker) reaches for pointer-capture and
// scrollIntoView, neither of which jsdom implements — without them the
// trigger click throws instead of opening the listbox.
beforeAll(() => {
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.setPointerCapture ??= () => {};
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
});

const SOURCES_PATH = "/ws-1/previews/sources";

function apiError(status: number, data: unknown) {
  const config = { headers: new AxiosHeaders() } as InternalAxiosRequestConfig;
  return new AxiosError(
    `Request failed with status code ${status}`,
    "ERR_BAD_REQUEST",
    config,
    null,
    { status, statusText: "", headers: new AxiosHeaders(), config, data }
  );
}

const source = (over: Partial<PreviewSourceItem>): PreviewSourceItem => ({
  pipeline: "quickbooks_financials_eastbay",
  environment: "sandbox",
  overrides: {
    realm_id: "4620816365000000",
    refresh_token_var: "QB_SANDBOX_REFRESH_TOKEN__EASTBAY",
    client_id_var: "QB_SANDBOX_CLIENT_ID",
    client_secret_var: "QB_SANDBOX_CLIENT_SECRET"
  },
  updated_by: "u1",
  updated_at: "2026-09-30T08:00:00.000Z",
  ...over
});

function serve(sources: PreviewSourceItem[]) {
  mocks.get.mockResolvedValue({ data: sources });
}

let client: QueryClient;

function renderPanel() {
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    createElement(
      QueryClientProvider,
      { client },
      createElement(PreviewSourcesPanel, { workspaceId: "ws-1" })
    )
  );
}

describe("PreviewSourcesPanel", () => {
  beforeEach(() => {
    mocks.get.mockReset();
    mocks.put.mockReset();
    mocks.toastError.mockReset();
    serve([]);
  });
  afterEach(() => cleanup());

  it("shows 'no sandbox sources registered yet' when the list is empty", async () => {
    renderPanel();
    expect(await screen.findByText("No sandbox sources registered yet.")).toBeInTheDocument();
  });

  it("lists a registered source's pipeline, realm and rotating var, never a secret value", async () => {
    serve([source({})]);
    renderPanel();

    const row = await screen.findByTestId("preview-source-quickbooks_financials_eastbay");
    expect(row).toHaveTextContent("quickbooks_financials_eastbay");
    expect(row).toHaveTextContent("4620816365000000");
    expect(row).toHaveTextContent("QB_SANDBOX_REFRESH_TOKEN__EASTBAY");
  });

  it("registers a new source with a refresh token, environment fixed to sandbox", async () => {
    mocks.put.mockResolvedValue({ data: source({}) });
    renderPanel();

    fireEvent.click(await screen.findByTestId("preview-sources-add-toggle"));
    fireEvent.change(screen.getByTestId("preview-source-pipeline"), {
      target: { value: "quickbooks_financials_eastbay" }
    });
    fireEvent.change(screen.getByTestId("preview-source-realm"), {
      target: { value: "4620816365000000" }
    });
    fireEvent.change(screen.getByTestId("preview-source-token-var"), {
      target: { value: "QB_SANDBOX_REFRESH_TOKEN__EASTBAY" }
    });
    fireEvent.change(screen.getByTestId("preview-source-client-secret-var"), {
      target: { value: "QB_SANDBOX_CLIENT_SECRET" }
    });
    fireEvent.change(screen.getByTestId("preview-source-client-id-var"), {
      target: { value: "QB_SANDBOX_CLIENT_ID" }
    });
    fireEvent.click(screen.getByTestId("preview-source-submit"));

    await waitFor(() =>
      expect(mocks.put).toHaveBeenCalledWith(SOURCES_PATH, {
        pipeline: "quickbooks_financials_eastbay",
        environment: "sandbox",
        overrides: {
          realm_id: "4620816365000000",
          refresh_token_var: "QB_SANDBOX_REFRESH_TOKEN__EASTBAY",
          client_secret_var: "QB_SANDBOX_CLIENT_SECRET",
          client_id_var: "QB_SANDBOX_CLIENT_ID"
        }
      })
    );
    // The form closes on success.
    await waitFor(() => expect(screen.queryByTestId("preview-source-form")).toBeNull());
  });

  it("sends access_token_var instead of refresh_token_var once the token type is switched", async () => {
    const user = userEvent.setup({ delay: null });
    mocks.put.mockResolvedValue({ data: source({}) });
    renderPanel();

    fireEvent.click(await screen.findByTestId("preview-sources-add-toggle"));
    fireEvent.change(screen.getByTestId("preview-source-pipeline"), {
      target: { value: "quickbooks_financials_eastbay" }
    });
    fireEvent.change(screen.getByTestId("preview-source-realm"), {
      target: { value: "4620816365000000" }
    });

    // Radix's Select opens on a pointer event, not a plain `fireEvent.click`.
    await user.click(screen.getByTestId("preview-source-token-mode"));
    await user.click(await screen.findByRole("option", { name: "Access token (static)" }));

    fireEvent.change(screen.getByTestId("preview-source-token-var"), {
      target: { value: "QB_SANDBOX_ACCESS_TOKEN" }
    });
    fireEvent.click(screen.getByTestId("preview-source-submit"));

    await waitFor(() =>
      expect(mocks.put).toHaveBeenCalledWith(
        SOURCES_PATH,
        expect.objectContaining({
          overrides: expect.objectContaining({ access_token_var: "QB_SANDBOX_ACCESS_TOKEN" })
        })
      )
    );
    expect(mocks.put.mock.calls[0][1].overrides.refresh_token_var).toBeUndefined();
  });

  it('refuses a pipeline name starting with "preview:" without submitting', async () => {
    renderPanel();
    fireEvent.click(await screen.findByTestId("preview-sources-add-toggle"));
    fireEvent.change(screen.getByTestId("preview-source-pipeline"), {
      target: { value: "preview:feat_x_92a1b7:quickbooks" }
    });
    fireEvent.change(screen.getByTestId("preview-source-realm"), { target: { value: "123" } });
    fireEvent.change(screen.getByTestId("preview-source-token-var"), { target: { value: "VAR" } });
    fireEvent.change(screen.getByTestId("preview-source-client-secret-var"), {
      target: { value: "SECRET" }
    });
    fireEvent.click(screen.getByTestId("preview-source-submit"));

    expect(await screen.findByTestId("preview-source-pipeline-error")).toHaveTextContent(
      "reserved"
    );
    expect(mocks.put).not.toHaveBeenCalled();
  });

  it("requires a client secret var when using a refresh token", async () => {
    renderPanel();
    fireEvent.click(await screen.findByTestId("preview-sources-add-toggle"));
    fireEvent.change(screen.getByTestId("preview-source-pipeline"), {
      target: { value: "quickbooks_financials_eastbay" }
    });
    fireEvent.change(screen.getByTestId("preview-source-realm"), { target: { value: "123" } });
    fireEvent.change(screen.getByTestId("preview-source-token-var"), { target: { value: "VAR" } });
    fireEvent.click(screen.getByTestId("preview-source-submit"));

    expect(await screen.findByTestId("preview-source-client-secret-var-error")).toHaveTextContent(
      "Required with a refresh token."
    );
    expect(mocks.put).not.toHaveBeenCalled();
  });

  it("edit pre-fills the form and locks the pipeline field", async () => {
    serve([source({})]);
    renderPanel();

    fireEvent.click(await screen.findByTestId("preview-source-quickbooks_financials_eastbay-edit"));

    const pipelineInput = screen.getByTestId("preview-source-pipeline") as HTMLInputElement;
    expect(pipelineInput).toHaveValue("quickbooks_financials_eastbay");
    expect(pipelineInput).toBeDisabled();
    expect(screen.getByTestId("preview-source-realm")).toHaveValue("4620816365000000");
    expect(screen.getByTestId("preview-source-token-var")).toHaveValue(
      "QB_SANDBOX_REFRESH_TOKEN__EASTBAY"
    );
  });

  it.each([
    ["production_var", 409, "preview-source-token-var-error", "sandbox sources need their own"],
    ["production_realm", 409, "preview-source-realm-error", "different sandbox company"],
    ["rotating_var_taken", 409, "preview-source-token-var-error", "each one needs its own"],
    ["reserved_var", 409, "preview-source-token-var-error", "sandbox-only secret name"]
  ] as const)(
    "maps %s to a field error, not a toast",
    async (code, status, fieldTestId, snippet) => {
      mocks.put.mockRejectedValue(apiError(status, { code }));
      renderPanel();

      fireEvent.click(await screen.findByTestId("preview-sources-add-toggle"));
      fireEvent.change(screen.getByTestId("preview-source-pipeline"), {
        target: { value: "quickbooks_financials_eastbay" }
      });
      fireEvent.change(screen.getByTestId("preview-source-realm"), { target: { value: "123" } });
      fireEvent.change(screen.getByTestId("preview-source-token-var"), {
        target: { value: "VAR" }
      });
      fireEvent.change(screen.getByTestId("preview-source-client-secret-var"), {
        target: { value: "SECRET" }
      });
      fireEvent.click(screen.getByTestId("preview-source-submit"));

      expect(await screen.findByTestId(fieldTestId)).toHaveTextContent(snippet);
      expect(mocks.toastError).not.toHaveBeenCalled();
    }
  );

  it("cancel closes the form without submitting", async () => {
    renderPanel();
    fireEvent.click(await screen.findByTestId("preview-sources-add-toggle"));
    fireEvent.click(screen.getByTestId("preview-source-cancel"));
    expect(screen.queryByTestId("preview-source-form")).toBeNull();
    expect(mocks.put).not.toHaveBeenCalled();
  });
});
